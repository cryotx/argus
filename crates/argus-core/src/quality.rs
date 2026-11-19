//! Quality Check Loop — validates task output before completion.
//!
//! Runs tests, AI code review, and security scans to ensure quality.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// Kill the isolated group on completion, timeout or future cancellation.
#[cfg(unix)]
pub struct ProcessGroupGuard(pub Option<u32>);

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        if let Some(pid) = self
            .0
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
        {
            // SAFETY: a negative child PID addresses its own isolated group; SIGKILL is 9 on Unix.
            unsafe {
                kill(-pid, 9);
            }
        }
    }
}

/// Result of a quality check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityResult {
    /// All checks passed.
    Passed,
    /// Checks passed with warnings.
    PassedWithWarnings { warnings: Vec<String> },
    /// One or more checks failed.
    Failed { errors: Vec<String> },
    /// Could not complete checks (e.g., missing test command).
    Indeterminate { reason: String },
}

impl QualityResult {
    /// Check if the result is passing (including with warnings).
    pub fn is_passing(&self) -> bool {
        matches!(
            self,
            QualityResult::Passed | QualityResult::PassedWithWarnings { .. }
        )
    }

    /// Check if the result is failing.
    pub fn is_failing(&self) -> bool {
        matches!(self, QualityResult::Failed { .. })
    }

    /// Get errors if any.
    pub fn errors(&self) -> Option<&[String]> {
        match self {
            QualityResult::Failed { errors } => Some(errors),
            _ => None,
        }
    }

    /// Get warnings if any.
    pub fn warnings(&self) -> Option<&[String]> {
        match self {
            QualityResult::PassedWithWarnings { warnings } => Some(warnings),
            _ => None,
        }
    }
}

/// Individual check stage result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageResult {
    pub stage: QualityStage,
    pub result: QualityResult,
    pub duration_ms: u64,
    pub details: HashMap<String, String>,
}

/// Quality check stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityStage {
    /// Run test commands.
    Test,
    /// AI code review.
    CodeReview,
    /// Security scan.
    Security,
    /// Lint and format checks.
    Lint,
    /// Build verification.
    Build,
    /// Post-deployment health verification.
    HealthCheck,
}

/// Complete quality report for a task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QualityReport {
    pub task_id: String,
    pub overall: QualityResult,
    pub stages: Vec<StageResult>,
    pub total_duration_ms: u64,
    pub recommendations: Vec<String>,
}

/// Quality check configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QualityConfig {
    /// Whether to run tests.
    pub run_tests: bool,
    /// Whether to run AI code review.
    pub run_code_review: bool,
    /// Whether to run security scan.
    pub run_security: bool,
    /// Whether to run lint checks.
    pub run_lint: bool,
    /// Whether to run build verification.
    pub run_build: bool,
    /// Whether deployment health checks are required when a task deploys.
    pub run_health_checks: bool,
    /// Code review model (different from the one that wrote the code).
    pub review_model: Option<String>,
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self {
            run_tests: true,
            run_code_review: true,
            run_security: true,
            run_lint: true,
            run_build: true,
            run_health_checks: true,
            review_model: None,
        }
    }
}

/// Quality checker — runs the quality check loop.
#[derive(Clone, Default)]
pub struct QualityChecker {
    config: QualityConfig,
    llm: Option<Arc<dyn crate::llm_client::LlmClient>>,
}

impl QualityChecker {
    /// Create a new quality checker without LLM.
    pub fn new(config: QualityConfig) -> Self {
        Self { config, llm: None }
    }

    /// Create a new quality checker with LLM client.
    pub fn with_llm(config: QualityConfig, llm: Arc<dyn crate::llm_client::LlmClient>) -> Self {
        Self {
            config,
            llm: Some(llm),
        }
    }

    /// Run quality checks on task output.
    ///
    /// In production, this would:
    /// 1. Execute test commands
    /// 2. Call different LLM for code review
    /// 3. Run security scanners (secret detection, SAST)
    /// 4. Return detailed report
    pub async fn check(&self, task_output: &TaskOutput) -> QualityReport {
        let mut stages = Vec::new();
        let start = std::time::Instant::now();

        // Stage 1: Run tests
        if self.config.run_tests {
            let stage_start = std::time::Instant::now();
            let result = self.run_tests(task_output).await;
            stages.push(StageResult {
                stage: QualityStage::Test,
                result,
                duration_ms: stage_start.elapsed().as_millis() as u64,
                details: HashMap::new(),
            });
        }

        // Stage 2: Code review (skip if tests failed hard)
        if self.config.run_code_review && !stages.iter().any(|s| s.result.is_failing()) {
            let stage_start = std::time::Instant::now();
            let result = self.run_code_review(task_output).await;
            stages.push(StageResult {
                stage: QualityStage::CodeReview,
                result,
                duration_ms: stage_start.elapsed().as_millis() as u64,
                details: HashMap::new(),
            });
        }

        // Stage 3: Security scan
        if self.config.run_security {
            let stage_start = std::time::Instant::now();
            let (result, details) = self.run_security_scan(task_output).await;
            stages.push(StageResult {
                stage: QualityStage::Security,
                result,
                duration_ms: stage_start.elapsed().as_millis() as u64,
                details,
            });
        }

        // Stage 4: Lint
        if self.config.run_lint {
            let stage_start = std::time::Instant::now();
            let result = self.run_lint(task_output).await;
            stages.push(StageResult {
                stage: QualityStage::Lint,
                result,
                duration_ms: stage_start.elapsed().as_millis() as u64,
                details: HashMap::new(),
            });
        }

        // Stage 5: Build
        if self.config.run_build {
            let stage_start = std::time::Instant::now();
            let result = self.run_build(task_output).await;
            stages.push(StageResult {
                stage: QualityStage::Build,
                result,
                duration_ms: stage_start.elapsed().as_millis() as u64,
                details: HashMap::new(),
            });
        }

        if self.config.run_health_checks && task_output.requires_deployment {
            let stage_start = std::time::Instant::now();
            let result = self
                .run_commands(
                    "Health check",
                    &task_output.health_check_commands,
                    task_output,
                )
                .await;
            stages.push(StageResult {
                stage: QualityStage::HealthCheck,
                result,
                duration_ms: stage_start.elapsed().as_millis() as u64,
                details: HashMap::new(),
            });
        }

        // Determine overall result
        let overall = Self::aggregate_results(&stages);

        // Generate recommendations
        let recommendations = Self::generate_recommendations(&stages);

        QualityReport {
            task_id: task_output.task_id.clone(),
            overall,
            stages,
            total_duration_ms: start.elapsed().as_millis() as u64,
            recommendations,
        }
    }

    /// Run tests for the task.
    async fn run_tests(&self, task_output: &TaskOutput) -> QualityResult {
        if task_output.test_commands.is_empty() {
            return QualityResult::Indeterminate {
                reason: "No test commands provided".to_string(),
            };
        }

        self.run_commands("Test", &task_output.test_commands, task_output)
            .await
    }

    /// Run AI code review.
    async fn run_code_review(&self, task_output: &TaskOutput) -> QualityResult {
        // Use LLM to review code if available
        if let Some(llm) = &self.llm {
            match self.review_with_llm(llm.as_ref(), task_output).await {
                Ok(result) => return result,
                Err(e) => {
                    tracing::warn!(error = %e, "LLM code review failed");
                }
            }
        }

        // Fallback: basic pattern checks
        let mut warnings = Vec::new();

        // Check for common issues in modified files
        for file_path in &task_output.files_modified {
            if let Ok(content) = tokio::fs::read_to_string(file_path).await {
                // Check for console.log or print statements
                if content.contains("console.log") || content.contains("print!(") {
                    warnings.push(format!("Found debug statement in {}", file_path));
                }
                // Check for TODO without issue tracker
                if content.contains("TODO") && !content.contains("https://github.com") {
                    warnings.push(format!("Found TODO without issue link in {}", file_path));
                }
            }
        }

        if warnings.is_empty() {
            QualityResult::Passed
        } else {
            QualityResult::PassedWithWarnings { warnings }
        }
    }

    /// Review code using LLM.
    async fn review_with_llm(
        &self,
        llm: &dyn crate::llm_client::LlmClient,
        task_output: &TaskOutput,
    ) -> Result<QualityResult, anyhow::Error> {
        let system_prompt = r#"You are a senior code reviewer. Review the following code changes and identify:
1. Bugs or potential runtime errors
2. Security vulnerabilities
3. Code quality issues
4. Performance concerns

Respond in JSON format:
{
  "issues": [
    {
      "severity": "critical|warning|suggestion",
      "file": "path/to/file",
      "line": 10,
      "description": "issue description"
    }
  ],
  "overall": "pass|pass_with_warnings|fail"

If no issues found, return {"issues": [], "overall": "pass"}."#;

        let mut code_context = String::new();
        for file_path in &task_output.files_modified {
            if let Ok(content) = tokio::fs::read_to_string(file_path).await {
                code_context.push_str(&format!("\n=== {} ===\n", file_path));
                code_context.push_str(&content);
            }
        }

        let messages = vec![
            crate::llm_client::ChatMessage::system(system_prompt),
            crate::llm_client::ChatMessage::user(&code_context),
        ];

        let mut request = crate::llm_client::LlmRequest::new(messages, "code_review");
        request.max_tokens = Some(2000);
        request.temperature = Some(0.3);

        let response = llm.chat(&request).await?;
        let parsed: serde_json::Value = serde_json::from_str(response.content.trim())
            .map_err(|e| anyhow::anyhow!("Failed to parse LLM response: {}", e))?;

        let overall = parsed
            .get("overall")
            .and_then(|v| v.as_str())
            .unwrap_or("pass");

        let issues: Vec<String> = parsed
            .get("issues")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|i| {
                        let severity = i
                            .get("severity")
                            .and_then(|v| v.as_str())
                            .unwrap_or("warning");
                        let file = i.get("file").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let desc = i.get("description").and_then(|v| v.as_str()).unwrap_or("");
                        if severity == "critical" {
                            Some(format!("{}: {}", file, desc))
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        if !issues.is_empty() {
            return Ok(QualityResult::Failed { errors: issues });
        }

        if overall == "pass_with_warnings" {
            let warnings: Vec<String> = parsed
                .get("issues")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|i| {
                            let file = i.get("file").and_then(|v| v.as_str()).unwrap_or("unknown");
                            let desc = i.get("description").and_then(|v| v.as_str()).unwrap_or("");
                            format!("{}: {}", file, desc)
                        })
                        .collect()
                })
                .unwrap_or_default();

            return Ok(QualityResult::PassedWithWarnings { warnings });
        }

        Ok(QualityResult::Passed)
    }

    /// Run security scan.
    async fn run_security_scan(
        &self,
        task_output: &TaskOutput,
    ) -> (QualityResult, HashMap<String, String>) {
        let mut issues = Vec::new();
        let mut scanned_files = 0;
        let mut skipped_deleted_files = 0;

        // Secret detection patterns
        let secret_patterns: Vec<(regex::Regex, &str)> = vec![
            (
                regex::Regex::new(r"(?i)(api[_-]?key|apikey)\s*[=:]\s*[a-zA-Z0-9_\-]{20,}")
                    .unwrap(),
                "Potential API key",
            ),
            (
                regex::Regex::new(r"(?i)(secret[_-]?key|secretkey)\s*[=:]\s*[a-zA-Z0-9_\-]{20,}")
                    .unwrap(),
                "Potential secret key",
            ),
            (
                regex::Regex::new(r"(?i)password\s*[=:]\s*[^\s]{8,}").unwrap(),
                "Hardcoded password",
            ),
            (
                regex::Regex::new(r"(?i)bearer\s+[a-zA-Z0-9_\-]+\.[a-zA-Z0-9_\-]+").unwrap(),
                "Bearer token",
            ),
            (
                regex::Regex::new(r"ghp_[a-zA-Z0-9]{36}").unwrap(),
                "GitHub token",
            ),
            (
                regex::Regex::new(r"sk-[a-zA-Z0-9]{48}").unwrap(),
                "OpenAI API key",
            ),
        ];

        match tokio::fs::canonicalize(&task_output.workspace_dir).await {
            Ok(workspace) => {
                let mut seen = HashSet::new();
                for file_path in task_output
                    .files_modified
                    .iter()
                    .chain(&task_output.files_created)
                {
                    if !seen.insert(file_path) {
                        continue;
                    }
                    match read_security_file(&workspace, file_path).await {
                        Ok(Some(content)) => {
                            scanned_files += 1;
                            for (re, description) in &secret_patterns {
                                if re.is_match(&content) {
                                    issues.push(format!("{} in {}", description, file_path));
                                }
                            }
                        }
                        Ok(None) => skipped_deleted_files += 1,
                        Err(reason) => issues.push(reason),
                    }
                }
            }
            Err(error) => issues.push(format!("Cannot resolve security scan workspace: {error}")),
        }

        let details = HashMap::from([
            ("scanned_files".into(), scanned_files.to_string()),
            (
                "skipped_deleted_files".into(),
                skipped_deleted_files.to_string(),
            ),
            ("failure_reasons".into(), issues.join("; ")),
        ]);
        let result = if issues.is_empty() {
            QualityResult::Passed
        } else {
            QualityResult::Failed { errors: issues }
        };
        (result, details)
    }

    /// Run lint checks.
    async fn run_lint(&self, task_output: &TaskOutput) -> QualityResult {
        if !task_output.lint_commands.is_empty() {
            return self
                .run_commands("Lint", &task_output.lint_commands, task_output)
                .await;
        }

        let mut all_issues = Vec::new();

        // Try to run appropriate linter based on file type
        for file_path in &task_output.files_modified {
            let path = std::path::Path::new(file_path);
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            let linter_cmd: Option<(&str, Vec<String>)> = match extension {
                "rs" => Some(("rustfmt", vec!["--check".to_string(), file_path.clone()])),
                "js" | "jsx" | "ts" | "tsx" => {
                    Some(("npx", vec!["eslint".to_string(), file_path.clone()]))
                }
                "py" => Some(("pylint", vec![file_path.clone()])),
                "toml" => Some(("toml-check", vec![file_path.clone()])),
                _ => None,
            };

            if let Some((cmd, args)) = linter_cmd {
                let result = tokio::process::Command::new(cmd)
                    .args(&args)
                    .current_dir(&task_output.workspace_dir)
                    .output()
                    .await;

                if let Ok(output) = result {
                    if !output.status.success() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        all_issues.push(format!(
                            "{} issue in {}: {}",
                            cmd,
                            file_path,
                            stderr.trim()
                        ));
                    }
                }
            }
        }

        if all_issues.is_empty() {
            QualityResult::Passed
        } else {
            QualityResult::PassedWithWarnings {
                warnings: all_issues,
            }
        }
    }

    /// Run build verification.
    async fn run_build(&self, task_output: &TaskOutput) -> QualityResult {
        if !task_output.build_commands.is_empty() {
            return self
                .run_commands("Build", &task_output.build_commands, task_output)
                .await;
        }

        // Try to detect and run build command
        let build_commands = ["npm run build", "cargo build", "go build", "make"];

        for cmd in build_commands {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            let result = tokio::process::Command::new(parts[0])
                .args(&parts[1..])
                .current_dir(&task_output.workspace_dir)
                .output()
                .await;

            if let Ok(output) = result {
                if output.status.success() {
                    return QualityResult::Passed;
                }
                // Try next command if this one doesn't exist
                if output.status.code() == Some(127) {
                    continue;
                }
                // Other errors might be actual build failures
                let stderr = String::from_utf8_lossy(&output.stderr);
                return QualityResult::Failed {
                    errors: vec![format!("Build failed: {}", stderr.trim())],
                };
            }
        }

        // No build commands available
        QualityResult::Indeterminate {
            reason: "No build commands available to verify".to_string(),
        }
    }

    async fn run_commands(
        &self,
        label: &str,
        commands: &[String],
        task_output: &TaskOutput,
    ) -> QualityResult {
        if commands.is_empty() {
            return QualityResult::Indeterminate {
                reason: format!("No {} commands provided", label.to_ascii_lowercase()),
            };
        }

        let mut errors = Vec::new();
        for command in commands {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.arg("-c")
                .arg(command)
                .current_dir(&task_output.workspace_dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            cmd.as_std_mut().process_group(0);
            let child = match cmd.spawn() {
                Ok(child) => child,
                Err(error) => {
                    errors.push(format!(
                        "{label} command '{command}' could not start: {error}"
                    ));
                    continue;
                }
            };
            #[cfg(unix)]
            let _group = ProcessGroupGuard(child.id());
            let result =
                tokio::time::timeout(task_output.command_timeout, child.wait_with_output()).await;
            match result {
                Ok(Ok(output)) if output.status.success() => {}
                Ok(Ok(output)) => errors.push(format!(
                    "{label} command '{command}' failed: {} {}",
                    String::from_utf8_lossy(&output.stdout).trim(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Ok(Err(error)) => errors.push(format!(
                    "{label} command '{command}' could not start: {error}"
                )),
                Err(_) => errors.push(format!(
                    "{label} command '{command}' timed out after {}s",
                    task_output.command_timeout.as_secs()
                )),
            }
        }

        if errors.is_empty() {
            QualityResult::Passed
        } else {
            QualityResult::Failed { errors }
        }
    }

    /// Run compensating commands after a failed gate. A task that has no
    /// automatic rollback commands is explicitly marked for human intervention.
    pub async fn rollback(&self, task_output: &TaskOutput) -> QualityResult {
        if task_output.rollback_commands.is_empty() {
            return QualityResult::Indeterminate {
                reason: "No automatic rollback configured; human intervention required".to_string(),
            };
        }
        self.run_commands("Rollback", &task_output.rollback_commands, task_output)
            .await
    }

    /// Aggregate individual stage results into overall result.
    fn aggregate_results(stages: &[StageResult]) -> QualityResult {
        let mut all_errors = Vec::new();
        let mut all_warnings = Vec::new();
        let mut any_indeterminate = false;

        for stage in stages {
            match &stage.result {
                QualityResult::Failed { errors } => {
                    all_errors.extend(errors.clone());
                }
                QualityResult::PassedWithWarnings { warnings } => {
                    all_warnings.extend(warnings.clone());
                }
                QualityResult::Indeterminate { .. } => {
                    any_indeterminate = true;
                }
                _ => {}
            }
        }

        if !all_errors.is_empty() {
            QualityResult::Failed { errors: all_errors }
        } else if !all_warnings.is_empty() {
            QualityResult::PassedWithWarnings {
                warnings: all_warnings,
            }
        } else if any_indeterminate {
            QualityResult::Indeterminate {
                reason: "Some stages could not be evaluated".to_string(),
            }
        } else {
            QualityResult::Passed
        }
    }

    /// Generate recommendations based on stage results.
    fn generate_recommendations(stages: &[StageResult]) -> Vec<String> {
        let mut recommendations = Vec::new();

        for stage in stages {
            match &stage.result {
                QualityResult::Failed { errors } => {
                    recommendations.push(format!(
                        "Fix {:?} issues: {}",
                        stage.stage,
                        errors.join(", ")
                    ));
                }
                QualityResult::PassedWithWarnings { warnings } => {
                    recommendations.push(format!(
                        "Consider addressing {:?} warnings: {}",
                        stage.stage,
                        warnings.join(", ")
                    ));
                }
                _ => {}
            }
        }

        if recommendations.is_empty() {
            recommendations.push("No recommendations — all checks passed!".to_string());
        }

        recommendations
    }
}

async fn read_security_file(workspace: &Path, file_path: &str) -> Result<Option<String>, String> {
    let input = Path::new(file_path);
    let path = workspace.join(input);
    if input.components().any(|part| part == Component::ParentDir) || !path.starts_with(workspace) {
        return Err(format!("Security scan path escapes workspace: {file_path}"));
    }
    let resolved = match tokio::fs::canonicalize(&path).await {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A deleted file is safe to skip only when its nearest existing ancestor is inside.
            let mut ancestor = path.as_path();
            loop {
                match tokio::fs::symlink_metadata(ancestor).await {
                    Ok(_) => {
                        let resolved =
                            tokio::fs::canonicalize(ancestor).await.map_err(|error| {
                                format!("Cannot resolve security scan path {file_path}: {error}")
                            })?;
                        if !resolved.starts_with(workspace) {
                            return Err(format!(
                                "Security scan path escapes workspace: {file_path}"
                            ));
                        }
                        return Ok(None);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        ancestor = ancestor.parent().ok_or_else(|| {
                            format!("Cannot resolve security scan path: {file_path}")
                        })?;
                    }
                    Err(error) => {
                        return Err(format!(
                            "Cannot inspect security scan path {file_path}: {error}"
                        ));
                    }
                }
            }
        }
        Err(error) => {
            return Err(format!(
                "Cannot resolve security scan path {file_path}: {error}"
            ));
        }
    };
    if !resolved.starts_with(workspace) {
        return Err(format!("Security scan path escapes workspace: {file_path}"));
    }
    tokio::fs::read_to_string(&resolved)
        .await
        .map(Some)
        .map_err(|error| format!("Cannot read security scan file {file_path}: {error}"))
}

/// Output from a completed task.
#[derive(Clone, Debug)]
pub struct TaskOutput {
    pub task_id: String,
    pub workspace_dir: PathBuf,
    pub files_modified: Vec<String>,
    pub files_created: Vec<String>,
    pub test_commands: Vec<String>,
    pub lint_commands: Vec<String>,
    pub build_commands: Vec<String>,
    pub health_check_commands: Vec<String>,
    pub rollback_commands: Vec<String>,
    pub requires_deployment: bool,
    pub command_timeout: Duration,
    pub output_log: String,
    pub written_by_model: Option<String>,
}

impl Default for TaskOutput {
    fn default() -> Self {
        Self {
            task_id: String::new(),
            workspace_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            files_modified: Vec::new(),
            files_created: Vec::new(),
            test_commands: Vec::new(),
            lint_commands: Vec::new(),
            build_commands: Vec::new(),
            health_check_commands: Vec::new(),
            rollback_commands: Vec::new(),
            requires_deployment: false,
            command_timeout: Duration::from_secs(15 * 60),
            output_log: String::new(),
            written_by_model: None,
        }
    }
}

/// Cross-model code review assignment.
pub struct CodeReviewAssignment;

impl CodeReviewAssignment {
    /// Get a different model for code review.
    ///
    /// Returns a model that is different from the one that wrote the code
    /// to avoid blind spots.
    pub fn get_reviewer(written_by: Option<&str>) -> &'static str {
        match written_by {
            Some("claude") | Some("claude-sonnet") => "gpt-4o",
            Some("gpt-4o") | Some("gpt-4") => "claude-sonnet",
            Some("deepseek") => "claude-sonnet",
            _ => "local", // Fallback to local model for basic review
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_quality_commands_preserve_closed_stdin() {
        let result = QualityChecker::default()
            .run_commands(
                "Test",
                &["read value && exit 1; exit 0".into()],
                &TaskOutput::default(),
            )
            .await;
        assert_eq!(result, QualityResult::Passed);
    }

    #[cfg(target_os = "linux")]
    async fn quality_grandchild_terminated(cancel: bool) {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let output = TaskOutput {
            workspace_dir: dir.path().to_path_buf(),
            command_timeout: Duration::from_secs(1),
            ..Default::default()
        };
        let job = tokio::spawn(async move {
            QualityChecker::default()
                .run_commands(
                    "Test",
                    &["sleep 30 & printf '%s\\n' \"$!\" > pid; wait".into()],
                    &output,
                )
                .await
        });
        let pid = loop {
            if let Ok(text) = tokio::fs::read_to_string(&pid_file).await {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    break pid;
                }
            }
            assert!(!job.is_finished());
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let running = || {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|text| {
                text.rsplit_once(") ")
                    .is_some_and(|(_, tail)| !tail.starts_with('Z'))
            })
        };
        assert!(running());
        if cancel {
            job.abort();
            assert!(job.await.unwrap_err().is_cancelled());
        } else {
            let result = job.await.unwrap();
            assert!(result.is_failing());
            assert!(result.errors().unwrap()[0].contains("timed out"));
        }
        for _ in 0..100 {
            if !running() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("quality command grandchild {pid} survived termination");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_quality_timeout_kills_grandchild() {
        quality_grandchild_terminated(false).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_quality_cancellation_kills_grandchild() {
        quality_grandchild_terminated(true).await;
    }

    async fn security_report(
        workspace: &Path,
        modified: &[&str],
        created: &[&str],
    ) -> QualityReport {
        let checker = QualityChecker::new(QualityConfig {
            run_tests: false,
            run_code_review: false,
            run_security: true,
            run_lint: false,
            run_build: false,
            run_health_checks: false,
            review_model: None,
        });
        checker
            .check(&TaskOutput {
                workspace_dir: workspace.to_path_buf(),
                files_modified: modified.iter().map(|path| (*path).into()).collect(),
                files_created: created.iter().map(|path| (*path).into()).collect(),
                ..Default::default()
            })
            .await
    }

    fn assert_scan_failed(report: &QualityReport, reason: &str) {
        assert!(report.overall.is_failing());
        assert!(report.stages[0].details["failure_reasons"].contains(reason));
        assert!(
            report
                .overall
                .errors()
                .unwrap()
                .iter()
                .any(|error| error.contains(reason))
        );
    }

    #[tokio::test]
    async fn security_scan_worktree_secret_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("worktree");
        assert!(
            std::process::Command::new("git")
                .arg("init")
                .arg(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args([
                    "-c",
                    "user.name=scan-test",
                    "-c",
                    "user.email=scan@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "fixture"
                ])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["worktree", "add", "-b", "scan-test"])
                .arg(&worktree)
                .output()
                .unwrap()
                .status
                .success()
        );
        std::fs::write(
            worktree.join("new.env"),
            format!("api_key={}", "a".repeat(32)),
        )
        .unwrap();
        let report = security_report(&worktree, &["new.env"], &["new.env"]).await;
        assert_scan_failed(&report, "Potential API key in new.env");
        assert_eq!(report.stages[0].details["scanned_files"], "1");
    }

    #[tokio::test]
    async fn security_scan_unreadable_file_fails_closed() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("invalid.bin"), [0xff, 0xfe]).unwrap();
        let report = security_report(workspace.path(), &["invalid.bin"], &[]).await;
        assert_scan_failed(&report, "Cannot read security scan file invalid.bin");
        assert_eq!(report.stages[0].details["scanned_files"], "0");
    }

    #[tokio::test]
    async fn security_scan_path_traversal_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(temp.path().join("outside.txt"), "safe").unwrap();
        for path in [
            "../outside.txt".to_string(),
            temp.path().join("outside.txt").display().to_string(),
            "../deleted.txt".to_string(),
        ] {
            let report = security_report(&workspace, &[&path], &[]).await;
            assert_scan_failed(&report, "Security scan path escapes workspace");
            assert_eq!(report.stages[0].details["scanned_files"], "0");
        }
    }

    #[tokio::test]
    async fn security_scan_deleted_file_is_skipped() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("deleted.txt"), "safe").unwrap();
        std::fs::remove_file(workspace.path().join("deleted.txt")).unwrap();
        std::fs::write(workspace.path().join("kept.txt"), "safe").unwrap();
        let report = security_report(workspace.path(), &["deleted.txt", "kept.txt"], &[]).await;
        assert_eq!(report.overall, QualityResult::Passed);
        assert_eq!(report.stages[0].details["scanned_files"], "1");
        assert_eq!(report.stages[0].details["skipped_deleted_files"], "1");
        assert!(report.stages[0].details["failure_reasons"].is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn security_scan_symlink_escape_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("exists.txt"), "safe").unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("link")).unwrap();
        for path in ["link/exists.txt", "link/deleted.txt"] {
            let report = security_report(&workspace, &[path], &[]).await;
            assert_scan_failed(&report, "Security scan path escapes workspace");
        }
    }

    #[tokio::test]
    async fn security_scan_missing_workspace_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let report = security_report(&temp.path().join("missing"), &["file.txt"], &[]).await;
        assert_scan_failed(&report, "Cannot resolve security scan workspace");
        assert_eq!(report.stages[0].details["scanned_files"], "0");
    }

    #[test]
    fn test_quality_result_passing() {
        assert!(QualityResult::Passed.is_passing());
        assert!(
            QualityResult::PassedWithWarnings {
                warnings: vec!["Minor issue".to_string()]
            }
            .is_passing()
        );
        assert!(
            !QualityResult::Failed {
                errors: vec!["Major error".to_string()]
            }
            .is_passing()
        );
    }

    #[test]
    fn test_quality_result_errors() {
        let failed = QualityResult::Failed {
            errors: vec!["Error 1".to_string(), "Error 2".to_string()],
        };
        assert_eq!(failed.errors().unwrap().len(), 2);

        let passed = QualityResult::Passed;
        assert!(passed.errors().is_none());
    }

    #[test]
    fn test_code_review_assignment() {
        assert_eq!(CodeReviewAssignment::get_reviewer(Some("claude")), "gpt-4o");
        assert_eq!(
            CodeReviewAssignment::get_reviewer(Some("gpt-4o")),
            "claude-sonnet"
        );
        assert_eq!(
            CodeReviewAssignment::get_reviewer(Some("deepseek")),
            "claude-sonnet"
        );
        assert_eq!(CodeReviewAssignment::get_reviewer(None), "local");
    }

    #[test]
    fn test_aggregate_results_all_pass() {
        let stages = vec![
            StageResult {
                stage: QualityStage::Test,
                result: QualityResult::Passed,
                duration_ms: 100,
                details: HashMap::new(),
            },
            StageResult {
                stage: QualityStage::Lint,
                result: QualityResult::Passed,
                duration_ms: 50,
                details: HashMap::new(),
            },
        ];

        let result = QualityChecker::aggregate_results(&stages);
        assert!(matches!(result, QualityResult::Passed));
    }

    #[test]
    fn test_aggregate_results_with_warnings() {
        let stages = vec![
            StageResult {
                stage: QualityStage::Test,
                result: QualityResult::Passed,
                duration_ms: 100,
                details: HashMap::new(),
            },
            StageResult {
                stage: QualityStage::Lint,
                result: QualityResult::PassedWithWarnings {
                    warnings: vec!["Unused import".to_string()],
                },
                duration_ms: 50,
                details: HashMap::new(),
            },
        ];

        let result = QualityChecker::aggregate_results(&stages);
        match result {
            QualityResult::PassedWithWarnings { warnings } => {
                assert_eq!(warnings.len(), 1);
            }
            _ => panic!("Expected PassedWithWarnings"),
        }
    }

    #[test]
    fn test_aggregate_results_with_failure() {
        let stages = vec![
            StageResult {
                stage: QualityStage::Test,
                result: QualityResult::Failed {
                    errors: vec!["Test failed".to_string()],
                },
                duration_ms: 100,
                details: HashMap::new(),
            },
            StageResult {
                stage: QualityStage::Lint,
                result: QualityResult::Passed,
                duration_ms: 50,
                details: HashMap::new(),
            },
        ];

        let result = QualityChecker::aggregate_results(&stages);
        match result {
            QualityResult::Failed { errors } => {
                assert_eq!(errors.len(), 1);
                assert!(errors[0].contains("Test failed"));
            }
            _ => panic!("Expected Failed"),
        }
    }

    #[test]
    fn test_generate_recommendations() {
        let stages = vec![StageResult {
            stage: QualityStage::Test,
            result: QualityResult::Failed {
                errors: vec!["Test A failed".to_string()],
            },
            duration_ms: 100,
            details: HashMap::new(),
        }];

        let recs = QualityChecker::generate_recommendations(&stages);
        assert!(!recs.is_empty());
        assert!(recs[0].contains("Test"));
    }

    #[tokio::test]
    async fn test_quality_checker_pass() {
        let config = QualityConfig::default();
        let checker = QualityChecker::new(config);

        // Use a command that won't actually run tests in this context
        let output = TaskOutput {
            task_id: "T1".to_string(),
            test_commands: vec!["echo 'test'".to_string()],
            ..Default::default()
        };

        let report = checker.check(&output).await;
        assert_eq!(report.task_id, "T1");
        // Just check that stages are populated, not actual test results
        assert!(!report.stages.is_empty());
    }

    #[tokio::test]
    async fn test_quality_checker_no_tests() {
        let config = QualityConfig::default();
        let checker = QualityChecker::new(config);

        let output = TaskOutput {
            task_id: "T2".to_string(),
            test_commands: vec![],
            ..Default::default()
        };

        let report = checker.check(&output).await;

        // Should have a test stage with Indeterminate result
        let test_stage = report
            .stages
            .iter()
            .find(|s| matches!(s.stage, QualityStage::Test));
        assert!(test_stage.is_some());
        assert!(matches!(
            test_stage.unwrap().result,
            QualityResult::Indeterminate { .. }
        ));
    }
}
