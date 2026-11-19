use serde::{Deserialize, Serialize};

use argus_core::types::TrustLevel;

use crate::client::McpServerConfig;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditVerdict {
    Allow,
    Warn,
    Deny,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditReport {
    pub verdict: AuditVerdict,
    pub trust_level: TrustLevel,
    pub reasons: Vec<String>,
}

pub struct SecurityAuditor;

impl SecurityAuditor {
    pub fn new() -> Self {
        Self
    }

    pub fn audit_server(&self, config: &McpServerConfig) -> AuditReport {
        // P8 heuristic audit placeholder.
        if config.transport == crate::client::McpTransport::Stdio && config.command.is_some() {
            AuditReport {
                verdict: AuditVerdict::Allow,
                trust_level: TrustLevel::ThirdParty,
                reasons: vec!["basic static checks passed".to_string()],
            }
        } else {
            AuditReport {
                verdict: AuditVerdict::Warn,
                trust_level: TrustLevel::Untrusted,
                reasons: vec!["non-stdio or missing command".to_string()],
            }
        }
    }
}

impl Default for SecurityAuditor {
    fn default() -> Self {
        Self::new()
    }
}
