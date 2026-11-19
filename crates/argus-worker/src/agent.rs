//! One-shot coding-agent drivers.
//!
//! Each driver wraps a real CLI coding agent (Claude Code, Codex, Kimi,
//! OpenCode) in its **non-interactive mode**: one process per task, prompt in,
//! repository edited on disk, result out, process exits.
//!
//! # Why this replaces the `drivers` module's shape
//!
//! [`crate::drivers`] models a worker as a long-lived interactive subprocess
//! that is fed over stdin and watched for a `TASK_COMPLETE` sentinel on
//! stdout. No shipped agent CLI behaves that way — none of them emit such a
//! sentinel, so that read loop would block until the process died. Every one
//! of them *does* have a proper one-shot mode, which is what this module uses.
//!
//! Verified invocations (from each CLI's own `--help`):
//!
//! | Agent       | Invocation                                                        |
//! |-------------|-------------------------------------------------------------------|
//! | Claude Code | `claude -p <prompt> --output-format json --permission-mode ...`   |
//! | Codex       | `codex exec <prompt> -C <dir> --sandbox ... --skip-git-repo-check`|
//! | Kimi        | `kimi --print -w <dir> -p <prompt>` (print implies `--yolo`)      |
//! | OpenCode    | `opencode run <prompt>` (cwd = workspace)                         |
//!
//! # Security
//!
//! These agents are launched with their approval prompts disabled — that is
//! the only way to run them unattended. They can write files and execute
//! commands inside the workspace. Codex is additionally confined by its own
//! `--sandbox workspace-write`; the others rely on the workspace directory
//! boundary alone. Do not point this at a repository you are not willing to
//! let an agent modify.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, warn};

use argus_core::worker_types::WorkerType;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// Maximum bytes of agent stdout/stderr retained.
const OUTPUT_LIMIT: usize = 16_000;

/// What the agent is being asked to do.
#[derive(Clone, Debug)]
pub struct AgentTask {
    /// Full natural-language instruction for the agent.
    pub prompt: String,
    /// Repository the agent works in.
    pub workspace: PathBuf,
    /// Hard wall-clock limit; the process is killed when it elapses.
    pub timeout: Duration,
    /// Model override (e.g. `haiku` for cheap sub-agents). `None` = CLI default.
    pub model: Option<String>,
}

/// What actually happened.
#[derive(Clone, Debug, Default)]
pub struct AgentRun {
    pub success: bool,
    pub exit_code: Option<i32>,
    /// The agent's final message, when the CLI reports one distinctly.
    pub final_message: String,
    pub stdout: String,
    pub stderr: String,
    /// Reported cost in USD, when the CLI provides it.
    pub cost_usd: Option<f64>,
    pub tokens_input: Option<u64>,
    pub tokens_output: Option<u64>,
    pub num_turns: Option<u64>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Set when `success` is false.
    pub failure_reason: Option<String>,
}

impl AgentRun {
    fn failure(reason: impl Into<String>) -> Self {
        Self {
            success: false,
            failure_reason: Some(reason.into()),
            ..Default::default()
        }
    }
}

/// A coding agent that can be invoked once to perform a task.
#[async_trait]
pub trait AgentDriver: Send + Sync {
    fn worker_type(&self) -> WorkerType;

    fn executor_type(&self) -> &'static str {
        match self.worker_type() {
            WorkerType::GenericCli => "codex",
            WorkerType::ClaudeCode => "claude_code",
            WorkerType::Aider => "kimi",
            WorkerType::OpenCode => "opencode",
        }
    }

    /// Executable this driver invokes.
    fn binary(&self) -> &str;

    /// Whether the binary is resolvable on `PATH`.
    async fn is_available(&self) -> bool {
        which(self.binary()).is_some()
    }

    async fn run(&self, task: &AgentTask) -> AgentRun;
}

/// Resolve a binary name through `PATH`.
pub fn which(binary: &str) -> Option<PathBuf> {
    if binary.contains('/') {
        let p = PathBuf::from(binary);
        return p.is_file().then_some(p);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(binary))
            .find(|c| c.is_file())
    })
}

/// Build a driver for a worker type.
///
/// `binary_override` replaces the default executable name — for agents
/// installed off `PATH`, and for tests that substitute a stub.
pub fn driver_for(
    worker_type: WorkerType,
    binary_override: Option<String>,
) -> Box<dyn AgentDriver> {
    match worker_type {
        WorkerType::ClaudeCode => {
            let mut d = ClaudeCodeDriver::default();
            if let Some(b) = binary_override {
                d.binary = b;
            }
            Box::new(d)
        }
        WorkerType::OpenCode => {
            let mut d = OpenCodeDriver::default();
            if let Some(b) = binary_override {
                d.binary = b;
            }
            Box::new(d)
        }
        // `Aider` is the enum's slot for "some other CLI agent"; Kimi occupies
        // it here because Aider is not installed and Kimi is. Renaming the
        // enum variant would touch the database `worker_type` type, which is
        // out of scope for this slice.
        WorkerType::Aider => {
            let mut d = KimiDriver::default();
            if let Some(b) = binary_override {
                d.binary = b;
            }
            Box::new(d)
        }
        WorkerType::GenericCli => {
            let mut d = CodexDriver::default();
            if let Some(b) = binary_override {
                d.binary = b;
            }
            Box::new(d)
        }
    }
}

pub async fn check_agent_startup(driver: &dyn AgentDriver) -> Result<String, String> {
    let label = format!("{} binary {}", driver.executor_type(), driver.binary());
    let path = which(driver.binary()).ok_or_else(|| format!("{label} not found"))?;
    let label = format!("{} binary {}", driver.executor_type(), path.display());
    let mut command = Command::new(&path);
    command.arg("--version");
    let output = capture(command, Duration::from_secs(5), &label).await?;
    let version = output.stdout.trim();
    if output.exit_code != Some(0)
        || !version
            .split_whitespace()
            .any(|word| word.contains('.') && word.chars().any(|c| c.is_ascii_digit()))
    {
        return Err(format!(
            "{label} did not return a version with exit status 0"
        ));
    }
    Ok(version.to_owned())
}

/// Raw process outcome.
#[derive(Debug)]
struct Captured {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Spawn a command, enforce a hard timeout, capture both streams.
async fn capture(mut cmd: Command, timeout: Duration, label: &str) -> Result<Captured, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Agents otherwise try to render TUI escape codes into the pipe.
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.as_std_mut().process_group(0);

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn {label}: {e}"))?;
    #[cfg(unix)]
    let _group = argus_core::quality::ProcessGroupGuard(child.id());

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(Captured {
            exit_code: output.status.code(),
            stdout: truncate(&String::from_utf8_lossy(&output.stdout)),
            stderr: truncate(&String::from_utf8_lossy(&output.stderr)),
        }),
        Ok(Err(e)) => Err(format!("{label} failed while running: {e}")),
        Err(_) => Err(format!(
            "{label} exceeded the task timeout of {}s and was killed",
            timeout.as_secs()
        )),
    }
}

fn truncate(raw: &str) -> String {
    if raw.len() <= OUTPUT_LIMIT {
        return raw.to_string();
    }
    let start = (raw.len() - OUTPUT_LIMIT..raw.len())
        .find(|i| raw.is_char_boundary(*i))
        .unwrap_or(raw.len());
    format!("...[truncated]\n{}", &raw[start..])
}

// ============================================================
// Claude Code
// ============================================================

/// Claude Code via `claude -p ... --output-format json`.
///
/// The only driver here whose CLI returns a structured result object, so it is
/// also the only one that can report cost and token counts precisely.
pub struct ClaudeCodeDriver {
    binary: String,
    permission_mode: String,
}

impl Default for ClaudeCodeDriver {
    fn default() -> Self {
        Self {
            binary: "claude".to_string(),
            // Lets the agent edit files without prompting, without granting
            // the blanket bypass that `--dangerously-skip-permissions` does.
            permission_mode: "acceptEdits".to_string(),
        }
    }
}

impl ClaudeCodeDriver {
    pub fn with_permission_mode(mut self, mode: impl Into<String>) -> Self {
        self.permission_mode = mode.into();
        self
    }

    /// Parse the `--output-format json` result object.
    fn parse(stdout: &str) -> Option<AgentRun> {
        // The result object is the last JSON line the CLI prints.
        let value: serde_json::Value = stdout
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with('{'))
            .and_then(|l| serde_json::from_str(l).ok())?;

        let is_error = value["is_error"].as_bool().unwrap_or(false);
        let subtype = value["subtype"].as_str().unwrap_or_default();
        let usage = &value["usage"];

        Some(AgentRun {
            success: !is_error && subtype == "success",
            exit_code: None,
            final_message: value["result"].as_str().unwrap_or_default().to_string(),
            stdout: String::new(),
            stderr: String::new(),
            cost_usd: value["total_cost_usd"].as_f64(),
            tokens_input: usage["input_tokens"].as_u64(),
            tokens_output: usage["output_tokens"].as_u64(),
            num_turns: value["num_turns"].as_u64(),
            session_id: value["session_id"].as_str().map(str::to_string),
            model: value["modelUsage"]
                .as_object()
                .and_then(|m| m.keys().next().cloned()),
            failure_reason: (is_error || subtype != "success").then(|| {
                let api_err = value["api_error_status"].as_str().unwrap_or("");
                let reason = value["terminal_reason"].as_str().unwrap_or(subtype);
                if api_err.is_empty() {
                    format!("claude reported failure: {reason}")
                } else {
                    format!("claude reported failure: {reason} (api status {api_err})")
                }
            }),
        })
    }
}

#[async_trait]
impl AgentDriver for ClaudeCodeDriver {
    fn worker_type(&self) -> WorkerType {
        WorkerType::ClaudeCode
    }
    fn binary(&self) -> &str {
        &self.binary
    }

    async fn run(&self, task: &AgentTask) -> AgentRun {
        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(&task.workspace)
            .arg("-p")
            .arg(&task.prompt)
            .arg("--output-format")
            .arg("json")
            .arg("--permission-mode")
            .arg(&self.permission_mode);
        if let Some(model) = &task.model {
            cmd.arg("--model").arg(model);
        }

        let captured = match capture(cmd, task.timeout, "claude").await {
            Ok(c) => c,
            Err(e) => return AgentRun::failure(e),
        };

        let mut run = ClaudeCodeDriver::parse(&captured.stdout).unwrap_or_else(|| {
            AgentRun::failure(format!(
                "could not parse claude JSON result (exit {:?})",
                captured.exit_code
            ))
        });
        run.exit_code = captured.exit_code;
        run.stdout = captured.stdout;
        run.stderr = captured.stderr;
        // A non-zero exit overrides a parsed "success".
        if captured.exit_code.is_some_and(|c| c != 0) {
            run.success = false;
            run.failure_reason.get_or_insert_with(|| {
                format!("claude exited with status {:?}", captured.exit_code)
            });
        }
        run
    }
}

// ============================================================
// Codex
// ============================================================

/// Codex via `codex exec`, confined by Codex's own sandbox.
pub struct CodexDriver {
    binary: String,
    sandbox: String,
}

impl Default for CodexDriver {
    fn default() -> Self {
        Self {
            binary: "codex".to_string(),
            // Codex may write inside the workspace but not outside it.
            sandbox: "workspace-write".to_string(),
        }
    }
}

impl CodexDriver {
    pub fn with_sandbox(mut self, sandbox: impl Into<String>) -> Self {
        self.sandbox = sandbox.into();
        self
    }
}

#[async_trait]
impl AgentDriver for CodexDriver {
    fn worker_type(&self) -> WorkerType {
        WorkerType::GenericCli
    }
    fn binary(&self) -> &str {
        &self.binary
    }

    async fn run(&self, task: &AgentTask) -> AgentRun {
        // `-o` gives us the agent's final message without parsing the JSONL.
        let last_message = task.workspace.join(".argus-codex-last-message");
        let _ = tokio::fs::remove_file(&last_message).await;

        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(&task.workspace)
            .arg("exec")
            .arg(&task.prompt)
            .arg("-C")
            .arg(&task.workspace)
            .arg("--sandbox")
            .arg(&self.sandbox)
            .arg("--skip-git-repo-check")
            .arg("-o")
            .arg(&last_message);
        if let Some(model) = &task.model {
            cmd.arg("-m").arg(model);
        }

        let captured = match capture(cmd, task.timeout, "codex").await {
            Ok(c) => c,
            Err(e) => return AgentRun::failure(e),
        };

        let final_message = tokio::fs::read_to_string(&last_message)
            .await
            .unwrap_or_default();
        let _ = tokio::fs::remove_file(&last_message).await;

        let success = captured.exit_code == Some(0);
        AgentRun {
            success,
            exit_code: captured.exit_code,
            final_message: final_message.trim().to_string(),
            failure_reason: (!success)
                .then(|| format!("codex exited with status {:?}", captured.exit_code)),
            model: task.model.clone(),
            stdout: captured.stdout,
            stderr: captured.stderr,
            ..Default::default()
        }
    }
}

// ============================================================
// Kimi
// ============================================================

/// Kimi via `kimi --print` (print mode implies `--yolo`).
pub struct KimiDriver {
    binary: String,
}

impl Default for KimiDriver {
    fn default() -> Self {
        Self {
            binary: "kimi".to_string(),
        }
    }
}

#[async_trait]
impl AgentDriver for KimiDriver {
    fn worker_type(&self) -> WorkerType {
        WorkerType::Aider
    }
    fn binary(&self) -> &str {
        &self.binary
    }

    async fn run(&self, task: &AgentTask) -> AgentRun {
        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(&task.workspace)
            .arg("--print")
            .arg("-w")
            .arg(&task.workspace)
            .arg("-p")
            .arg(&task.prompt);
        if let Some(model) = &task.model {
            cmd.arg("-m").arg(model);
        }

        let captured = match capture(cmd, task.timeout, "kimi").await {
            Ok(c) => c,
            Err(e) => return AgentRun::failure(e),
        };

        let success = captured.exit_code == Some(0);
        AgentRun {
            success,
            exit_code: captured.exit_code,
            final_message: captured.stdout.trim().to_string(),
            failure_reason: (!success)
                .then(|| format!("kimi exited with status {:?}", captured.exit_code)),
            model: task.model.clone(),
            stdout: captured.stdout,
            stderr: captured.stderr,
            ..Default::default()
        }
    }
}

// ============================================================
// OpenCode
// ============================================================

/// OpenCode via `opencode run`.
pub struct OpenCodeDriver {
    binary: String,
}

impl Default for OpenCodeDriver {
    fn default() -> Self {
        Self {
            binary: "opencode".to_string(),
        }
    }
}

#[async_trait]
impl AgentDriver for OpenCodeDriver {
    fn worker_type(&self) -> WorkerType {
        WorkerType::OpenCode
    }
    fn binary(&self) -> &str {
        &self.binary
    }

    async fn run(&self, task: &AgentTask) -> AgentRun {
        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(&task.workspace).arg("run");
        if let Some(model) = &task.model {
            cmd.arg("--model").arg(model);
        }
        cmd.arg(&task.prompt);

        let captured = match capture(cmd, task.timeout, "opencode").await {
            Ok(c) => c,
            Err(e) => return AgentRun::failure(e),
        };

        let success = captured.exit_code == Some(0);
        AgentRun {
            success,
            exit_code: captured.exit_code,
            final_message: captured.stdout.trim().to_string(),
            failure_reason: (!success)
                .then(|| format!("opencode exited with status {:?}", captured.exit_code)),
            model: task.model.clone(),
            stdout: captured.stdout,
            stderr: captured.stderr,
            ..Default::default()
        }
    }
}

// ============================================================
// Test / stub driver
// ============================================================

/// Runs an arbitrary executable as if it were an agent, passing the prompt as
/// `$1` and the workspace as cwd.
///
/// This exists so the agent pipeline can be tested end-to-end — real repo
/// edits, real exit codes, real git diffs — without spending API credits.
/// It is never selected by [`driver_for`]; tests construct it explicitly.
pub struct StubAgentDriver {
    binary: String,
}

impl StubAgentDriver {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }
}

#[async_trait]
impl AgentDriver for StubAgentDriver {
    fn executor_type(&self) -> &'static str {
        "stub"
    }
    fn worker_type(&self) -> WorkerType {
        WorkerType::GenericCli
    }
    fn binary(&self) -> &str {
        &self.binary
    }

    async fn run(&self, task: &AgentTask) -> AgentRun {
        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(&task.workspace).arg(&task.prompt);

        let captured = match capture(cmd, task.timeout, "stub-agent").await {
            Ok(c) => c,
            Err(e) => return AgentRun::failure(e),
        };
        let success = captured.exit_code == Some(0);
        debug!(success, exit = ?captured.exit_code, "stub agent finished");
        AgentRun {
            success,
            exit_code: captured.exit_code,
            final_message: captured.stdout.trim().to_string(),
            failure_reason: (!success)
                .then(|| format!("stub agent exited with status {:?}", captured.exit_code)),
            stdout: captured.stdout,
            stderr: captured.stderr,
            ..Default::default()
        }
    }
}

// ============================================================
// Availability
// ============================================================

/// Report which agent binaries are present, for startup logging.
pub async fn available_agents() -> Vec<(WorkerType, bool)> {
    let mut out = Vec::new();
    for wt in [
        WorkerType::ClaudeCode,
        WorkerType::GenericCli,
        WorkerType::Aider,
        WorkerType::OpenCode,
    ] {
        let driver = driver_for(wt, None);
        let ok = driver.is_available().await;
        if !ok {
            warn!(worker_type = %wt, binary = driver.binary(), "agent binary not found on PATH");
        }
        out.push((wt, ok));
    }
    out
}

/// Whether `dir` is inside a git work tree.
pub fn is_git_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("argus-process-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("stub.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn test_startup_missing_binary_reports_executor_and_path() {
        let dir = Scratch::new();
        let path = dir.0.join("missing-codex").display().to_string();
        let driver = driver_for(WorkerType::GenericCli, Some(path.clone()));
        let error = check_agent_startup(driver.as_ref()).await.unwrap_err();
        assert!(error.contains("codex"));
        assert!(error.contains(&path));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_startup_override_only_requests_version() {
        let dir = Scratch::new();
        let path = script(
            &dir.0,
            "[ \"$1\" = --version ] || exit 99\nprintf 'stub 1.2.3\\n'",
        );
        let driver = driver_for(WorkerType::GenericCli, Some(path.display().to_string()));
        assert_eq!(
            check_agent_startup(driver.as_ref()).await.unwrap(),
            "stub 1.2.3"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_startup_rejects_invalid_or_failed_version() {
        let dir = Scratch::new();
        for body in ["printf 'no version\\n'", "printf 'stub 1.2.3\\n'; exit 1"] {
            let path = script(&dir.0, body);
            let driver = driver_for(WorkerType::ClaudeCode, Some(path.display().to_string()));
            let error = check_agent_startup(driver.as_ref()).await.unwrap_err();
            assert!(error.contains("claude_code"));
            assert!(error.contains(&path.display().to_string()));
        }
    }

    #[cfg(target_os = "linux")]
    async fn assert_grandchild_terminated(cancel: bool) {
        let dir = Scratch::new();
        let pid_file = dir.0.join("pid");
        let path = script(&dir.0, "sleep 30 &\nprintf '%s\\n' \"$!\" > pid\nwait");
        let mut cmd = Command::new(path);
        cmd.current_dir(&dir.0);
        let job =
            tokio::spawn(async move { capture(cmd, Duration::from_secs(1), "forking-stub").await });
        let pid = loop {
            if let Ok(text) = tokio::fs::read_to_string(&pid_file).await {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    break pid;
                }
            }
            assert!(!job.is_finished(), "stub must fork before finishing");
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let running = || {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|text| {
                text.rsplit_once(") ")
                    .is_some_and(|(_, tail)| !tail.starts_with('Z'))
            })
        };
        assert!(running(), "grandchild was running before termination");
        if cancel {
            job.abort();
            assert!(job.await.unwrap_err().is_cancelled());
        } else {
            assert!(
                job.await
                    .unwrap()
                    .unwrap_err()
                    .contains("exceeded the task timeout")
            );
        }
        for _ in 0..100 {
            if !running() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("grandchild {pid} survived termination");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_agent_timeout_kills_grandchild() {
        assert_grandchild_terminated(false).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_agent_cancellation_kills_grandchild() {
        assert_grandchild_terminated(true).await;
    }

    #[test]
    fn test_claude_parse_success() {
        let stdout = r#"{"is_error":false,"subtype":"success","result":"Created hello.txt","total_cost_usd":0.081,"num_turns":2,"session_id":"abc-123","usage":{"input_tokens":4,"output_tokens":163},"modelUsage":{"claude-opus-5":{}},"terminal_reason":"completed","type":"result"}"#;
        let run = ClaudeCodeDriver::parse(stdout).expect("parses");
        assert!(run.success);
        assert_eq!(run.final_message, "Created hello.txt");
        assert_eq!(run.cost_usd, Some(0.081));
        assert_eq!(run.tokens_input, Some(4));
        assert_eq!(run.tokens_output, Some(163));
        assert_eq!(run.num_turns, Some(2));
        assert_eq!(run.session_id.as_deref(), Some("abc-123"));
        assert_eq!(run.model.as_deref(), Some("claude-opus-5"));
        assert!(run.failure_reason.is_none());
    }

    #[test]
    fn test_claude_parse_error() {
        let stdout = r#"{"is_error":true,"subtype":"error_during_execution","result":"","terminal_reason":"error","api_error_status":"529","type":"result"}"#;
        let run = ClaudeCodeDriver::parse(stdout).expect("parses");
        assert!(!run.success);
        let reason = run.failure_reason.expect("reason");
        assert!(reason.contains("529"), "got: {reason}");
    }

    #[test]
    fn test_claude_parse_picks_last_json_line() {
        let stdout = "{\"type\":\"system\"}\nnoise\n{\"is_error\":false,\"subtype\":\"success\",\"result\":\"done\",\"type\":\"result\"}";
        let run = ClaudeCodeDriver::parse(stdout).expect("parses");
        assert!(run.success);
        assert_eq!(run.final_message, "done");
    }

    #[test]
    fn test_claude_parse_rejects_garbage() {
        assert!(ClaudeCodeDriver::parse("not json at all").is_none());
    }

    #[test]
    fn test_driver_for_maps_every_worker_type() {
        assert_eq!(driver_for(WorkerType::ClaudeCode, None).binary(), "claude");
        assert_eq!(driver_for(WorkerType::GenericCli, None).binary(), "codex");
        assert_eq!(driver_for(WorkerType::Aider, None).binary(), "kimi");
        assert_eq!(driver_for(WorkerType::OpenCode, None).binary(), "opencode");
        assert_eq!(
            driver_for(WorkerType::ClaudeCode, Some("/opt/claude".to_string())).binary(),
            "/opt/claude"
        );
    }

    #[test]
    fn test_which_resolves_and_rejects() {
        assert!(which("sh").is_some());
        assert!(which("argus-definitely-not-a-real-binary").is_none());
    }

    #[test]
    fn test_truncate_keeps_tail() {
        let long = "a".repeat(OUTPUT_LIMIT + 100);
        let t = truncate(&long);
        assert!(t.starts_with("...[truncated]"));
        assert!(t.len() < long.len());
        assert_eq!(truncate("short"), "short");
    }

    #[tokio::test]
    async fn test_capture_enforces_timeout() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 30");
        let err = capture(cmd, Duration::from_millis(200), "sleeper")
            .await
            .expect_err("must time out");
        assert!(err.contains("exceeded the task timeout"), "got: {err}");
    }

    #[tokio::test]
    async fn test_capture_reports_exit_and_streams() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo out; echo err >&2; exit 5");
        let c = capture(cmd, Duration::from_secs(10), "t").await.unwrap();
        assert_eq!(c.exit_code, Some(5));
        assert!(c.stdout.contains("out"));
        assert!(c.stderr.contains("err"));
    }
}
