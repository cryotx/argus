//! First-Principles Decomposer — breaks down project goals into executable plans.
//!
//! Uses first-principles thinking to decompose complex goals into milestones and atomic tasks.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::worker_types::WorkerType;

/// Technology stack decisions for a project.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TechDecisions {
    pub frontend: Option<String>,
    pub backend: Option<String>,
    pub database: Option<String>,
    pub caching: Option<String>,
    pub deployment: Option<String>,
    pub third_party: Vec<String>,
}

/// Acceptance criteria for a task — how to verify it's done.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AcceptanceCriteria {
    /// Human-readable description of what "done" means.
    pub description: String,
    /// Commands to run to verify completion (test commands, build commands, etc.).
    pub test_commands: Vec<String>,
    /// Expected output patterns or file paths that should exist.
    pub artifacts: Vec<String>,
}

/// A single atomic task that can be completed by a worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AtomicTask {
    /// Unique task ID (e.g., "T1.1", "T1.2").
    pub id: String,
    /// Short task title.
    pub title: String,
    /// Detailed description of what needs to be done.
    pub description: String,
    /// How to verify this task is complete.
    pub acceptance: AcceptanceCriteria,
    /// IDs of tasks that must complete before this one.
    pub depends_on: Vec<String>,
    /// Recommended worker type for this task.
    pub worker_type: WorkerType,
    /// Estimated duration in minutes.
    pub estimated_minutes: u32,
    /// Additional metadata (file paths, templates, etc.).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// A milestone groups related tasks together.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Milestone {
    /// Unique milestone ID (e.g., "M1", "M2").
    pub id: String,
    /// Milestone name.
    pub name: String,
    /// What this milestone achieves.
    pub description: String,
    /// Tasks belonging to this milestone.
    pub tasks: Vec<AtomicTask>,
    /// Estimated start after completing these milestone IDs.
    pub starts_after: Vec<String>,
}

/// Full decomposition result — the complete plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecompositionResult {
    /// Refined and clarified project goal.
    pub refined_goal: String,
    /// Constraints and assumptions identified during decomposition.
    pub constraints: Vec<String>,
    /// Technology stack decisions.
    pub tech_decisions: TechDecisions,
    /// Ordered milestones with tasks.
    pub milestones: Vec<Milestone>,
    /// Total estimated minutes (sum of all tasks).
    pub total_estimated_minutes: u32,
}

impl DecompositionResult {
    /// Create an empty decomposition result.
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            refined_goal: goal.into(),
            constraints: Vec::new(),
            tech_decisions: TechDecisions::default(),
            milestones: Vec::new(),
            total_estimated_minutes: 0,
        }
    }

    /// Add a milestone to the result.
    pub fn add_milestone(&mut self, milestone: Milestone) {
        self.total_estimated_minutes += milestone
            .tasks
            .iter()
            .map(|t| t.estimated_minutes)
            .sum::<u32>();
        self.milestones.push(milestone);
    }

    /// Get all tasks across all milestones.
    pub fn all_tasks(&self) -> Vec<&AtomicTask> {
        self.milestones.iter().flat_map(|m| &m.tasks).collect()
    }

    /// Get a task by ID.
    pub fn get_task(&self, task_id: &str) -> Option<&AtomicTask> {
        self.all_tasks().into_iter().find(|t| t.id == task_id)
    }

    /// Check for circular dependencies in the task graph.
    pub fn validate_dependencies(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        let task_ids: std::collections::HashSet<_> =
            self.all_tasks().iter().map(|t| &t.id).collect();

        for task in self.all_tasks() {
            for dep in &task.depends_on {
                if !task_ids.contains(dep) {
                    errors.push(format!(
                        "Task {} depends on non-existent task {}",
                        task.id, dep
                    ));
                }
            }
        }

        // Check for circular dependencies using DFS
        fn has_cycle(
            task_id: &str,
            visited: &mut std::collections::HashSet<String>,
            path: &mut std::collections::HashSet<String>,
            all_tasks: &std::collections::HashMap<String, &AtomicTask>,
        ) -> bool {
            if path.contains(task_id) {
                return true;
            }
            if visited.contains(task_id) {
                return false;
            }

            visited.insert(task_id.to_string());
            path.insert(task_id.to_string());

            if let Some(task) = all_tasks.get(task_id) {
                for dep in &task.depends_on {
                    if has_cycle(dep, visited, path, all_tasks) {
                        return true;
                    }
                }
            }

            path.remove(task_id);
            false
        }

        let all_tasks_map: std::collections::HashMap<String, &AtomicTask> = self
            .all_tasks()
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();

        let mut visited = std::collections::HashSet::new();
        let mut path = std::collections::HashSet::new();

        for task in self.all_tasks() {
            if has_cycle(&task.id, &mut visited, &mut path, &all_tasks_map) {
                errors.push(format!(
                    "Circular dependency detected involving {}",
                    task.id
                ));
                break;
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Get tasks in topological order (respecting dependencies).
    pub fn topological_order(&self) -> Result<Vec<&AtomicTask>, String> {
        self.validate_dependencies().map_err(|e| e.join("; "))?;

        let mut result = Vec::new();
        let mut completed = std::collections::HashSet::new();
        let all_tasks: std::collections::HashMap<String, &AtomicTask> = self
            .all_tasks()
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();

        while result.len() < all_tasks.len() {
            let mut made_progress = false;

            for task in all_tasks.values() {
                if completed.contains(&task.id) {
                    continue;
                }

                let deps_satisfied = task.depends_on.iter().all(|dep| completed.contains(dep));

                if deps_satisfied {
                    result.push(*task);
                    completed.insert(task.id.clone());
                    made_progress = true;
                }
            }

            if !made_progress {
                return Err("Deadlock in task dependencies".to_string());
            }
        }

        Ok(result)
    }
}

/// Decomposer that breaks down goals using first-principles thinking.
#[derive(Clone, Default)]
pub struct FirstPrinciplesDecomposer {
    llm: Option<Arc<dyn crate::llm_client::LlmClient>>,
}

impl FirstPrinciplesDecomposer {
    /// Create a new decomposer without LLM.
    pub fn new() -> Self {
        Self { llm: None }
    }

    /// Create a new decomposer with LLM client.
    pub fn with_llm(llm: Arc<dyn crate::llm_client::LlmClient>) -> Self {
        Self { llm: Some(llm) }
    }

    /// Decompose a goal into a plan.
    ///
    /// Uses LLM if available, otherwise returns a basic placeholder.
    pub async fn decompose(
        &self,
        goal: &str,
        context: &DecompositionContext,
    ) -> DecompositionResult {
        // Try LLM decomposition first if available
        if let Some(llm) = &self.llm {
            match self.decompose_with_llm(llm.as_ref(), goal, context).await {
                Ok(result) => return result,
                Err(e) => {
                    tracing::warn!(error = %e, "LLM decomposition failed, using fallback");
                }
            }
        }

        // Fallback to simple decomposition
        self.decompose_fallback(goal, context)
    }

    /// Decompose with a real configured LLM and fail instead of silently
    /// substituting the deterministic fallback plan.
    pub async fn decompose_required(
        &self,
        goal: &str,
        context: &DecompositionContext,
    ) -> Result<DecompositionResult, anyhow::Error> {
        let llm = self
            .llm
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("a real LLM client is required"))?;
        self.decompose_with_llm(llm, goal, context).await
    }

    /// Decompose using LLM.
    async fn decompose_with_llm(
        &self,
        llm: &dyn crate::llm_client::LlmClient,
        goal: &str,
        context: &DecompositionContext,
    ) -> Result<DecompositionResult, anyhow::Error> {
        let system_prompt = r#"You are a project planner that breaks down goals into executable tasks.
Analyze the user's goal and create a structured decomposition.

Respond in JSON format:
{
  "refined_goal": "A clearer version of the original goal",
  "constraints": ["constraint1", "constraint2"],
  "tech_decisions": {
    "frontend": "React",
    "backend": "Node.js",
    "database": "PostgreSQL",
    "caching": null,
    "deployment": "Docker",
    "third_party": ["service1", "service2"]
  },
  "milestones": [
    {
      "id": "M1",
      "name": "Milestone name",
      "description": "What this achieves",
      "tasks": [
        {
          "id": "T1.1",
          "title": "Task title",
          "description": "Detailed description",
          "acceptance": {
            "description": "How to verify done",
            "test_commands": ["npm test"],
            "artifacts": ["file1.txt"]
          },
          "depends_on": [],
          "worker_type": "aider",
          "estimated_minutes": 30
        }
      ],
      "starts_after": []
    }
  ],
  "total_minutes": 120
}

IMPORTANT: Return ONLY valid JSON, no markdown formatting."#;

        let context_info = format!(
            "Platform: {:?}\nPreferred languages: {:?}\nConstraints: {:?}",
            context.platform, context.preferred_languages, context.constraints
        );

        let user_message = format!("Context:\n{}\n\nGoal to decompose:\n{}", context_info, goal);

        let messages = vec![
            crate::llm_client::ChatMessage::system(system_prompt),
            crate::llm_client::ChatMessage::user(&user_message),
        ];

        let mut request = crate::llm_client::LlmRequest::new(messages, "task_decomposition");
        request.max_tokens = Some(4000);
        request.temperature = Some(0.7);

        let response = llm.chat(&request).await?;

        // Parse JSON response
        let json_str = response.content.trim();
        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse LLM response: {}", e))?;

        let result = self.parse_decomposition_response(goal, parsed)?;
        Ok(result)
    }

    /// Parse LLM JSON response into DecompositionResult.
    fn parse_decomposition_response(
        &self,
        goal: &str,
        parsed: serde_json::Value,
    ) -> Result<DecompositionResult, anyhow::Error> {
        let mut result = DecompositionResult::new(goal);

        if let Some(v) = parsed.get("refined_goal").and_then(|v| v.as_str()) {
            result.refined_goal = v.to_string();
        }

        if let Some(arr) = parsed.get("constraints").and_then(|v| v.as_array()) {
            result.constraints = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }

        if let Some(obj) = parsed.get("tech_decisions") {
            if let Some(v) = obj.get("frontend").and_then(|v| v.as_str()) {
                result.tech_decisions.frontend = Some(v.to_string());
            }
            if let Some(v) = obj.get("backend").and_then(|v| v.as_str()) {
                result.tech_decisions.backend = Some(v.to_string());
            }
            if let Some(v) = obj.get("database").and_then(|v| v.as_str()) {
                result.tech_decisions.database = Some(v.to_string());
            }
            if let Some(v) = obj.get("deployment").and_then(|v| v.as_str()) {
                result.tech_decisions.deployment = Some(v.to_string());
            }
            if let Some(arr) = obj.get("third_party").and_then(|v| v.as_array()) {
                result.tech_decisions.third_party = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
            }
        }

        if let Some(milestones) = parsed.get("milestones").and_then(|v| v.as_array()) {
            for m in milestones {
                let milestone = self.parse_milestone(m)?;
                result.milestones.push(milestone);
            }
        }

        if let Some(v) = parsed.get("total_minutes").and_then(|v| v.as_u64()) {
            result.total_estimated_minutes = v as u32;
        }

        Ok(result)
    }

    /// Parse a milestone from JSON.
    fn parse_milestone(&self, v: &serde_json::Value) -> Result<Milestone, anyhow::Error> {
        let id = v
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("M1")
            .to_string();
        let name = v
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = v
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut milestone = Milestone {
            id,
            name,
            description,
            tasks: Vec::new(),
            starts_after: Vec::new(),
        };

        if let Some(tasks) = v.get("tasks").and_then(|v| v.as_array()) {
            for t in tasks {
                if let Ok(task) = self.parse_task(t) {
                    milestone.tasks.push(task);
                }
            }
        }

        if let Some(arr) = v.get("starts_after").and_then(|v| v.as_array()) {
            milestone.starts_after = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }

        Ok(milestone)
    }

    /// Parse a task from JSON.
    fn parse_task(&self, v: &serde_json::Value) -> Result<AtomicTask, anyhow::Error> {
        let id = v
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("T1")
            .to_string();
        let title = v
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = v
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let worker_type_str = v
            .get("worker_type")
            .and_then(|v| v.as_str())
            .unwrap_or("aider");
        let estimated_minutes = v
            .get("estimated_minutes")
            .and_then(|v| v.as_u64())
            .unwrap_or(30) as u32;

        let acceptance = if let Some(acc_obj) = v.get("acceptance") {
            AcceptanceCriteria {
                description: acc_obj
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Done")
                    .to_string(),
                test_commands: acc_obj
                    .get("test_commands")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                artifacts: acc_obj
                    .get("artifacts")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        } else {
            AcceptanceCriteria::default()
        };

        let depends_on = v
            .get("depends_on")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let worker_type = match worker_type_str {
            "claude_code" => crate::worker_types::WorkerType::ClaudeCode,
            "opencode" => crate::worker_types::WorkerType::OpenCode,
            "aider" => crate::worker_types::WorkerType::Aider,
            "cli" => crate::worker_types::WorkerType::GenericCli,
            _ => crate::worker_types::WorkerType::Aider,
        };

        Ok(AtomicTask {
            id,
            title,
            description,
            acceptance,
            depends_on,
            worker_type,
            estimated_minutes,
            metadata: std::collections::HashMap::new(),
        })
    }

    /// Fallback decomposition when LLM is not available.
    fn decompose_fallback(
        &self,
        goal: &str,
        context: &DecompositionContext,
    ) -> DecompositionResult {
        let mut result = DecompositionResult::new(goal);

        // Basic constraints from context
        if let Some(platform) = &context.platform {
            result.constraints.push(format!("Platform: {}", platform));
        }
        result.constraints.extend(context.constraints.clone());

        // Create a single milestone with basic tasks
        let task = AtomicTask {
            id: "T1.1".to_string(),
            title: "Analyze and plan".to_string(),
            description: format!("Analyze the goal: {}", goal),
            acceptance: AcceptanceCriteria {
                description: "Analysis complete".to_string(),
                test_commands: vec![],
                artifacts: vec![],
            },
            depends_on: vec![],
            worker_type: crate::worker_types::WorkerType::Aider,
            estimated_minutes: 30,
            metadata: std::collections::HashMap::new(),
        };

        let milestone = Milestone {
            id: "M1".to_string(),
            name: "Initial Planning".to_string(),
            description: "Initial analysis and planning phase".to_string(),
            tasks: vec![task],
            starts_after: vec![],
        };

        result.milestones.push(milestone);
        result.total_estimated_minutes = 30;

        result
    }
}

/// Context for decomposition — owner preferences, platform constraints, etc.
#[derive(Clone, Debug, Default)]
pub struct DecompositionContext {
    pub platform: Option<String>,
    pub preferred_languages: Vec<String>,
    pub constraints: Vec<String>,
    pub owner_preferences: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_task(id: &str, deps: Vec<&str>) -> AtomicTask {
        AtomicTask {
            id: id.to_string(),
            title: format!("Task {}", id),
            description: format!("Description for {}", id),
            acceptance: AcceptanceCriteria {
                description: "Done".to_string(),
                test_commands: vec![],
                artifacts: vec![],
            },
            depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
            worker_type: WorkerType::OpenCode,
            estimated_minutes: 10,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn test_task_creation() {
        let task = create_test_task("T1", vec![]);
        assert_eq!(task.id, "T1");
        assert!(task.depends_on.is_empty());
    }

    #[test]
    fn test_validate_dependencies_ok() {
        let m1 = Milestone {
            id: "M1".to_string(),
            name: "Setup".to_string(),
            description: "Initial setup".to_string(),
            tasks: vec![
                create_test_task("T1.1", vec![]),
                create_test_task("T1.2", vec!["T1.1"]),
            ],
            starts_after: vec![],
        };

        let result = DecompositionResult {
            refined_goal: "Test".to_string(),
            constraints: vec![],
            tech_decisions: TechDecisions::default(),
            milestones: vec![m1],
            total_estimated_minutes: 20,
        };

        assert!(result.validate_dependencies().is_ok());
    }

    #[test]
    fn test_validate_dependencies_missing() {
        let m1 = Milestone {
            id: "M1".to_string(),
            name: "Setup".to_string(),
            description: "Initial setup".to_string(),
            tasks: vec![create_test_task("T1.1", vec!["T0"])],
            starts_after: vec![],
        };

        let result = DecompositionResult {
            refined_goal: "Test".to_string(),
            constraints: vec![],
            tech_decisions: TechDecisions::default(),
            milestones: vec![m1],
            total_estimated_minutes: 10,
        };

        let errors = result.validate_dependencies().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("T0"));
    }

    #[test]
    fn test_validate_dependencies_circular() {
        let m1 = Milestone {
            id: "M1".to_string(),
            name: "Setup".to_string(),
            description: "Initial setup".to_string(),
            tasks: vec![
                create_test_task("T1", vec!["T2"]),
                create_test_task("T2", vec!["T1"]),
            ],
            starts_after: vec![],
        };

        let result = DecompositionResult {
            refined_goal: "Test".to_string(),
            constraints: vec![],
            tech_decisions: TechDecisions::default(),
            milestones: vec![m1],
            total_estimated_minutes: 20,
        };

        let errors = result.validate_dependencies().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Circular"));
    }

    #[test]
    fn test_topological_order() {
        let m1 = Milestone {
            id: "M1".to_string(),
            name: "Setup".to_string(),
            description: "Initial setup".to_string(),
            tasks: vec![
                create_test_task("T3", vec!["T2"]),
                create_test_task("T1", vec![]),
                create_test_task("T2", vec!["T1"]),
            ],
            starts_after: vec![],
        };

        let result = DecompositionResult {
            refined_goal: "Test".to_string(),
            constraints: vec![],
            tech_decisions: TechDecisions::default(),
            milestones: vec![m1],
            total_estimated_minutes: 30,
        };

        let order = result.topological_order().unwrap();
        let ids: Vec<_> = order.iter().map(|t| t.id.as_str()).collect();

        // T1 must come before T2, T2 before T3
        let t1_idx = ids.iter().position(|&id| id == "T1").unwrap();
        let t2_idx = ids.iter().position(|&id| id == "T2").unwrap();
        let t3_idx = ids.iter().position(|&id| id == "T3").unwrap();

        assert!(t1_idx < t2_idx);
        assert!(t2_idx < t3_idx);
    }

    #[test]
    fn test_worker_type_serialization() {
        let wt = WorkerType::OpenCode;
        let json = serde_json::to_string(&wt).unwrap();
        // snake_case converts OpenCode to "open_code"
        assert!(json.contains("open_code"), "JSON was: {}", json);

        // Test round-trip
        let deserialized: WorkerType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, WorkerType::OpenCode);
    }
}
