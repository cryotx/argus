//! Core types for the LLM Provider Engine.
//!
//! Defines the message protocol, model classification, request/response
//! structures, and usage tracking types used across all providers.

use std::fmt;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ============================================================
// Model classification
// ============================================================

/// Model classes group models by capability, not by provider.
///
/// The router uses this to select which provider/model to use for a given task.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelClass {
    /// Complex reasoning: architecture decisions, debugging, root cause analysis.
    /// Models: o1, o3-mini, claude-opus, deepseek-r1, gemini-pro.
    Reasoning,

    /// Code generation and modification: the primary workhorse.
    /// Models: claude-sonnet, gpt-4o, deepseek-chat, qwen-max, glm-4-plus.
    Coding,

    /// Simple tasks: intent classification, formatting, simple edits.
    /// Models: gpt-4o-mini, claude-haiku, qwen-turbo, glm-4-flash.
    Fast,

    /// Long documents: summarize large codebases, analyze long logs.
    /// Models: moonshot-128k, gemini-pro (1M context).
    LongContext,

    /// Generate vector embeddings for memory system.
    /// Fixed model, not affected by routing decisions.
    Embedding,
}

impl fmt::Display for ModelClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Reasoning => "reasoning",
            Self::Coding => "coding",
            Self::Fast => "fast",
            Self::LongContext => "long_context",
            Self::Embedding => "embedding",
        };
        write!(f, "{s}")
    }
}

impl ModelClass {
    /// Parse a model class from a string (case-insensitive).
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "reasoning" => Some(Self::Reasoning),
            "coding" => Some(Self::Coding),
            "fast" => Some(Self::Fast),
            "long_context" | "longcontext" => Some(Self::LongContext),
            "embedding" => Some(Self::Embedding),
            _ => None,
        }
    }
}

// ============================================================
// Chat message protocol
// ============================================================

/// Role in a chat conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

impl fmt::Display for ChatRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        };
        write!(f, "{s}")
    }
}

/// A single message in a chat conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Optional name (for multi-user or tool-call scenarios).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool call ID (for tool responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::System,
            content: content.into(),
            name: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
            name: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
            name: None,
            tool_call_id: None,
        }
    }
}

// ============================================================
// LLM request / response
// ============================================================

/// A request to an LLM provider.
#[derive(Clone, Debug)]
pub struct LlmRequest {
    /// Which class of model to use.
    pub model_class: ModelClass,
    /// The conversation messages.
    pub messages: Vec<ChatMessage>,
    /// Maximum tokens to generate.
    pub max_tokens: Option<u32>,
    /// Temperature (0.0 = deterministic, 1.0+ = creative).
    pub temperature: Option<f32>,
    /// Top-p nucleus sampling.
    pub top_p: Option<f32>,
    /// Stop sequences.
    pub stop: Option<Vec<String>>,
    /// Purpose of this request (for logging/budget tracking).
    pub purpose: String,
    /// Optional trace id propagated from request boundaries.
    pub trace_id: Option<String>,
    /// Optional override: force a specific provider.
    pub force_provider: Option<String>,
    /// Optional override: force a specific model.
    pub force_model: Option<String>,
    /// Optional project association (for budget tracking).
    pub project_id: Option<Uuid>,
    /// Optional task association (for budget tracking).
    pub task_id: Option<Uuid>,
}

impl LlmRequest {
    /// Create a simple request with a model class and messages.
    pub fn new(
        model_class: ModelClass,
        messages: Vec<ChatMessage>,
        purpose: impl Into<String>,
    ) -> Self {
        Self {
            model_class,
            messages,
            max_tokens: None,
            temperature: None,
            top_p: None,
            stop: None,
            purpose: purpose.into(),
            trace_id: None,
            force_provider: None,
            force_model: None,
            project_id: None,
            task_id: None,
        }
    }

    /// Attach a trace id to this request.
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        let id = trace_id.into();
        if !id.trim().is_empty() {
            self.trace_id = Some(id);
        }
        self
    }

    /// Estimate input tokens (rough: ~4 chars per token).
    pub fn estimated_input_tokens(&self) -> u32 {
        let total_chars: usize = self.messages.iter().map(|m| m.content.len()).sum();
        std::cmp::max(1, (total_chars / 4) as u32)
    }
}

/// A response from an LLM provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmResponse {
    /// The generated content.
    pub content: String,
    /// Which provider fulfilled the request.
    pub provider: String,
    /// Which model was used.
    pub model: String,
    /// Token usage.
    pub usage: TokenUsage,
    /// Finish reason (stop, length, etc.).
    pub finish_reason: Option<String>,
    /// Request latency in milliseconds.
    pub latency_ms: u64,
    /// Estimated cost in USD.
    pub cost_usd: f64,
}

/// Token usage from an LLM response.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

/// A streaming chunk from an LLM provider.
#[derive(Clone, Debug)]
pub struct StreamChunk {
    /// The delta content in this chunk.
    pub delta: String,
    /// Whether this is the final chunk.
    pub is_final: bool,
    /// Finish reason (only on final chunk).
    pub finish_reason: Option<String>,
    /// Usage info (only on final chunk, if provider supports it).
    pub usage: Option<TokenUsage>,
}

// ============================================================
// Routing decision
// ============================================================

/// The result of the provider router selecting a provider + model.
#[derive(Clone, Debug)]
pub struct RoutingDecision {
    /// Selected provider ID.
    pub provider_id: String,
    /// Selected model ID.
    pub model_id: String,
    /// The API key to use (index into the provider's key rotator).
    pub key_index: usize,
    /// Estimated cost for this request (USD).
    pub estimated_cost: f64,
    /// Ordered fallback providers if the primary fails.
    pub fallbacks: Vec<FallbackOption>,
}

/// A fallback option in the routing chain.
#[derive(Clone, Debug)]
pub struct FallbackOption {
    pub provider_id: String,
    pub model_id: String,
}

// ============================================================
// Usage / budget tracking types
// ============================================================

/// A completed usage record to feed into the budget tracker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageRecord {
    pub provider: String,
    pub model: String,
    pub model_class: ModelClass,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub timestamp: DateTime<Utc>,
    pub latency_ms: u64,
    pub purpose: String,
}

/// Budget check result.
#[derive(Clone, Debug)]
pub enum BudgetCheck {
    /// The request is within all applicable budgets.
    WithinBudget,
    /// The request would exceed a budget.
    OverBudget {
        scope: BudgetScope,
        remaining_usd: f64,
    },
}

/// Which budget scope was exceeded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetScope {
    GlobalMonthly,
    GlobalDaily,
    ProviderMonthly,
    ProjectDaily,
    TaskTotal,
}

impl fmt::Display for BudgetScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::GlobalMonthly => "global_monthly",
            Self::GlobalDaily => "global_daily",
            Self::ProviderMonthly => "provider_monthly",
            Self::ProjectDaily => "project_daily",
            Self::TaskTotal => "task_total",
        };
        write!(f, "{s}")
    }
}

/// Internal budget state for a single scope.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BudgetState {
    /// Monthly spending limit in USD (None = unlimited).
    pub monthly_limit_usd: Option<f64>,
    /// Daily spending limit in USD (None = unlimited).
    pub daily_limit_usd: Option<f64>,
    /// How much has been spent this month.
    pub month_spent_usd: f64,
    /// How much has been spent today.
    pub today_spent_usd: f64,
    /// Total input tokens consumed.
    pub total_input_tokens: u64,
    /// Total output tokens consumed.
    pub total_output_tokens: u64,
    /// Total requests made.
    pub request_count: u64,
    /// When the daily counter was last reset.
    pub last_daily_reset: NaiveDate,
    /// When the monthly counter was last reset.
    pub last_monthly_reset: NaiveDate,
}

impl BudgetState {
    pub fn new(monthly_limit: Option<f64>, daily_limit: Option<f64>) -> Self {
        let today = Utc::now().date_naive();
        Self {
            monthly_limit_usd: monthly_limit,
            daily_limit_usd: daily_limit,
            month_spent_usd: 0.0,
            today_spent_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            request_count: 0,
            last_daily_reset: today,
            last_monthly_reset: today,
        }
    }

    /// Reset daily counters if the date has changed.
    pub fn maybe_reset_daily(&mut self) {
        let today = Utc::now().date_naive();
        if today > self.last_daily_reset {
            self.today_spent_usd = 0.0;
            self.last_daily_reset = today;
        }
    }

    /// Reset monthly counters if the month has changed.
    pub fn maybe_reset_monthly(&mut self) {
        let today = Utc::now().date_naive();
        if today.month() != self.last_monthly_reset.month()
            || today.year() != self.last_monthly_reset.year()
        {
            self.month_spent_usd = 0.0;
            self.last_monthly_reset = today;
        }
    }

    /// Record spending.
    pub fn record(&mut self, cost_usd: f64, input_tokens: u32, output_tokens: u32) {
        self.maybe_reset_daily();
        self.maybe_reset_monthly();

        self.month_spent_usd += cost_usd;
        self.today_spent_usd += cost_usd;
        self.total_input_tokens += input_tokens as u64;
        self.total_output_tokens += output_tokens as u64;
        self.request_count += 1;
    }
}

// ============================================================
// Model info
// ============================================================

/// Information about a specific model within a provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model ID as used in API calls (e.g., "gpt-4o", "claude-sonnet-4-20250514").
    pub id: String,
    /// Which class this model belongs to.
    pub class: ModelClass,
    /// Cost per 1K input tokens in USD (None for free models).
    pub cost_per_1k_input: Option<f64>,
    /// Cost per 1K output tokens in USD (None for free models).
    pub cost_per_1k_output: Option<f64>,
    /// Maximum context window size in tokens (if known).
    pub max_context_tokens: Option<u32>,
}

impl ModelInfo {
    /// Estimate the cost of a request with this model.
    pub fn estimate_cost(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        let input_cost = self
            .cost_per_1k_input
            .map(|c| c * input_tokens as f64 / 1000.0)
            .unwrap_or(0.0);
        let output_cost = self
            .cost_per_1k_output
            .map(|c| c * output_tokens as f64 / 1000.0)
            .unwrap_or(0.0);
        input_cost + output_cost
    }
}

// ============================================================
// Provider health
// ============================================================

/// Provider health status for circuit breaker decisions.
#[derive(Clone, Debug)]
pub struct ProviderHealth {
    /// Is this provider currently considered healthy?
    pub is_healthy: bool,
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
    /// When the provider was last marked as down.
    pub down_since: Option<DateTime<Utc>>,
    /// When the circuit breaker should allow a retry.
    pub retry_after: Option<DateTime<Utc>>,
    /// Total requests made.
    pub total_requests: u64,
    /// Total failures.
    pub total_failures: u64,
    /// Average latency in ms (rolling window).
    pub avg_latency_ms: f64,
}

impl Default for ProviderHealth {
    fn default() -> Self {
        Self {
            is_healthy: true,
            consecutive_failures: 0,
            down_since: None,
            retry_after: None,
            total_requests: 0,
            total_failures: 0,
            avg_latency_ms: 0.0,
        }
    }
}

impl ProviderHealth {
    /// Record a successful request.
    pub fn record_success(&mut self, latency_ms: u64) {
        self.is_healthy = true;
        self.consecutive_failures = 0;
        self.down_since = None;
        self.retry_after = None;
        self.total_requests += 1;

        // Exponential moving average for latency
        let alpha = 0.1;
        self.avg_latency_ms = self.avg_latency_ms * (1.0 - alpha) + latency_ms as f64 * alpha;
    }

    /// Record a failed request. Returns true if the provider should be marked as down.
    pub fn record_failure(&mut self, circuit_breaker_threshold: u32) -> bool {
        self.consecutive_failures += 1;
        self.total_requests += 1;
        self.total_failures += 1;

        if self.consecutive_failures >= circuit_breaker_threshold {
            self.is_healthy = false;
            self.down_since = Some(Utc::now());
            // Exponential backoff: 5min * 2^(failures - threshold)
            let backoff_minutes =
                5i64 * 2i64.pow((self.consecutive_failures - circuit_breaker_threshold).min(5));
            self.retry_after = Some(Utc::now() + chrono::Duration::minutes(backoff_minutes));
            true
        } else {
            false
        }
    }

    /// Check if the provider can be retried (circuit breaker half-open).
    pub fn can_retry(&self) -> bool {
        if self.is_healthy {
            return true;
        }
        match self.retry_after {
            Some(retry_at) => Utc::now() >= retry_at,
            None => true,
        }
    }
}

// ============================================================
// Rate limiting
// ============================================================

/// Rate limit configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Requests per minute.
    pub rpm: Option<u32>,
    /// Tokens per minute.
    pub tpm: Option<u32>,
}

/// Rate limit state tracker (sliding window).
#[derive(Clone, Debug)]
pub struct RateLimitState {
    pub config: RateLimitConfig,
    /// Timestamps of requests in the current minute window.
    request_timestamps: Vec<DateTime<Utc>>,
    /// Token counts in the current minute window.
    token_counts: Vec<(DateTime<Utc>, u32)>,
}

impl RateLimitState {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            request_timestamps: Vec::new(),
            token_counts: Vec::new(),
        }
    }

    /// Prune entries older than 1 minute.
    fn prune(&mut self) {
        let cutoff = Utc::now() - chrono::Duration::seconds(60);
        self.request_timestamps.retain(|t| *t > cutoff);
        self.token_counts.retain(|(t, _)| *t > cutoff);
    }

    /// Check if a new request is within rate limits.
    pub fn check(&mut self) -> bool {
        self.prune();

        if let Some(rpm) = self.config.rpm {
            if self.request_timestamps.len() >= rpm as usize {
                return false;
            }
        }

        if let Some(tpm) = self.config.tpm {
            let total_tokens: u32 = self.token_counts.iter().map(|(_, t)| *t).sum();
            if total_tokens >= tpm {
                return false;
            }
        }

        true
    }

    /// Record a request.
    pub fn record(&mut self, tokens: u32) {
        let now = Utc::now();
        self.request_timestamps.push(now);
        self.token_counts.push((now, tokens));
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_class_display() {
        assert_eq!(ModelClass::Reasoning.to_string(), "reasoning");
        assert_eq!(ModelClass::Coding.to_string(), "coding");
        assert_eq!(ModelClass::Fast.to_string(), "fast");
        assert_eq!(ModelClass::LongContext.to_string(), "long_context");
        assert_eq!(ModelClass::Embedding.to_string(), "embedding");
    }

    #[test]
    fn test_model_class_from_str() {
        assert_eq!(
            ModelClass::from_str_loose("reasoning"),
            Some(ModelClass::Reasoning)
        );
        assert_eq!(
            ModelClass::from_str_loose("CODING"),
            Some(ModelClass::Coding)
        );
        assert_eq!(ModelClass::from_str_loose("Fast"), Some(ModelClass::Fast));
        assert_eq!(
            ModelClass::from_str_loose("long_context"),
            Some(ModelClass::LongContext)
        );
        assert_eq!(ModelClass::from_str_loose("unknown"), None);
    }

    #[test]
    fn test_chat_message_constructors() {
        let sys = ChatMessage::system("You are helpful");
        assert_eq!(sys.role, ChatRole::System);
        assert_eq!(sys.content, "You are helpful");

        let user = ChatMessage::user("Hello");
        assert_eq!(user.role, ChatRole::User);

        let asst = ChatMessage::assistant("Hi there");
        assert_eq!(asst.role, ChatRole::Assistant);
    }

    #[test]
    fn test_estimated_input_tokens() {
        let req = LlmRequest::new(
            ModelClass::Fast,
            vec![ChatMessage::user("Hello world, this is a test message")],
            "test",
        );
        let tokens = req.estimated_input_tokens();
        // "Hello world, this is a test message" = 35 chars / 4 ≈ 8
        assert!(tokens > 0);
        assert!(tokens < 100);
    }

    #[test]
    fn test_model_info_cost_estimation() {
        let model = ModelInfo {
            id: "gpt-4o".to_string(),
            class: ModelClass::Coding,
            cost_per_1k_input: Some(0.0025),
            cost_per_1k_output: Some(0.01),
            max_context_tokens: Some(128000),
        };

        let cost = model.estimate_cost(1000, 500);
        // 1000 input tokens * 0.0025/1000 + 500 output tokens * 0.01/1000
        // = 0.0025 + 0.005 = 0.0075
        assert!((cost - 0.0075).abs() < 0.0001);
    }

    #[test]
    fn test_model_info_free_model() {
        let model = ModelInfo {
            id: "qwen2.5:7b".to_string(),
            class: ModelClass::Fast,
            cost_per_1k_input: None,
            cost_per_1k_output: None,
            max_context_tokens: None,
        };

        assert_eq!(model.estimate_cost(1000, 1000), 0.0);
    }

    #[test]
    fn test_budget_state_recording() {
        let mut state = BudgetState::new(Some(100.0), Some(10.0));
        state.record(0.5, 1000, 500);

        assert_eq!(state.request_count, 1);
        assert!((state.month_spent_usd - 0.5).abs() < 0.001);
        assert!((state.today_spent_usd - 0.5).abs() < 0.001);
        assert_eq!(state.total_input_tokens, 1000);
        assert_eq!(state.total_output_tokens, 500);
    }

    #[test]
    fn test_provider_health_success() {
        let mut health = ProviderHealth::default();
        health.record_success(100);

        assert!(health.is_healthy);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.total_requests, 1);
        assert!(health.avg_latency_ms > 0.0);
    }

    #[test]
    fn test_provider_health_circuit_breaker() {
        let mut health = ProviderHealth::default();

        // 3 failures should trigger circuit breaker
        assert!(!health.record_failure(3));
        assert!(!health.record_failure(3));
        assert!(health.record_failure(3)); // 3rd failure trips the breaker

        assert!(!health.is_healthy);
        assert!(health.down_since.is_some());
        assert!(health.retry_after.is_some());

        // can_retry should be false (retry_after is in the future)
        assert!(!health.can_retry());
    }

    #[test]
    fn test_provider_health_recovery() {
        let mut health = ProviderHealth::default();
        health.record_failure(3);
        health.record_failure(3);
        health.record_failure(3);
        assert!(!health.is_healthy);

        // A success recovers the provider
        health.record_success(100);
        assert!(health.is_healthy);
        assert_eq!(health.consecutive_failures, 0);
    }

    #[test]
    fn test_rate_limit_check() {
        let config = RateLimitConfig {
            rpm: Some(2),
            tpm: None,
        };
        let mut state = RateLimitState::new(config);

        assert!(state.check());
        state.record(100);
        assert!(state.check());
        state.record(100);
        // Now at 2 requests in the window — should be rate limited
        assert!(!state.check());
    }

    #[test]
    fn test_rate_limit_token_based() {
        let config = RateLimitConfig {
            rpm: None,
            tpm: Some(1000),
        };
        let mut state = RateLimitState::new(config);

        assert!(state.check());
        state.record(500);
        assert!(state.check());
        state.record(500);
        // Now at 1000 tokens — should be rate limited
        assert!(!state.check());
    }
}
