//! Provider router — the central LLM request dispatcher.
//!
//! The router selects the best provider for each request based on model class,
//! priority, health, rate limits, budget, and key availability. It implements
//! automatic failover with circuit breaker protection.
//!
//! # Routing algorithm
//!
//! 1. Filter providers: enabled, has requested model class, healthy (or can retry)
//! 2. Sort by priority (lower = higher priority)
//! 3. For each candidate: check rate limit → check budget → get available key
//! 4. On failure: record error, try next provider in chain
//! 5. Circuit breaker: 3 consecutive failures → cooldown with exponential backoff
//!
//! # Event emission
//!
//! The router emits events at every decision point for observability:
//! `LlmRequestSent`, `LlmResponseReceived`, `LlmRequestFailed`,
//! `LlmKeyExhausted`, `LlmProviderDown`, `LlmProviderRecovered`,
//! `LlmProviderSwitched`, `LlmBudgetExhausted`.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::RwLock;

use argus_core::event_bus::EventBus;
use argus_core::events::{ArgusEvent, EventSource};
use argus_core::types::{ArgusError, ArgusResult};

use crate::budget::BudgetTracker;
use crate::config::{ModelConfig, ProvidersConfig};
use crate::key_rotation::{ApiErrorKind, KeyRotator};
use crate::provider::LlmProvider;
use crate::types::{
    BudgetCheck, FallbackOption, LlmRequest, LlmResponse, ModelClass, ModelInfo, ProviderHealth,
    RateLimitConfig, RateLimitState, RoutingDecision, UsageRecord,
};

#[async_trait::async_trait]
impl argus_core::llm_client::LlmClient for ProviderRouter {
    async fn chat(
        &self,
        request: &argus_core::llm_client::LlmRequest,
    ) -> Result<argus_core::llm_client::LlmResponse, argus_core::llm_client::LlmError> {
        use argus_core::llm_client::{ChatRole, LlmError};

        let model_class = match request.purpose.as_str() {
            "task_decomposition" | "code_review" | "intent_analysis" => ModelClass::Reasoning,
            purpose if purpose.contains("code") => ModelClass::Coding,
            _ => ModelClass::Fast,
        };
        let messages = request
            .messages
            .iter()
            .map(|message| match message.role {
                ChatRole::System => crate::types::ChatMessage::system(&message.content),
                ChatRole::User => crate::types::ChatMessage::user(&message.content),
                ChatRole::Assistant => crate::types::ChatMessage::assistant(&message.content),
            })
            .collect();
        let mut routed = LlmRequest::new(model_class, messages, request.purpose.clone());
        routed.max_tokens = request.max_tokens;
        routed.temperature = request.temperature;

        ProviderRouter::chat(self, &routed)
            .await
            .map(|response| argus_core::llm_client::LlmResponse {
                content: response.content,
                provider: response.provider,
                model: response.model,
            })
            .map_err(|error| LlmError::RequestFailed(error.to_string()))
    }
}

// ============================================================
// Constants
// ============================================================

/// Number of consecutive failures before the circuit breaker trips.
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;

// ============================================================
// Registered provider entry
// ============================================================

/// A provider registered with the router, bundled with its operational state.
struct RegisteredProvider {
    /// The provider implementation.
    provider: Box<dyn LlmProvider>,
    /// Priority (lower = higher priority).
    priority: u32,
    /// Key rotator for this provider.
    key_rotator: KeyRotator,
    /// Health state with circuit breaker.
    health: RwLock<ProviderHealth>,
    /// Rate limit state.
    rate_limit: RwLock<RateLimitState>,
}

// ============================================================
// ProviderRouter
// ============================================================

/// The central LLM request router.
///
/// Holds all registered providers and their operational state. Routes
/// requests to the best available provider with automatic failover.
pub struct ProviderRouter {
    /// Registered providers, keyed by provider ID.
    providers: HashMap<String, RegisteredProvider>,
    /// Provider IDs sorted by priority (ascending = highest priority first).
    priority_order: Vec<String>,
    /// Shared budget tracker.
    budget: Arc<BudgetTracker>,
    /// Shared event bus.
    event_bus: Arc<EventBus>,
}

impl ProviderRouter {
    /// Create a new router with the given budget tracker and event bus.
    ///
    /// Providers must be registered after construction using `register()`.
    pub fn new(budget: Arc<BudgetTracker>, event_bus: Arc<EventBus>) -> Self {
        Self {
            providers: HashMap::new(),
            priority_order: Vec::new(),
            budget,
            event_bus,
        }
    }

    /// Build a router from a `ProvidersConfig` TOML configuration.
    ///
    /// This factory method creates concrete provider instances from config
    /// and registers them. Unknown provider types are logged and skipped.
    pub fn from_config(
        config: &ProvidersConfig,
        budget: Arc<BudgetTracker>,
        event_bus: Arc<EventBus>,
    ) -> Self {
        let mut router = Self::new(budget, event_bus);

        for (id, prov_config) in &config.providers {
            if !prov_config.enabled {
                tracing::info!(provider = id, "Provider disabled, skipping");
                continue;
            }

            let models = Self::build_model_infos(&prov_config.models);
            let rate_limit_cfg = prov_config.rate_limit.clone().unwrap_or(RateLimitConfig {
                rpm: None,
                tpm: None,
            });

            match prov_config.provider_type.as_str() {
                "openai_compatible" => {
                    let base_url = prov_config
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                    let provider = crate::providers::openai::OpenAiCompatibleProvider::new(
                        id.clone(),
                        id.clone(), // name = id for now
                        base_url,
                        models,
                    );
                    let key_rotator = KeyRotator::new(id, prov_config.api_keys.clone());
                    router.register_with_state(
                        Box::new(provider),
                        prov_config.priority,
                        key_rotator,
                        rate_limit_cfg,
                    );
                }
                "anthropic" => {
                    let base_url = prov_config
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.anthropic.com".to_string());
                    let provider = crate::providers::anthropic::AnthropicProvider::new(
                        id.clone(),
                        id.clone(),
                        base_url,
                        models,
                    );
                    let key_rotator = KeyRotator::new(id, prov_config.api_keys.clone());
                    router.register_with_state(
                        Box::new(provider),
                        prov_config.priority,
                        key_rotator,
                        rate_limit_cfg,
                    );
                }
                "ollama" => {
                    let base_url = prov_config
                        .base_url
                        .clone()
                        .unwrap_or_else(|| "http://localhost:11434".to_string());
                    let provider = crate::providers::ollama::OllamaProvider::new(
                        id.clone(),
                        id.clone(),
                        base_url,
                        models,
                    );
                    let key_rotator = KeyRotator::none(id);
                    router.register_with_state(
                        Box::new(provider),
                        prov_config.priority,
                        key_rotator,
                        rate_limit_cfg,
                    );
                }
                "web_scraper" => {
                    tracing::info!(
                        provider = id,
                        "Web scraper provider type not yet implemented, skipping"
                    );
                }
                other => {
                    tracing::warn!(
                        provider = id,
                        provider_type = other,
                        "Unknown provider type, skipping"
                    );
                }
            }

            // Set provider budget if configured
            if let Some(monthly) = prov_config.monthly_budget_usd {
                router.budget.set_provider_budget(id, monthly);
            }
        }

        router
    }

    /// Register a provider with all its operational state.
    fn register_with_state(
        &mut self,
        provider: Box<dyn LlmProvider>,
        priority: u32,
        key_rotator: KeyRotator,
        rate_limit_config: RateLimitConfig,
    ) {
        let id = provider.id().to_string();
        tracing::info!(
            provider = id,
            priority = priority,
            models = provider.models().len(),
            "Registering LLM provider"
        );

        self.providers.insert(
            id.clone(),
            RegisteredProvider {
                provider,
                priority,
                key_rotator,
                health: RwLock::new(ProviderHealth::default()),
                rate_limit: RwLock::new(RateLimitState::new(rate_limit_config)),
            },
        );

        // Rebuild priority order
        self.rebuild_priority_order();
    }

    /// Register a provider with manual configuration (for testing or dynamic registration).
    pub fn register(
        &mut self,
        provider: Box<dyn LlmProvider>,
        priority: u32,
        api_keys: Vec<String>,
        rate_limit_config: RateLimitConfig,
    ) {
        let id = provider.id().to_string();
        let key_rotator = if api_keys.is_empty() {
            KeyRotator::none(&id)
        } else {
            KeyRotator::new(&id, api_keys)
        };
        self.register_with_state(provider, priority, key_rotator, rate_limit_config);
    }

    /// Rebuild the priority-sorted provider order.
    fn rebuild_priority_order(&mut self) {
        let mut entries: Vec<(String, u32)> = self
            .providers
            .iter()
            .map(|(id, rp)| (id.clone(), rp.priority))
            .collect();
        entries.sort_by_key(|(_, p)| *p);
        self.priority_order = entries.into_iter().map(|(id, _)| id).collect();
    }

    /// Convert `ModelConfig` list to `ModelInfo` list.
    fn build_model_infos(configs: &[ModelConfig]) -> Vec<ModelInfo> {
        configs
            .iter()
            .map(|mc| ModelInfo {
                id: mc.id.clone(),
                class: ModelClass::from_str_loose(&mc.class).unwrap_or(ModelClass::Coding),
                cost_per_1k_input: mc.cost_per_1k_input,
                cost_per_1k_output: mc.cost_per_1k_output,
                max_context_tokens: mc.max_context_tokens,
            })
            .collect()
    }

    // ============================================================
    // Routing
    // ============================================================

    /// Select the best provider and model for the given request.
    ///
    /// Returns a `RoutingDecision` with the selected provider, model, key,
    /// estimated cost, and ordered fallback options.
    pub async fn route(&self, request: &LlmRequest) -> ArgusResult<RoutingDecision> {
        // If force_provider is set, try only that provider
        if let Some(ref forced_id) = request.force_provider {
            return self.route_forced(forced_id, request).await;
        }

        let mut fallbacks = Vec::new();

        for provider_id in &self.priority_order {
            let rp = match self.providers.get(provider_id) {
                Some(rp) => rp,
                None => continue,
            };

            // 1. Check enabled
            if !rp.provider.is_enabled() {
                continue;
            }

            // 2. Check model class availability
            let model = if let Some(ref forced_model) = request.force_model {
                rp.provider.get_model(forced_model)
            } else {
                rp.provider.best_model_for_class(&request.model_class)
            };
            let model = match model {
                Some(m) => m,
                None => continue,
            };

            // 3. Check health (circuit breaker)
            {
                let health = rp.health.read().await;
                if !health.can_retry() {
                    continue;
                }
            }

            // 4. Check rate limit
            {
                let mut rl = rp.rate_limit.write().await;
                if !rl.check() {
                    tracing::debug!(provider = provider_id, "Rate limited, trying next provider");
                    continue;
                }
            }

            // 5. Check budget
            let estimated_cost = model.estimate_cost(
                request.estimated_input_tokens(),
                request.max_tokens.unwrap_or(1000),
            );
            let budget_check = self
                .budget
                .check_budget(
                    provider_id,
                    request.project_id,
                    request.task_id,
                    estimated_cost,
                )
                .await;

            if let BudgetCheck::OverBudget { scope, .. } = budget_check {
                tracing::info!(
                    provider = provider_id,
                    scope = %scope,
                    "Budget exceeded, trying next provider"
                );
                self.event_bus.publish(
                    EventSource::LlmProvider,
                    ArgusEvent::LlmBudgetExhausted {
                        provider: provider_id.clone(),
                    },
                );
                continue;
            }

            // 6. Get available key
            let selected_key = match rp.key_rotator.get_available_key().await {
                Some(k) => k,
                None => {
                    tracing::warn!(
                        provider = provider_id,
                        "All keys exhausted, trying next provider"
                    );
                    self.event_bus.publish(
                        EventSource::LlmProvider,
                        ArgusEvent::LlmKeyExhausted {
                            provider: provider_id.clone(),
                            key_index: 0,
                            next_key_index: None,
                        },
                    );
                    continue;
                }
            };

            // Collect remaining candidates as fallbacks
            let remaining_start = self
                .priority_order
                .iter()
                .position(|id| id == provider_id)
                .map(|i| i + 1)
                .unwrap_or(self.priority_order.len());

            for fb_id in &self.priority_order[remaining_start..] {
                if let Some(fb_rp) = self.providers.get(fb_id) {
                    if fb_rp.provider.is_enabled() {
                        if let Some(fb_model) =
                            fb_rp.provider.best_model_for_class(&request.model_class)
                        {
                            fallbacks.push(FallbackOption {
                                provider_id: fb_id.clone(),
                                model_id: fb_model.id.clone(),
                            });
                        }
                    }
                }
            }

            return Ok(RoutingDecision {
                provider_id: provider_id.clone(),
                model_id: model.id.clone(),
                key_index: selected_key.index,
                estimated_cost,
                fallbacks,
            });
        }

        Err(ArgusError::Llm(format!(
            "No provider available for model class '{}'. Last error: all providers filtered out",
            request.model_class,
        )))
    }

    /// Route to a specific forced provider.
    async fn route_forced(
        &self,
        provider_id: &str,
        request: &LlmRequest,
    ) -> ArgusResult<RoutingDecision> {
        let rp = self
            .providers
            .get(provider_id)
            .ok_or_else(|| ArgusError::Llm(format!("Forced provider '{provider_id}' not found")))?;

        let model = if let Some(ref forced_model) = request.force_model {
            rp.provider.get_model(forced_model)
        } else {
            rp.provider.best_model_for_class(&request.model_class)
        };
        let model = model.ok_or_else(|| {
            ArgusError::Llm(format!(
                "Forced provider '{provider_id}' has no model for class '{}'",
                request.model_class
            ))
        })?;

        let selected_key = rp.key_rotator.get_available_key().await.ok_or_else(|| {
            ArgusError::Llm(format!(
                "Forced provider '{provider_id}' has no available API keys"
            ))
        })?;

        let estimated_cost = model.estimate_cost(
            request.estimated_input_tokens(),
            request.max_tokens.unwrap_or(1000),
        );

        Ok(RoutingDecision {
            provider_id: provider_id.to_string(),
            model_id: model.id.clone(),
            key_index: selected_key.index,
            estimated_cost,
            fallbacks: vec![],
        })
    }

    // ============================================================
    // Chat (high-level convenience method)
    // ============================================================

    /// Send a chat completion request with automatic routing and failover.
    ///
    /// This is the primary entry point for LLM calls. It:
    /// 1. Routes to the best provider
    /// 2. Emits `LlmRequestSent`
    /// 3. Calls the provider
    /// 4. On success: records usage, emits `LlmResponseReceived`, updates health
    /// 5. On failure: emits `LlmRequestFailed`, updates health/keys, fails over
    pub async fn chat(&self, request: &LlmRequest) -> ArgusResult<LlmResponse> {
        let decision = self.route(request).await?;
        self.execute_with_failover(request, decision).await
    }

    /// Execute a request with the given routing decision, failing over to
    /// fallback providers on error.
    async fn execute_with_failover(
        &self,
        request: &LlmRequest,
        decision: RoutingDecision,
    ) -> ArgusResult<LlmResponse> {
        // Try primary provider
        let mut current_provider_id = decision.provider_id.clone();
        let mut current_model_id = decision.model_id.clone();
        let mut fallback_iter = decision.fallbacks.iter();
        let mut fallback_hop: u32 = 0;

        loop {
            let rp = match self.providers.get(&current_provider_id) {
                Some(rp) => rp,
                None => {
                    return Err(ArgusError::Llm(format!(
                        "Provider '{current_provider_id}' not found during execution"
                    )));
                }
            };

            // Get a key for this attempt
            let selected_key = match rp.key_rotator.get_available_key().await {
                Some(k) => k,
                None => {
                    // No keys available, try next fallback
                    if let Some(fb) = fallback_iter.next() {
                        fallback_hop += 1;
                        self.emit_provider_switch(
                            &current_provider_id,
                            &fb.provider_id,
                            "all keys exhausted",
                            request.trace_id.as_deref(),
                            fallback_hop,
                        );
                        current_provider_id = fb.provider_id.clone();
                        current_model_id = fb.model_id.clone();
                        continue;
                    }
                    return Err(ArgusError::Llm(format!(
                        "No keys available for provider '{current_provider_id}' and no fallbacks remain"
                    )));
                }
            };

            // Emit LlmRequestSent
            self.event_bus.publish(
                EventSource::LlmProvider,
                ArgusEvent::LlmRequestSent {
                    provider: current_provider_id.clone(),
                    model: current_model_id.clone(),
                    model_class: request.model_class.to_string(),
                    estimated_tokens: request.estimated_input_tokens(),
                    purpose: request.purpose.clone(),
                    trace_id: request.trace_id.clone(),
                },
            );

            // Record rate limit
            {
                let mut rl = rp.rate_limit.write().await;
                rl.record(request.estimated_input_tokens());
            }

            // Execute
            let result = rp
                .provider
                .complete(
                    &current_model_id,
                    &request.messages,
                    request.max_tokens,
                    request.temperature,
                    &selected_key.key,
                )
                .await;

            match result {
                Ok(response) => {
                    // Success path
                    rp.key_rotator
                        .report_success(selected_key.index, response.usage.total_tokens)
                        .await;

                    // Update health
                    {
                        let mut health = rp.health.write().await;
                        let was_down = !health.is_healthy;
                        health.record_success(response.latency_ms);
                        if was_down {
                            self.event_bus.publish(
                                EventSource::LlmProvider,
                                ArgusEvent::LlmProviderRecovered {
                                    provider: current_provider_id.clone(),
                                },
                            );
                        }
                    }

                    // Record usage
                    let usage_record = UsageRecord {
                        provider: response.provider.clone(),
                        model: response.model.clone(),
                        model_class: request.model_class.clone(),
                        input_tokens: response.usage.input_tokens,
                        output_tokens: response.usage.output_tokens,
                        cost_usd: response.cost_usd,
                        project_id: request.project_id,
                        task_id: request.task_id,
                        timestamp: Utc::now(),
                        latency_ms: response.latency_ms,
                        purpose: request.purpose.clone(),
                    };
                    self.budget.record_usage(&usage_record).await;

                    // Emit LlmResponseReceived
                    self.event_bus.publish(
                        EventSource::LlmProvider,
                        ArgusEvent::LlmResponseReceived {
                            provider: response.provider.clone(),
                            model: response.model.clone(),
                            input_tokens: response.usage.input_tokens,
                            output_tokens: response.usage.output_tokens,
                            cost_usd: response.cost_usd,
                            latency_ms: response.latency_ms,
                            trace_id: request.trace_id.clone(),
                        },
                    );

                    return Ok(response);
                }
                Err(error) => {
                    let error_str = error.to_string();
                    let has_fallback = fallback_iter.len() > 0;

                    // Classify the error for key rotation
                    let error_kind = Self::classify_error(&error_str);

                    // Report error to key rotator
                    rp.key_rotator
                        .report_error(selected_key.index, error_kind)
                        .await;

                    // Update health / circuit breaker
                    let provider_went_down = {
                        let mut health = rp.health.write().await;
                        health.record_failure(CIRCUIT_BREAKER_THRESHOLD)
                    };

                    if provider_went_down {
                        self.event_bus.publish(
                            EventSource::LlmProvider,
                            ArgusEvent::LlmProviderDown {
                                provider: current_provider_id.clone(),
                                error: error_str.clone(),
                            },
                        );
                    }

                    // Emit LlmRequestFailed
                    self.event_bus.publish(
                        EventSource::LlmProvider,
                        ArgusEvent::LlmRequestFailed {
                            provider: current_provider_id.clone(),
                            model: current_model_id.clone(),
                            error: error_str.clone(),
                            will_failover: has_fallback,
                            trace_id: request.trace_id.clone(),
                        },
                    );

                    tracing::warn!(
                        trace_id = ?request.trace_id,
                        provider = current_provider_id,
                        model = current_model_id,
                        error = error_str,
                        has_fallback = has_fallback,
                        "LLM request failed"
                    );

                    // Try next fallback
                    if let Some(fb) = fallback_iter.next() {
                        fallback_hop += 1;
                        self.emit_provider_switch(
                            &current_provider_id,
                            &fb.provider_id,
                            &error_str,
                            request.trace_id.as_deref(),
                            fallback_hop,
                        );
                        current_provider_id = fb.provider_id.clone();
                        current_model_id = fb.model_id.clone();
                        continue;
                    }

                    // No more fallbacks
                    return Err(ArgusError::Llm(format!(
                        "All providers failed. Last error from '{}': {}",
                        current_provider_id, error_str
                    )));
                }
            }
        }
    }

    // ============================================================
    // Helpers
    // ============================================================

    /// Classify an error string into an `ApiErrorKind` for key rotation.
    fn classify_error(error: &str) -> ApiErrorKind {
        let lower = error.to_lowercase();
        if lower.contains("429") || lower.contains("rate limit") || lower.contains("rate_limit") {
            ApiErrorKind::RateLimited {
                retry_after_seconds: 60,
            }
        } else if lower.contains("401") || lower.contains("unauthorized") {
            ApiErrorKind::Unauthorized
        } else if lower.contains("402")
            || lower.contains("403")
            || lower.contains("quota")
            || lower.contains("insufficient")
            || lower.contains("billing")
        {
            ApiErrorKind::QuotaExhausted
        } else {
            ApiErrorKind::Transient
        }
    }

    /// Emit `LlmProviderSwitched` event.
    fn emit_provider_switch(
        &self,
        from: &str,
        to: &str,
        reason: &str,
        trace_id: Option<&str>,
        fallback_hop: u32,
    ) {
        tracing::info!(
            trace_id,
            from_provider = from,
            to_provider = to,
            reason = reason,
            fallback_hop,
            "Failing over to next provider"
        );
        self.event_bus.publish(
            EventSource::LlmProvider,
            ArgusEvent::LlmProviderSwitched {
                from_provider: from.to_string(),
                to_provider: to.to_string(),
                reason: reason.to_string(),
            },
        );
    }

    // ============================================================
    // Introspection
    // ============================================================

    /// Get the list of registered provider IDs in priority order.
    pub fn provider_ids(&self) -> &[String] {
        &self.priority_order
    }

    /// Get the number of registered providers.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Get the health state of a specific provider.
    pub async fn provider_health(&self, provider_id: &str) -> Option<ProviderHealth> {
        if let Some(rp) = self.providers.get(provider_id) {
            Some(rp.health.read().await.clone())
        } else {
            None
        }
    }

    /// Get the number of available keys for a provider.
    pub async fn provider_available_keys(&self, provider_id: &str) -> Option<usize> {
        if let Some(rp) = self.providers.get(provider_id) {
            Some(rp.key_rotator.available_count().await)
        } else {
            None
        }
    }

    /// Check which model classes are available across all providers.
    pub fn available_model_classes(&self) -> Vec<ModelClass> {
        let mut classes = Vec::new();
        for class in &[
            ModelClass::Reasoning,
            ModelClass::Coding,
            ModelClass::Fast,
            ModelClass::LongContext,
        ] {
            let has_any = self
                .providers
                .values()
                .any(|rp| rp.provider.is_enabled() && rp.provider.has_model_class(class));
            if has_any {
                classes.push(class.clone());
            }
        }
        classes
    }

    /// List all models across all providers.
    pub fn all_models(&self) -> Vec<(&str, &ModelInfo)> {
        let mut result = Vec::new();
        for (id, rp) in &self.providers {
            for model in rp.provider.models() {
                result.push((id.as_str(), model));
            }
        }
        result
    }

    /// Run health checks on all providers concurrently.
    pub async fn health_check_all(&self) {
        for (id, rp) in &self.providers {
            let is_healthy = rp.provider.health_check().await;
            if !is_healthy {
                tracing::warn!(provider = id, "Provider health check failed");
                let mut health = rp.health.write().await;
                let went_down = health.record_failure(CIRCUIT_BREAKER_THRESHOLD);
                if went_down {
                    self.event_bus.publish(
                        EventSource::LlmProvider,
                        ArgusEvent::LlmProviderDown {
                            provider: id.clone(),
                            error: "Health check failed".to_string(),
                        },
                    );
                }
            } else {
                let mut health = rp.health.write().await;
                let was_down = !health.is_healthy;
                health.record_success(0);
                if was_down {
                    self.event_bus.publish(
                        EventSource::LlmProvider,
                        ArgusEvent::LlmProviderRecovered {
                            provider: id.clone(),
                        },
                    );
                }
            }
        }
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::LlmProvider;
    use crate::types::{ChatMessage, StreamChunk, TokenUsage};
    use async_trait::async_trait;
    use futures::Stream;
    use std::pin::Pin;

    // ----- Mock provider for testing -----

    struct MockProvider {
        id: String,
        enabled: bool,
        models: Vec<ModelInfo>,
        /// If Some, complete() returns this error.
        fail_with: Option<String>,
    }

    impl MockProvider {
        fn new(id: &str, models: Vec<ModelInfo>) -> Self {
            Self {
                id: id.to_string(),
                enabled: true,
                models,
                fail_with: None,
            }
        }

        fn failing(id: &str, models: Vec<ModelInfo>, error: &str) -> Self {
            Self {
                id: id.to_string(),
                enabled: true,
                models,
                fail_with: Some(error.to_string()),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn id(&self) -> &str {
            &self.id
        }
        fn name(&self) -> &str {
            &self.id
        }
        fn is_enabled(&self) -> bool {
            self.enabled
        }
        fn models(&self) -> &[ModelInfo] {
            &self.models
        }

        async fn complete(
            &self,
            model_id: &str,
            _messages: &[ChatMessage],
            _max_tokens: Option<u32>,
            _temperature: Option<f32>,
            _api_key: &str,
        ) -> ArgusResult<LlmResponse> {
            if let Some(ref err) = self.fail_with {
                return Err(ArgusError::Llm(err.clone()));
            }
            Ok(LlmResponse {
                content: format!("Mock response from {}", self.id),
                provider: self.id.clone(),
                model: model_id.to_string(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                    total_tokens: 30,
                },
                finish_reason: Some("stop".to_string()),
                latency_ms: 100,
                cost_usd: 0.001,
            })
        }

        async fn complete_stream(
            &self,
            _model_id: &str,
            _messages: &[ChatMessage],
            _max_tokens: Option<u32>,
            _temperature: Option<f32>,
            _api_key: &str,
        ) -> ArgusResult<Pin<Box<dyn Stream<Item = ArgusResult<StreamChunk>> + Send>>> {
            Err(ArgusError::Llm(
                "Streaming not implemented in mock".to_string(),
            ))
        }
    }

    fn make_model(id: &str, class: ModelClass) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            class,
            cost_per_1k_input: Some(0.001),
            cost_per_1k_output: Some(0.002),
            max_context_tokens: Some(128000),
        }
    }

    fn make_event_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    fn make_budget(event_bus: Arc<EventBus>) -> Arc<BudgetTracker> {
        Arc::new(BudgetTracker::new(Some(100.0), Some(10.0), 0.8, event_bus))
    }

    fn make_request(class: ModelClass) -> LlmRequest {
        LlmRequest::new(class, vec![ChatMessage::user("Hello")], "test")
    }

    // ----- Tests -----

    #[tokio::test]
    async fn test_route_selects_highest_priority() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let mut router = ProviderRouter::new(budget, event_bus);

        // Register two providers: low priority first, high priority second
        router.register(
            Box::new(MockProvider::new(
                "low-priority",
                vec![make_model("model-lp", ModelClass::Coding)],
            )),
            50,
            vec!["key-lp".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );
        router.register(
            Box::new(MockProvider::new(
                "high-priority",
                vec![make_model("model-hp", ModelClass::Coding)],
            )),
            10,
            vec!["key-hp".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        let decision = router
            .route(&make_request(ModelClass::Coding))
            .await
            .unwrap();
        assert_eq!(decision.provider_id, "high-priority");
        assert_eq!(decision.model_id, "model-hp");
        assert_eq!(decision.fallbacks.len(), 1);
        assert_eq!(decision.fallbacks[0].provider_id, "low-priority");
    }

    #[tokio::test]
    async fn test_route_skips_provider_without_model_class() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let mut router = ProviderRouter::new(budget, event_bus);

        router.register(
            Box::new(MockProvider::new(
                "fast-only",
                vec![make_model("fast-model", ModelClass::Fast)],
            )),
            10,
            vec!["key".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );
        router.register(
            Box::new(MockProvider::new(
                "coding",
                vec![make_model("coding-model", ModelClass::Coding)],
            )),
            20,
            vec!["key".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        let decision = router
            .route(&make_request(ModelClass::Coding))
            .await
            .unwrap();
        assert_eq!(decision.provider_id, "coding");
    }

    #[tokio::test]
    async fn test_route_no_provider_available() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let router = ProviderRouter::new(budget, event_bus);

        let result = router.route(&make_request(ModelClass::Reasoning)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_chat_success() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let mut router = ProviderRouter::new(budget, event_bus);

        router.register(
            Box::new(MockProvider::new(
                "mock",
                vec![make_model("test-model", ModelClass::Coding)],
            )),
            10,
            vec!["key-a".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        let response = router
            .chat(&make_request(ModelClass::Coding))
            .await
            .unwrap();
        assert_eq!(response.provider, "mock");
        assert!(response.content.contains("Mock response"));
    }

    #[tokio::test]
    async fn test_chat_failover() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let mut router = ProviderRouter::new(budget, event_bus);

        // Primary fails
        router.register(
            Box::new(MockProvider::failing(
                "primary",
                vec![make_model("m1", ModelClass::Coding)],
                "Server error 500",
            )),
            10,
            vec!["key-a".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        // Fallback succeeds
        router.register(
            Box::new(MockProvider::new(
                "fallback",
                vec![make_model("m2", ModelClass::Coding)],
            )),
            20,
            vec!["key-b".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        let response = router
            .chat(&make_request(ModelClass::Coding))
            .await
            .unwrap();
        assert_eq!(response.provider, "fallback");
    }

    #[tokio::test]
    async fn test_chat_all_providers_fail() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let mut router = ProviderRouter::new(budget, event_bus);

        router.register(
            Box::new(MockProvider::failing(
                "p1",
                vec![make_model("m1", ModelClass::Coding)],
                "Error 1",
            )),
            10,
            vec!["key".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );
        router.register(
            Box::new(MockProvider::failing(
                "p2",
                vec![make_model("m2", ModelClass::Coding)],
                "Error 2",
            )),
            20,
            vec!["key".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        let result = router.chat(&make_request(ModelClass::Coding)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("All providers failed"));
    }

    #[tokio::test]
    async fn test_provider_count_and_ids() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let mut router = ProviderRouter::new(budget, event_bus);

        assert_eq!(router.provider_count(), 0);
        assert!(router.provider_ids().is_empty());

        router.register(
            Box::new(MockProvider::new("alpha", vec![])),
            20,
            vec![],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );
        router.register(
            Box::new(MockProvider::new("beta", vec![])),
            10,
            vec![],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        assert_eq!(router.provider_count(), 2);
        // beta has priority 10, alpha has 20 — beta should be first
        assert_eq!(router.provider_ids()[0], "beta");
        assert_eq!(router.provider_ids()[1], "alpha");
    }

    #[tokio::test]
    async fn test_provider_health_tracking() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let mut router = ProviderRouter::new(budget, event_bus);

        router.register(
            Box::new(MockProvider::new(
                "mock",
                vec![make_model("m", ModelClass::Coding)],
            )),
            10,
            vec!["key".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        let health = router.provider_health("mock").await.unwrap();
        assert!(health.is_healthy);
        assert_eq!(health.consecutive_failures, 0);

        // After a successful chat, health should still be good
        let _ = router.chat(&make_request(ModelClass::Coding)).await;
        let health = router.provider_health("mock").await.unwrap();
        assert!(health.is_healthy);
        assert_eq!(health.total_requests, 1);
    }

    #[tokio::test]
    async fn test_available_model_classes() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let mut router = ProviderRouter::new(budget, event_bus);

        router.register(
            Box::new(MockProvider::new(
                "p1",
                vec![
                    make_model("coding-m", ModelClass::Coding),
                    make_model("fast-m", ModelClass::Fast),
                ],
            )),
            10,
            vec!["key".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        let classes = router.available_model_classes();
        assert!(classes.contains(&ModelClass::Coding));
        assert!(classes.contains(&ModelClass::Fast));
        assert!(!classes.contains(&ModelClass::Reasoning));
    }

    #[tokio::test]
    async fn test_classify_error() {
        // Rate limited
        let kind = ProviderRouter::classify_error("API error 429: too many requests");
        assert!(matches!(kind, ApiErrorKind::RateLimited { .. }));

        // Unauthorized
        let kind = ProviderRouter::classify_error("API error 401: unauthorized");
        assert!(matches!(kind, ApiErrorKind::Unauthorized));

        // Quota
        let kind = ProviderRouter::classify_error("insufficient quota for this request");
        assert!(matches!(kind, ApiErrorKind::QuotaExhausted));

        // Transient
        let kind = ProviderRouter::classify_error("connection timed out");
        assert!(matches!(kind, ApiErrorKind::Transient));
    }

    #[tokio::test]
    async fn test_force_provider() {
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());
        let mut router = ProviderRouter::new(budget, event_bus);

        router.register(
            Box::new(MockProvider::new(
                "high-pri",
                vec![make_model("m1", ModelClass::Coding)],
            )),
            10,
            vec!["key".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );
        router.register(
            Box::new(MockProvider::new(
                "low-pri",
                vec![make_model("m2", ModelClass::Coding)],
            )),
            50,
            vec!["key".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        // Force the low-priority provider
        let mut request = make_request(ModelClass::Coding);
        request.force_provider = Some("low-pri".to_string());

        let response = router.chat(&request).await.unwrap();
        assert_eq!(response.provider, "low-pri");
    }

    #[tokio::test]
    async fn test_from_config() {
        let toml_str = r#"
[global]
default_model_class = "coding"
max_monthly_budget_usd = 100.0

[providers.test_ollama]
enabled = true
type = "ollama"
priority = 50
base_url = "http://localhost:11434"

[[providers.test_ollama.models]]
id = "qwen2.5:7b"
class = "fast"

[embedding]
type = "local"
model = "multilingual-e5-large"
dimension = 1024
"#;
        let config: ProvidersConfig = toml::from_str(toml_str).unwrap();
        let event_bus = make_event_bus();
        let budget = make_budget(event_bus.clone());

        let router = ProviderRouter::from_config(&config, budget, event_bus);
        assert_eq!(router.provider_count(), 1);
        assert_eq!(router.provider_ids()[0], "test_ollama");
    }

    #[tokio::test]
    async fn test_budget_blocks_provider() {
        let event_bus = make_event_bus();
        // Tight budget: $0.003 monthly — enough for one call's estimated cost (~$0.002)
        // but after recording the first call's actual cost ($0.001), the second call's
        // estimated cost (~$0.002) will push total over the limit.
        let budget = Arc::new(BudgetTracker::new(
            Some(0.003),
            None,
            0.8,
            event_bus.clone(),
        ));
        let mut router = ProviderRouter::new(budget.clone(), event_bus);

        router.register(
            Box::new(MockProvider::new(
                "expensive",
                vec![make_model("m", ModelClass::Coding)],
            )),
            10,
            vec!["key".to_string()],
            RateLimitConfig {
                rpm: None,
                tpm: None,
            },
        );

        // First call should work
        let r1 = router.chat(&make_request(ModelClass::Coding)).await;
        assert!(r1.is_ok());

        // Second call should fail (budget exceeded)
        let r2 = router.chat(&make_request(ModelClass::Coding)).await;
        assert!(r2.is_err());
    }
}
