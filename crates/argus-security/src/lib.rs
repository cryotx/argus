//! argus-security: Key vault, RBAC, audit logging, command guard, secret scanner

pub mod audit;
pub mod command_guard;
pub mod guardrails;
pub mod key_vault;
pub mod rbac;
pub mod secret_scanner;

#[cfg(test)]
mod tests {
    use argus_core::types::{AuditOutcome, Role};

    use crate::audit::AuditLogger;
    use crate::command_guard::CommandSafetyGuard;
    use crate::guardrails::GuardrailEngine;
    use crate::key_vault::{InMemoryKeyVault, KeyVault};
    use crate::rbac::{Permission, RbacPolicyEngine};
    use crate::secret_scanner::SecretScanner;

    #[test]
    fn test_smoke_rbac_guard() {
        let rbac = RbacPolicyEngine::new();
        assert!(
            rbac.check_roles(&[Role::Developer], Permission::ReadProject)
                .allowed
        );
        assert!(
            !rbac
                .check_roles(&[Role::Viewer], Permission::ExecuteCommand)
                .allowed
        );
    }

    #[test]
    fn test_smoke_guardrails() {
        let engine = GuardrailEngine::new();
        let report = engine.check_paths(["crates/argus-security/src/rbac.rs"]);
        assert!(!report.passed);
    }

    #[test]
    fn test_smoke_audit_chain() {
        let logger = AuditLogger::new();
        logger
            .append(
                "owner",
                "run_task",
                AuditOutcome::Success,
                Some("project-a"),
                None,
            )
            .unwrap();
        logger
            .append(
                "owner",
                "deploy",
                AuditOutcome::Warned,
                None::<String>,
                None,
            )
            .unwrap();
        assert!(logger.verify_chain().unwrap());
    }

    #[test]
    fn test_smoke_command_guard() {
        let guard = CommandSafetyGuard::new();
        assert!(guard.assess("cargo test").allowed);
        assert!(!guard.assess("rm -rf /").allowed);
    }

    #[test]
    fn test_smoke_secret_scanner() {
        let scanner = SecretScanner::new();
        let findings = scanner.scan_text("OPENAI_API_KEY=sk-1234567890abcdefghijklmnop");
        assert!(!findings.is_empty());
    }

    #[test]
    fn test_smoke_key_vault() {
        let vault = InMemoryKeyVault::new();
        vault.put("db_password", "pass-123").unwrap();
        assert_eq!(
            vault.get("db_password").unwrap(),
            Some("pass-123".to_string())
        );
    }
}
