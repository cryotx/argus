//! Built-in provider/model registry with config overrides.
//!
//! Providers and models are data, not if/else branches. Built-in defaults are
//! a starting point; they can be disabled, re-prioritized, or extended by
//! config without pretending permanent knowledge of vendor model catalogs.

use chrono::Utc;

use crate::intelligence::{CapabilityRequirement as C, ModelTier, ReasoningEffort};

use super::types::*;

/// A persisted/config override for one provider.
#[derive(Clone, Debug)]
pub struct ProviderOverride {
    pub provider_id: RuntimeProvider,
    pub enabled: Option<bool>,
    pub priority: Option<i32>,
    pub runtime_capabilities: Option<Vec<C>>,
}

pub struct ProviderRegistry {
    providers: Vec<ProviderDescriptor>,
    models: Vec<ModelDescriptor>,
}

impl ProviderRegistry {
    pub fn new_default() -> Self {
        Self {
            providers: builtin_providers(),
            models: builtin_models(),
        }
    }

    pub fn providers(&self) -> &[ProviderDescriptor] {
        &self.providers
    }

    pub fn models(&self) -> &[ModelDescriptor] {
        &self.models
    }

    pub fn provider(&self, id: RuntimeProvider) -> Option<&ProviderDescriptor> {
        self.providers.iter().find(|p| p.provider_id == id)
    }

    pub fn models_for(&self, id: RuntimeProvider) -> impl Iterator<Item = &ModelDescriptor> {
        self.models.iter().filter(move |m| m.provider_id == id)
    }

    /// Apply config overrides. Unknown providers are ignored (never invented).
    pub fn apply_overrides(&mut self, overrides: &[ProviderOverride]) {
        for o in overrides {
            if let Some(p) = self
                .providers
                .iter_mut()
                .find(|p| p.provider_id == o.provider_id)
            {
                if let Some(enabled) = o.enabled {
                    p.enabled = enabled;
                }
                if let Some(priority) = o.priority {
                    p.priority = priority;
                }
                if let Some(caps) = &o.runtime_capabilities {
                    p.runtime_capabilities = caps.clone();
                }
            }
        }
    }

    /// Merge additional model descriptors (discovered or configured). A model
    /// with the same id+provider replaces the previous one.
    pub fn upsert_models(&mut self, models: Vec<ModelDescriptor>) {
        for model in models {
            if let Some(existing) = self
                .models
                .iter_mut()
                .find(|m| m.model_id == model.model_id && m.provider_id == model.provider_id)
            {
                *existing = model;
            } else {
                self.models.push(model);
            }
        }
    }
}

fn caps(items: &[C]) -> Vec<C> {
    items.to_vec()
}

#[allow(clippy::too_many_arguments)]
fn model(
    provider: RuntimeProvider,
    model_id: &str,
    display_name: &str,
    tier: ModelTier,
    capabilities: Vec<C>,
    reasoning_levels: Vec<ReasoningEffort>,
    context_class: &str,
    supports_tools: bool,
) -> ModelDescriptor {
    ModelDescriptor {
        model_id: model_id.to_string(),
        provider_id: provider,
        display_name: display_name.to_string(),
        capability_tier: tier,
        capabilities,
        supported_roles: Vec::new(),
        supported_purposes: Vec::new(),
        reasoning_levels,
        context_class: context_class.to_string(),
        supports_tools,
        supports_code_execution: supports_tools,
        supports_structured_output: true,
        availability: true,
        metadata_revision: "builtin-1".to_string(),
        discovered_at: Utc::now(),
    }
}

const AGENT_CAPS: &[C] = &[
    C::CodeGeneration,
    C::StrongReasoning,
    C::RepositoryNavigation,
    C::MultiFileEditing,
    C::DatabaseReasoning,
    C::ArchitectureReasoning,
    C::SecurityReasoning,
    C::Debugging,
    C::TestGeneration,
    C::LongContext,
    C::ToolUse,
    C::HighReliability,
];

pub fn builtin_providers() -> Vec<ProviderDescriptor> {
    let cli = |id: RuntimeProvider, name: &str, priority: i32| ProviderDescriptor {
        provider_id: id,
        display_name: name.to_string(),
        kind: ProviderKind::Cli,
        billing_modes: vec![BillingMode::SubscriptionIncluded, BillingMode::ApiMetered],
        runtime_capabilities: caps(AGENT_CAPS),
        execution_mode: ExecutionMode::Automatic,
        enabled: true,
        priority,
        allowed_roles: Vec::new(),
    };
    let api = |id: RuntimeProvider, name: &str, priority: i32| ProviderDescriptor {
        provider_id: id,
        display_name: name.to_string(),
        kind: ProviderKind::Api,
        billing_modes: vec![BillingMode::ApiMetered],
        runtime_capabilities: caps(&[
            C::CodeGeneration,
            C::StrongReasoning,
            C::DatabaseReasoning,
            C::ArchitectureReasoning,
            C::SecurityReasoning,
            C::Debugging,
            C::TestGeneration,
            C::LongContext,
            C::HighReliability,
        ]),
        execution_mode: ExecutionMode::Automatic,
        enabled: true,
        priority,
        allowed_roles: Vec::new(),
    };
    vec![
        cli(RuntimeProvider::ClaudeCodeCli, "Claude Code CLI", 10),
        cli(RuntimeProvider::CodexCli, "Codex CLI", 11),
        cli(RuntimeProvider::OpenCodeCli, "OpenCode CLI", 12),
        api(RuntimeProvider::OpenAiApi, "OpenAI API", 20),
        api(RuntimeProvider::AnthropicApi, "Anthropic API", 21),
        ProviderDescriptor {
            provider_id: RuntimeProvider::ManualWeb,
            display_name: "Manual Web Planner".to_string(),
            kind: ProviderKind::Manual,
            billing_modes: vec![BillingMode::Unknown],
            runtime_capabilities: caps(&[
                C::StrongReasoning,
                C::ArchitectureReasoning,
                C::LongContext,
            ]),
            execution_mode: ExecutionMode::Manual,
            enabled: true,
            priority: 90,
            allowed_roles: Vec::new(),
        },
        ProviderDescriptor {
            provider_id: RuntimeProvider::Test,
            display_name: "Test Provider".to_string(),
            kind: ProviderKind::Test,
            billing_modes: vec![BillingMode::LocalCompute],
            runtime_capabilities: caps(AGENT_CAPS),
            execution_mode: ExecutionMode::Automatic,
            enabled: true,
            priority: 1000,
            allowed_roles: Vec::new(),
        },
    ]
}

pub fn builtin_models() -> Vec<ModelDescriptor> {
    let all_reasoning = vec![
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::VeryHigh,
    ];
    vec![
        model(
            RuntimeProvider::ClaudeCodeCli,
            "claude-code-default",
            "Claude Code (default)",
            ModelTier::T3,
            caps(AGENT_CAPS),
            all_reasoning.clone(),
            "long",
            true,
        ),
        model(
            RuntimeProvider::ClaudeCodeCli,
            "claude-code-fast",
            "Claude Code (fast)",
            ModelTier::T1,
            caps(&[C::CodeGeneration, C::Debugging, C::TestGeneration]),
            vec![ReasoningEffort::Low, ReasoningEffort::Medium],
            "medium",
            true,
        ),
        model(
            RuntimeProvider::CodexCli,
            "codex-default",
            "Codex (default)",
            ModelTier::T3,
            caps(AGENT_CAPS),
            all_reasoning.clone(),
            "long",
            true,
        ),
        model(
            RuntimeProvider::CodexCli,
            "codex-standard",
            "Codex (standard)",
            ModelTier::T2,
            caps(&[
                C::CodeGeneration,
                C::StrongReasoning,
                C::RepositoryNavigation,
                C::MultiFileEditing,
                C::Debugging,
                C::TestGeneration,
                C::ToolUse,
            ]),
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            "long",
            true,
        ),
        model(
            RuntimeProvider::OpenCodeCli,
            "opencode-default",
            "OpenCode (default)",
            ModelTier::T2,
            caps(&[
                C::CodeGeneration,
                C::StrongReasoning,
                C::RepositoryNavigation,
                C::MultiFileEditing,
                C::Debugging,
                C::TestGeneration,
                C::ToolUse,
            ]),
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            "long",
            true,
        ),
        model(
            RuntimeProvider::OpenAiApi,
            "openai-api-default",
            "OpenAI API (default)",
            ModelTier::T3,
            caps(&[
                C::CodeGeneration,
                C::StrongReasoning,
                C::DatabaseReasoning,
                C::ArchitectureReasoning,
                C::SecurityReasoning,
                C::TestGeneration,
                C::LongContext,
                C::HighReliability,
            ]),
            all_reasoning.clone(),
            "long",
            false,
        ),
        model(
            RuntimeProvider::OpenAiApi,
            "openai-api-fast",
            "OpenAI API (fast)",
            ModelTier::T1,
            caps(&[C::CodeGeneration, C::StrongReasoning, C::TestGeneration]),
            vec![ReasoningEffort::Low, ReasoningEffort::Medium],
            "medium",
            false,
        ),
        model(
            RuntimeProvider::AnthropicApi,
            "anthropic-api-default",
            "Anthropic API (default)",
            ModelTier::T3,
            caps(&[
                C::CodeGeneration,
                C::StrongReasoning,
                C::DatabaseReasoning,
                C::ArchitectureReasoning,
                C::SecurityReasoning,
                C::TestGeneration,
                C::LongContext,
                C::HighReliability,
            ]),
            all_reasoning.clone(),
            "long",
            false,
        ),
        model(
            RuntimeProvider::ManualWeb,
            "manual-web-default",
            "Manual Web",
            ModelTier::T3,
            caps(&[C::StrongReasoning, C::ArchitectureReasoning, C::LongContext]),
            all_reasoning,
            "very_long",
            false,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_runtimes_have_agent_capabilities() {
        let r = ProviderRegistry::new_default();
        let claude = r.provider(RuntimeProvider::ClaudeCodeCli).unwrap();
        assert!(
            claude
                .runtime_capabilities
                .contains(&C::RepositoryNavigation)
        );
        let api = r.provider(RuntimeProvider::OpenAiApi).unwrap();
        assert!(!api.runtime_capabilities.contains(&C::ToolUse));
    }

    #[test]
    fn overrides_never_invent_providers() {
        let mut r = ProviderRegistry::new_default();
        r.apply_overrides(&[ProviderOverride {
            provider_id: RuntimeProvider::Test,
            enabled: Some(false),
            priority: None,
            runtime_capabilities: None,
        }]);
        assert!(!r.provider(RuntimeProvider::Test).unwrap().enabled);
    }
}
