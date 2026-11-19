//! Brain Engine — Central intelligence of Argus.
//!
//! Coordinates intent classification, goal decomposition, complexity assessment,
//! decision making, and quality checks.

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::complexity::{ComplexityAssessment, ComplexityAssessor, ProjectContext};
use crate::decision::{DecisionContext, DecisionEngine, DecisionLevel, OwnerProfile};
use crate::decomposer::{DecompositionContext, DecompositionResult, FirstPrinciplesDecomposer};
use crate::intent::{ClassificationResult, Intent};
use crate::llm_client::{ChatMessage, LlmClient, LlmRequest};
use crate::quality::{QualityChecker, QualityConfig, QualityReport};

/// Context for intent classification.
#[derive(Clone, Debug, Default)]
pub struct IntentContext {
    /// Current project context (if any).
    pub project_id: Option<uuid::Uuid>,
    /// Recent conversation history.
    pub recent_messages: Vec<RecentMessage>,
    /// User's expertise level.
    pub user_expertise: UserExpertise,
}

#[derive(Clone, Debug, Default)]
pub enum UserExpertise {
    #[default]
    Beginner,
    Intermediate,
    Expert,
}

#[derive(Clone, Debug, Default)]
pub struct RecentMessage {
    pub role: String,
    pub content: String,
}

/// Central intelligence engine that coordinates all brain functions.
#[derive(Clone)]
pub struct BrainEngine {
    decomposer: FirstPrinciplesDecomposer,
    assessor: ComplexityAssessor,
    decision_engine: DecisionEngine,
    quality_checker: QualityChecker,
    _owner: OwnerProfile,
    llm: Option<Arc<dyn LlmClient>>,
}

impl Default for BrainEngine {
    fn default() -> Self {
        Self::new(OwnerProfile::default())
    }
}

impl BrainEngine {
    /// Create a new brain engine without LLM (for testing).
    pub fn new(owner: OwnerProfile) -> Self {
        Self {
            decomposer: FirstPrinciplesDecomposer::new(),
            assessor: ComplexityAssessor::new(),
            decision_engine: DecisionEngine::new(owner.clone()),
            quality_checker: QualityChecker::new(QualityConfig::default()),
            _owner: owner,
            llm: None,
        }
    }

    /// Create a new brain engine with LLM client.
    pub fn with_llm(owner: OwnerProfile, llm: Arc<dyn LlmClient>) -> Self {
        Self {
            decomposer: FirstPrinciplesDecomposer::new(),
            assessor: ComplexityAssessor::new(),
            decision_engine: DecisionEngine::new(owner.clone()),
            quality_checker: QualityChecker::new(QualityConfig::default()),
            _owner: owner,
            llm: Some(llm),
        }
    }

    /// Process an incoming message and determine intent.
    ///
    /// Uses LLM if available, otherwise falls back to keyword matching.
    pub async fn classify_intent(
        &self,
        message: &str,
        context: &IntentContext,
    ) -> ClassificationResult {
        info!(message = message, "Classifying intent");

        // Try LLM classification first if available
        if let Some(llm) = &self.llm {
            match self
                .classify_intent_with_llm(llm.as_ref(), message, context)
                .await
            {
                Ok(result) => return result,
                Err(e) => {
                    warn!(error = %e, "LLM classification failed, falling back to keyword matching");
                }
            }
        }

        // Fallback to keyword matching
        self.classify_intent_fallback(message)
    }

    /// Classify intent using LLM.
    async fn classify_intent_with_llm(
        &self,
        llm: &dyn LlmClient,
        message: &str,
        context: &IntentContext,
    ) -> Result<ClassificationResult, anyhow::Error> {
        let system_prompt = r#"You are an intent classifier for an AI developer assistant.
Classify the user's message into one of these intents:
- greeting: User says hello or initiates casual conversation
- help: User asks for help or clarification
- query_status: User asks about project/task status
- create_task: User wants to create a new task
- create_project: User wants to create a new project
- execute_task: User wants to execute/run a task
- modify_task: User wants to modify an existing task
- query_memory: User asks about past conversations or memories
- unclear: Cannot determine intent

Respond with ONLY the intent name and a confidence score (0.0-1.0) in this format:
intent: <intent_name>
confidence: <score>
reason: <brief explanation>"#;

        let user_message = format!(
            "Recent context: {}",
            context
                .recent_messages
                .last()
                .map(|m| m.content.clone())
                .unwrap_or_else(|| "none".to_string())
        );

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(&user_message),
            ChatMessage::user(message),
        ];

        let mut request = LlmRequest::new(messages, "intent_classification");
        request.max_tokens = Some(200);
        request.temperature = Some(0.3);

        let response = llm.chat(&request).await?;
        let content = response.content.to_lowercase();

        // Parse the response
        let intent = if content.contains("greeting") {
            Intent::Greeting
        } else if content.contains("help") {
            Intent::Help
        } else if content.contains("query_status") {
            Intent::QueryStatus {
                scope: crate::intent::QueryScope::AllProjects,
            }
        } else if content.contains("create_task")
            || content.contains("create task")
            || content.contains("new task")
        {
            Intent::SuggestFeature {
                project_hint: String::new(),
                feature: message.to_string(),
            }
        } else if content.contains("create_project")
            || content.contains("create project")
            || content.contains("new project")
        {
            Intent::CreateProject {
                description: message.to_string(),
            }
        } else if content.contains("execute_task")
            || content.contains("execute")
            || content.contains("run")
        {
            Intent::RunCommand {
                target: String::new(),
                command: message.to_string(),
            }
        } else if content.contains("query_memory") || content.contains("memory") {
            Intent::QueryMemory {
                query: message.to_string(),
            }
        } else {
            Intent::Unclear {
                original: message.to_string(),
            }
        };

        // Extract confidence from response
        let confidence = if content.contains("confidence:") {
            content
                .split("confidence:")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<f32>().ok())
                .map(crate::intent::Confidence::from_raw)
                .unwrap_or(crate::intent::Confidence::Medium)
        } else {
            crate::intent::Confidence::Medium
        };

        Ok(ClassificationResult {
            intent,
            confidence,
            raw_confidence: confidence.to_raw(),
        })
    }

    /// Fallback intent classification using keywords.
    fn classify_intent_fallback(&self, message: &str) -> ClassificationResult {
        let msg_lower = message.to_lowercase();

        let intent = if msg_lower.contains("hello")
            || msg_lower.contains("你好")
            || msg_lower.contains("hi")
        {
            Intent::Greeting
        } else if msg_lower.contains("help") || msg_lower.contains("帮助") {
            Intent::Help
        } else if msg_lower.contains("status") || msg_lower.contains("状态") {
            Intent::QueryStatus {
                scope: crate::intent::QueryScope::AllProjects,
            }
        } else if msg_lower.contains("create task")
            || msg_lower.contains("创建任务")
            || msg_lower.contains("新任务")
        {
            Intent::SuggestFeature {
                project_hint: String::new(),
                feature: message.to_string(),
            }
        } else if msg_lower.contains("create project")
            || msg_lower.contains("创建项目")
            || msg_lower.contains("新项目")
        {
            Intent::CreateProject {
                description: message.to_string(),
            }
        } else if msg_lower.contains("run")
            || msg_lower.contains("execute")
            || msg_lower.contains("执行")
        {
            Intent::RunCommand {
                target: String::new(),
                command: message.to_string(),
            }
        } else if msg_lower.contains("memory")
            || msg_lower.contains("记忆")
            || msg_lower.contains("之前")
        {
            Intent::QueryMemory {
                query: message.to_string(),
            }
        } else {
            Intent::Unclear {
                original: message.to_string(),
            }
        };

        ClassificationResult {
            intent,
            confidence: crate::intent::Confidence::Medium,
            raw_confidence: 0.75,
        }
    }

    /// Decompose a goal into a plan.
    pub async fn decompose(
        &self,
        goal: &str,
        context: &DecompositionContext,
    ) -> DecompositionResult {
        info!(goal = goal, "Decomposing goal");

        let result = self.decomposer.decompose(goal, context).await;

        // Validate the decomposition
        if let Err(errors) = result.validate_dependencies() {
            warn!(errors = ?errors, "Decomposition has dependency issues");
        }

        result
    }

    /// Assess complexity of a task.
    pub fn assess_complexity(
        &self,
        task: &crate::decomposer::AtomicTask,
        project_context: &ProjectContext,
    ) -> ComplexityAssessment {
        debug!(task_id = task.id, "Assessing complexity");

        self.assessor.assess(task, project_context)
    }

    /// Make a decision based on context.
    pub async fn decide(
        &self,
        action: &crate::decision::Action,
        context: &DecisionContext,
    ) -> DecisionLevel {
        info!(action = ?action, "Making decision");

        self.decision_engine.decide(action, context)
    }

    /// Check quality of an output.
    pub async fn check_quality(&self, task_output: &crate::quality::TaskOutput) -> QualityReport {
        debug!(task_id = task_output.task_id, "Checking quality");

        self.quality_checker.check(task_output).await
    }

    /// Get the decision engine for external access.
    pub fn decision_engine(&self) -> &DecisionEngine {
        &self.decision_engine
    }

    /// Get the decomposer for external access.
    pub fn decomposer(&self) -> &FirstPrinciplesDecomposer {
        &self.decomposer
    }
}
