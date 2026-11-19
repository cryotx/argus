use argus_core::types::{ArgusError, ArgusResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandAssessment {
    pub allowed: bool,
    pub reason: String,
    pub violations: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CommandSafetyGuard {
    denylist_tokens: Vec<String>,
    denylist_substrings: Vec<String>,
}

impl CommandSafetyGuard {
    pub fn new() -> Self {
        Self {
            denylist_tokens: vec![
                "rm -rf /".to_string(),
                "mkfs".to_string(),
                "dd if=".to_string(),
                "shutdown".to_string(),
                "reboot".to_string(),
                "poweroff".to_string(),
                "chown -R /".to_string(),
                "chmod 777 /".to_string(),
                ":(){ :|:& };:".to_string(),
            ],
            denylist_substrings: vec![
                "curl | sh".to_string(),
                "wget | sh".to_string(),
                "`".to_string(),
                "$(".to_string(),
                "> /dev/sd".to_string(),
                ">/dev/sd".to_string(),
            ],
        }
    }

    pub fn with_additional_denied_tokens<I, S>(additional_tokens: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut guard = Self::new();
        for token in additional_tokens {
            let token = token.into();
            if !token.trim().is_empty() {
                guard.denylist_tokens.push(token);
            }
        }
        guard
    }

    pub fn assess(&self, command: &str) -> CommandAssessment {
        let cmd = command.trim();
        if cmd.is_empty() {
            return CommandAssessment {
                allowed: false,
                reason: "empty command".to_string(),
                violations: vec!["command is empty".to_string()],
            };
        }

        let normalized = normalize(cmd);
        let mut violations = Vec::new();

        for token in &self.denylist_tokens {
            if normalized.contains(&normalize(token)) {
                violations.push(format!("denied token matched: {token}"));
            }
        }

        for pattern in &self.denylist_substrings {
            if normalized.contains(&normalize(pattern)) {
                violations.push(format!("denied pattern matched: {pattern}"));
            }
        }

        if normalized.contains("sudo ") {
            violations.push("sudo elevation is blocked".to_string());
        }

        if violations.is_empty() {
            CommandAssessment {
                allowed: true,
                reason: "command passed safety checks".to_string(),
                violations,
            }
        } else {
            CommandAssessment {
                allowed: false,
                reason: violations.join("; "),
                violations,
            }
        }
    }

    pub fn enforce(&self, command: &str) -> ArgusResult<()> {
        let result = self.assess(command);
        if result.allowed {
            Ok(())
        } else {
            Err(ArgusError::Security(result.reason))
        }
    }
}

impl Default for CommandSafetyGuard {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize(s: &str) -> String {
    s.to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allow_safe_command() {
        let guard = CommandSafetyGuard::new();
        let result = guard.assess("cargo test -p argus-security");
        assert!(result.allowed);
    }

    #[test]
    fn test_block_rm_rf_root() {
        let guard = CommandSafetyGuard::new();
        let result = guard.assess("rm -rf /");
        assert!(!result.allowed);
    }

    #[test]
    fn test_block_subshell_pattern() {
        let guard = CommandSafetyGuard::new();
        let result = guard.assess("echo $(cat secret)");
        assert!(!result.allowed);
    }

    #[test]
    fn test_enforce_returns_error() {
        let guard = CommandSafetyGuard::new();
        let err = guard.enforce("sudo reboot").unwrap_err();
        assert!(format!("{err}").contains("Security error"));
    }

    #[test]
    fn test_additional_tokens_blocked() {
        let guard = CommandSafetyGuard::with_additional_denied_tokens(["cargo build"]);
        let result = guard.assess("cargo build");
        assert!(!result.allowed);
    }
}
