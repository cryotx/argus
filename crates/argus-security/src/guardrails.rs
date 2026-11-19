use argus_core::types::{ArgusError, ArgusResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailViolation {
    pub rule_id: String,
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardrailReport {
    pub passed: bool,
    pub violations: Vec<GuardrailViolation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImmutableRuleSet {
    pub protected_paths: Vec<String>,
    pub forbidden_content_markers: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GuardrailEngine {
    rules: ImmutableRuleSet,
}

impl GuardrailEngine {
    pub fn new() -> Self {
        Self {
            rules: ImmutableRuleSet {
                protected_paths: vec![
                    "crates/argus-security/src/rbac.rs".to_string(),
                    "crates/argus-security/src/guardrails.rs".to_string(),
                    "crates/argus-security/src/audit.rs".to_string(),
                    "crates/argus-security/src/key_vault.rs".to_string(),
                    "config/argus.default.toml".to_string(),
                ],
                forbidden_content_markers: vec![
                    "ALLOW_ALL_SECURITY_BYPASS".to_string(),
                    "DISABLE_GUARDRAILS".to_string(),
                    "SKIP_AUDIT_CHAIN_VERIFY".to_string(),
                ],
            },
        }
    }

    pub fn with_rules(rules: ImmutableRuleSet) -> Self {
        Self { rules }
    }

    pub fn with_additions<I1, S1, I2, S2>(
        additional_protected_paths: I1,
        additional_forbidden_markers: I2,
    ) -> Self
    where
        I1: IntoIterator<Item = S1>,
        S1: Into<String>,
        I2: IntoIterator<Item = S2>,
        S2: Into<String>,
    {
        let mut engine = Self::new();

        for path in additional_protected_paths {
            let path = path.into();
            if !path.trim().is_empty() {
                engine.rules.protected_paths.push(path);
            }
        }

        for marker in additional_forbidden_markers {
            let marker = marker.into();
            if !marker.trim().is_empty() {
                engine.rules.forbidden_content_markers.push(marker);
            }
        }

        engine
    }

    pub fn rules(&self) -> &ImmutableRuleSet {
        &self.rules
    }

    pub fn check_paths<I, S>(&self, changed_paths: I) -> GuardrailReport
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut violations = Vec::new();

        for path in changed_paths {
            let path = path.as_ref();
            if self.is_protected(path) {
                violations.push(GuardrailViolation {
                    rule_id: "immutable_protected_path".to_string(),
                    path: Some(path.to_string()),
                    message: format!("modification blocked for protected path: {path}"),
                });
            }
        }

        GuardrailReport {
            passed: violations.is_empty(),
            violations,
        }
    }

    pub fn check_content(&self, path: &str, content: &str) -> GuardrailReport {
        let mut violations = Vec::new();

        for marker in &self.rules.forbidden_content_markers {
            if content.contains(marker) {
                violations.push(GuardrailViolation {
                    rule_id: "forbidden_marker".to_string(),
                    path: Some(path.to_string()),
                    message: format!("forbidden marker detected: {marker}"),
                });
            }
        }

        GuardrailReport {
            passed: violations.is_empty(),
            violations,
        }
    }

    pub fn enforce_paths<I, S>(&self, changed_paths: I) -> ArgusResult<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let report = self.check_paths(changed_paths);
        if report.passed {
            Ok(())
        } else {
            let messages = report
                .violations
                .into_iter()
                .map(|v| v.message)
                .collect::<Vec<_>>()
                .join("; ");
            Err(ArgusError::Security(messages))
        }
    }

    pub fn enforce_content(&self, path: &str, content: &str) -> ArgusResult<()> {
        let report = self.check_content(path, content);
        if report.passed {
            Ok(())
        } else {
            let messages = report
                .violations
                .into_iter()
                .map(|v| v.message)
                .collect::<Vec<_>>()
                .join("; ");
            Err(ArgusError::Security(messages))
        }
    }

    fn is_protected(&self, path: &str) -> bool {
        let normalized_path = normalize_path(path);
        self.rules.protected_paths.iter().any(|protected| {
            let normalized_protected = normalize_path(protected);
            normalized_path == normalized_protected
                || normalized_path
                    .strip_prefix(&normalized_protected)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    }
}

fn normalize_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let mut stack: Vec<&str> = Vec::new();

    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if stack.last().is_some_and(|segment| *segment != "..") {
                    stack.pop();
                } else {
                    stack.push("..");
                }
            }
            _ => stack.push(part),
        }
    }

    stack.join("/")
}

impl Default for GuardrailEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protected_path_blocked() {
        let engine = GuardrailEngine::new();
        let report = engine.check_paths(["crates/argus-security/src/guardrails.rs"]);
        assert!(!report.passed);
        assert_eq!(report.violations.len(), 1);
    }

    #[test]
    fn test_unprotected_path_allowed() {
        let engine = GuardrailEngine::new();
        let report = engine.check_paths(["crates/argus-core/src/mvp.rs"]);
        assert!(report.passed);
    }

    #[test]
    fn test_forbidden_marker_blocked() {
        let engine = GuardrailEngine::new();
        let report = engine.check_content("x.rs", "const X: &str = \"DISABLE_GUARDRAILS\";");
        assert!(!report.passed);
    }

    #[test]
    fn test_enforce_content_err() {
        let engine = GuardrailEngine::new();
        let err = engine
            .enforce_content("x.rs", "ALLOW_ALL_SECURITY_BYPASS")
            .unwrap_err();
        assert!(format!("{err}").contains("forbidden marker detected"));
    }

    #[test]
    fn test_additional_rules_are_applied() {
        let engine = GuardrailEngine::with_additions(["tmp/locked.txt"], ["CUSTOM_FORBIDDEN"]);

        let path_report = engine.check_paths(["tmp/locked.txt"]);
        assert!(!path_report.passed);

        let content_report = engine.check_content("x", "CUSTOM_FORBIDDEN");
        assert!(!content_report.passed);
    }

    #[test]
    fn test_protected_path_normalization_blocks_bypass_variants() {
        let engine = GuardrailEngine::new();

        let report_a = engine.check_paths(["./crates/argus-security/src/rbac.rs"]);
        assert!(!report_a.passed);

        let report_b = engine.check_paths(["crates/argus-security/src/../src/rbac.rs"]);
        assert!(!report_b.passed);
    }
}
