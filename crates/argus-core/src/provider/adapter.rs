//! Provider discovery adapters.
//!
//! Discovery is limited to safe, official signals: whether the CLI binary is
//! installed, its `--version`, and its own non-interactive auth-status command.
//! Authentication status is derived from those official commands (or from
//! ARGUS's own [`AuthProfile`] rows), never scraped from a browser, session
//! file, or token store. No model call is made here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use super::types::*;

/// Parser versions (§72) so a later CLI wording change can be told apart from
/// a historical reading.
pub const CLAUDE_AUTH_PARSER: &str = "claude-auth-status-parser-v1";
pub const CODEX_AUTH_PARSER: &str = "codex-login-status-parser-v1";

/// Resolve an allowlisted CLI binary by scanning `PATH` (no shell).
pub fn resolve_binary(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(binary);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Run an allowlisted binary with fixed arguments. Never through a shell, so no
/// caller-supplied string can become a command.
async fn run_quiet(path: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.args(args);
    let output = tokio::time::timeout(Duration::from_secs(5), cmd.output()).await;
    match output {
        Ok(Ok(out)) if out.status.success() => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            if text.trim().is_empty() {
                text = String::from_utf8_lossy(&out.stderr).to_string();
            }
            Some(text)
        }
        _ => None,
    }
}

/// The CLI's own non-interactive status command, when one is officially
/// supported. Absence means "we do not guess" — status stays `Unknown`.
fn auth_status_args(provider: RuntimeProvider) -> Option<(&'static [&'static str], &'static str)> {
    match provider {
        RuntimeProvider::ClaudeCodeCli => Some((&["auth", "status", "--json"], CLAUDE_AUTH_PARSER)),
        RuntimeProvider::CodexCli => Some((&["login", "status"], CODEX_AUTH_PARSER)),
        _ => None,
    }
}

/// Turn a status command's output into an [`AuthStatus`] + confidence.
///
/// Any failure to recognise the shape yields `Unknown` — never `Unavailable`
/// and never a fabricated `Available` (§23: a changed CLI format must not take
/// the scheduler down or be read as "logged out").
pub fn parse_auth_status(provider: RuntimeProvider, raw: &str) -> (AuthStatus, ParseConfidence) {
    match provider {
        // Official machine-readable JSON: the strongest signal available.
        RuntimeProvider::ClaudeCodeCli => {
            match serde_json::from_str::<serde_json::Value>(raw.trim()) {
                Ok(value) => match value.get("loggedIn").and_then(|v| v.as_bool()) {
                    Some(true) => (AuthStatus::Available, ParseConfidence::High),
                    Some(false) => (AuthStatus::NeedsLogin, ParseConfidence::High),
                    None => (AuthStatus::Unknown, ParseConfidence::Low),
                },
                Err(_) => (AuthStatus::Unknown, ParseConfidence::Unknown),
            }
        }
        // Human-readable but stable phrasing, e.g. "Logged in using ChatGPT".
        RuntimeProvider::CodexCli => {
            let text = raw.trim().to_ascii_lowercase();
            if text.starts_with("not logged in") || text.contains("no credentials") {
                (AuthStatus::NeedsLogin, ParseConfidence::Medium)
            } else if text.starts_with("logged in") || text.contains("logged in using") {
                (AuthStatus::Available, ParseConfidence::Medium)
            } else {
                (AuthStatus::Unknown, ParseConfidence::Low)
            }
        }
        _ => (AuthStatus::Unknown, ParseConfidence::Unknown),
    }
}

/// Discover a CLI runtime: installed?, version?, official auth status. Never
/// logs in, never reads credentials, never makes a model call.
pub async fn discover_cli(provider: RuntimeProvider) -> ProviderDiscovery {
    let Some(binary) = provider.cli_binary() else {
        return ProviderDiscovery {
            installed: false,
            note: Some("not a CLI provider".to_string()),
            checked_at: Some(chrono::Utc::now()),
            ..Default::default()
        };
    };
    let Some(path) = resolve_binary(binary) else {
        return ProviderDiscovery {
            installed: false,
            checked_at: Some(chrono::Utc::now()),
            note: Some(format!("{binary} not found on PATH")),
            ..Default::default()
        };
    };

    let (version, note) = match run_quiet(&path, &["--version"]).await {
        Some(text) => {
            let first = text.lines().next().unwrap_or("").trim().to_string();
            if first.is_empty() {
                (None, Some("`--version` produced no output".to_string()))
            } else {
                (Some(first), None)
            }
        }
        None => (
            None,
            Some("`--version` failed, exited non-zero, or timed out".to_string()),
        ),
    };

    // Auth status: the CLI's own official, non-interactive command only.
    let (auth_status, auth_confidence, auth_parser_version, auth_note) =
        match auth_status_args(provider) {
            Some((args, version_tag)) => match run_quiet(&path, args).await {
                Some(raw) => {
                    let (status, confidence) = parse_auth_status(provider, &raw);
                    (status, confidence, Some(version_tag.to_string()), None)
                }
                None => (
                    AuthStatus::Unknown,
                    ParseConfidence::Unknown,
                    Some(version_tag.to_string()),
                    Some("auth status command unavailable".to_string()),
                ),
            },
            None => (
                AuthStatus::Unknown,
                ParseConfidence::Unknown,
                None,
                Some("no official non-interactive status command".to_string()),
            ),
        };

    ProviderDiscovery {
        installed: true,
        version,
        path: Some(path.display().to_string()),
        checked_at: Some(chrono::Utc::now()),
        auth_status,
        auth_confidence,
        auth_parser_version,
        note: note.or(auth_note),
    }
}

#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn id(&self) -> RuntimeProvider;
    async fn discover(&self) -> ProviderDiscovery;
    async fn health_check(&self) -> ProviderHealth;
}

/// Real CLI discovery adapter (binary/version only).
pub struct CliProviderAdapter {
    provider: RuntimeProvider,
}

impl CliProviderAdapter {
    pub fn new(provider: RuntimeProvider) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl ProviderAdapter for CliProviderAdapter {
    fn id(&self) -> RuntimeProvider {
        self.provider
    }

    async fn discover(&self) -> ProviderDiscovery {
        discover_cli(self.provider).await
    }

    async fn health_check(&self) -> ProviderHealth {
        let discovery = discover_cli(self.provider).await;
        ProviderHealth {
            provider_id: Some(self.provider),
            status: Some(if discovery.installed {
                ProviderHealthStatus::Healthy
            } else {
                ProviderHealthStatus::Unavailable
            }),
            auth_status: Some(discovery.auth_status),
            checked_at: Some(chrono::Utc::now()),
            ..Default::default()
        }
    }
}

/// Deterministic fake adapter for tests — no subprocess, no network.
pub struct FakeProviderAdapter {
    provider: RuntimeProvider,
    discovery: ProviderDiscovery,
    health: ProviderHealth,
}

impl FakeProviderAdapter {
    pub fn new(provider: RuntimeProvider, installed: bool) -> Self {
        Self {
            provider,
            discovery: ProviderDiscovery {
                installed,
                version: Some("fake-1.0".to_string()),
                path: Some(format!("/fake/{provider}")),
                checked_at: Some(chrono::Utc::now()),
                auth_status: AuthStatus::Unknown,
                auth_confidence: ParseConfidence::Unknown,
                auth_parser_version: None,
                note: None,
            },
            health: ProviderHealth {
                provider_id: Some(provider),
                status: Some(if installed {
                    ProviderHealthStatus::Healthy
                } else {
                    ProviderHealthStatus::Unavailable
                }),
                checked_at: Some(chrono::Utc::now()),
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl ProviderAdapter for FakeProviderAdapter {
    fn id(&self) -> RuntimeProvider {
        self.provider
    }

    async fn discover(&self) -> ProviderDiscovery {
        self.discovery.clone()
    }

    async fn health_check(&self) -> ProviderHealth {
        self.health.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_discovery_is_deterministic() {
        let adapter = FakeProviderAdapter::new(RuntimeProvider::CodexCli, true);
        assert!(adapter.discover().await.installed);
        let adapter = FakeProviderAdapter::new(RuntimeProvider::CodexCli, false);
        assert!(!adapter.discover().await.installed);
    }

    #[test]
    fn resolve_rejects_shell_metacharacters() {
        assert!(resolve_binary("definitely-not-a-real-binary-xyz").is_none());
    }

    #[test]
    fn claude_machine_readable_status_is_high_confidence() {
        let (status, conf) =
            parse_auth_status(RuntimeProvider::ClaudeCodeCli, r#"{"loggedIn":true}"#);
        assert_eq!(status, AuthStatus::Available);
        assert_eq!(conf, ParseConfidence::High);
        let (status, _) =
            parse_auth_status(RuntimeProvider::ClaudeCodeCli, r#"{"loggedIn":false}"#);
        assert_eq!(status, AuthStatus::NeedsLogin);
    }

    /// §23/§98: a CLI that changes its output shape must degrade to `Unknown`,
    /// never panic and never be misread as logged-out or logged-in.
    #[test]
    fn changed_output_shape_degrades_to_unknown() {
        for raw in [
            "",
            "not json at all",
            "{ broken",
            r#"{"someNewField":"reworded"}"#,
            "usage: claude auth status [options]",
        ] {
            let (status, _) = parse_auth_status(RuntimeProvider::ClaudeCodeCli, raw);
            assert_eq!(
                status,
                AuthStatus::Unknown,
                "unparseable claude output must be Unknown, got {status:?} for {raw:?}"
            );
        }
        let (status, _) = parse_auth_status(RuntimeProvider::CodexCli, "entirely new wording 42");
        assert_eq!(status, AuthStatus::Unknown);
    }

    #[test]
    fn codex_text_status_is_medium_confidence() {
        let (status, conf) =
            parse_auth_status(RuntimeProvider::CodexCli, "Logged in using ChatGPT");
        assert_eq!(status, AuthStatus::Available);
        assert_eq!(conf, ParseConfidence::Medium);
        let (status, _) = parse_auth_status(RuntimeProvider::CodexCli, "Not logged in");
        assert_eq!(status, AuthStatus::NeedsLogin);
    }

    /// §60: the parser is a pure function of output to an enum — it never
    /// carries any part of the raw payload (which may hold an account id).
    #[test]
    fn parser_never_echoes_raw_output() {
        let secretish =
            r#"{"loggedIn":true,"email":"who@example.com","token":"sk-secret-example"}"#;
        let (status, conf) = parse_auth_status(RuntimeProvider::ClaudeCodeCli, secretish);
        let rendered = format!("{status:?}{conf:?}");
        assert!(!rendered.contains("sk-secret-example"));
        assert!(!rendered.contains("who@example.com"));
    }

    #[test]
    fn providers_without_an_official_status_command_are_not_guessed() {
        assert!(auth_status_args(RuntimeProvider::OpenCodeCli).is_none());
        assert!(auth_status_args(RuntimeProvider::OpenAiApi).is_none());
        let (status, conf) = parse_auth_status(
            RuntimeProvider::OpenCodeCli,
            "logged in? whatever the tool prints",
        );
        assert_eq!(status, AuthStatus::Unknown);
        assert_eq!(conf, ParseConfidence::Unknown);
    }
}
