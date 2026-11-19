use serde_json::Value;

use argus_core::config::SecurityConfig;
use argus_core::types::{ArgusError, ArgusResult, AuditOutcome, Role};
use argus_security::audit::AuditLogger;
use argus_security::rbac::{Permission, RbacPolicyEngine};

use crate::builtin::{database::DatabaseServer, filesystem::FilesystemServer, shell::ShellServer};
use crate::client::{McpClient, ToolResult};
use crate::registry::ServerRegistry;

pub struct ToolRouter {
    pub registry: ServerRegistry,
    pub filesystem: FilesystemServer,
    pub shell: ShellServer,
    pub database: DatabaseServer,
    pub rbac: RbacPolicyEngine,
    pub audit: AuditLogger,
}

impl ToolRouter {
    pub fn new(registry: ServerRegistry, root: impl Into<std::path::PathBuf>) -> Self {
        let root = root.into();
        Self {
            registry,
            filesystem: FilesystemServer::new(root),
            shell: ShellServer::new(),
            database: DatabaseServer::new(),
            rbac: RbacPolicyEngine::new(),
            audit: AuditLogger::new(),
        }
    }

    pub fn with_security_config(
        registry: ServerRegistry,
        root: impl Into<std::path::PathBuf>,
        security: &SecurityConfig,
    ) -> Self {
        let root = root.into();
        Self {
            registry,
            filesystem: FilesystemServer::with_security_overrides(
                root,
                security.protected_paths.clone(),
                security.forbidden_content_markers.clone(),
                security.secret_keywords.clone(),
            ),
            shell: ShellServer::with_blocked_commands(security.blocked_commands.clone()),
            database: DatabaseServer::new(),
            rbac: RbacPolicyEngine::new(),
            audit: AuditLogger::new(),
        }
    }

    pub async fn call_builtin(&self, tool_name: &str, args: Value) -> ArgusResult<ToolResult> {
        self.call_builtin_with_roles(tool_name, args, &[Role::Developer])
            .await
    }

    pub async fn call_builtin_with_roles(
        &self,
        tool_name: &str,
        args: Value,
        roles: &[Role],
    ) -> ArgusResult<ToolResult> {
        let permission = builtin_permission(tool_name);
        if let Some(permission) = permission {
            if let Err(err) = self.rbac.require_roles(roles, permission) {
                self.audit.append(
                    format!("roles={roles:?}"),
                    format!("builtin:{tool_name}"),
                    AuditOutcome::Blocked,
                    None::<String>,
                    Some(serde_json::json!({"reason": format!("{err}")})),
                )?;
                return Err(err);
            }
        }

        let result = self.call_builtin_inner(tool_name, args.clone()).await;
        self.audit_builtin(tool_name, &args, &result)?;
        result
    }

    async fn call_builtin_inner(&self, tool_name: &str, args: Value) -> ArgusResult<ToolResult> {
        match tool_name {
            "filesystem.read" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ArgusError::Mcp("missing path".to_string()))?;
                let content = self.filesystem.read_text(path).await?;
                Ok(ToolResult {
                    success: true,
                    content: Value::String(content),
                    error: None,
                })
            }
            "filesystem.write" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ArgusError::Mcp("missing path".to_string()))?;
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ArgusError::Mcp("missing content".to_string()))?;
                self.filesystem.write_text(path, content).await?;
                Ok(ToolResult {
                    success: true,
                    content: serde_json::json!({"written": true}),
                    error: None,
                })
            }
            "shell.run" => {
                let cmd = args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ArgusError::Mcp("missing command".to_string()))?;
                let output = self.shell.run(cmd).await?;
                Ok(ToolResult {
                    success: true,
                    content: Value::String(output),
                    error: None,
                })
            }
            "database.query" => {
                let sql = args
                    .get("sql")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ArgusError::Mcp("missing sql".to_string()))?;
                let output = self.database.query_readonly(sql).await?;
                Ok(ToolResult {
                    success: true,
                    content: Value::String(output),
                    error: None,
                })
            }
            _ => Err(ArgusError::Mcp(format!(
                "unknown builtin tool: {tool_name}"
            ))),
        }
    }

    pub async fn call_server(
        &self,
        server_id: &str,
        tool_name: &str,
        args: Value,
    ) -> ArgusResult<ToolResult> {
        if let Err(err) = self
            .rbac
            .require_roles(&[Role::Developer], Permission::ExecuteCommand)
        {
            self.audit.append(
                "role=developer",
                format!("server:{server_id}:{tool_name}"),
                AuditOutcome::Blocked,
                Some(server_id),
                Some(serde_json::json!({"reason": format!("{err}")})),
            )?;
            return Err(err);
        }

        let server = self
            .registry
            .get(server_id)?
            .ok_or_else(|| ArgusError::Mcp(format!("server not found: {server_id}")))?;

        if !server.enabled {
            return Err(ArgusError::Mcp("server is disabled".to_string()));
        }

        let client = McpClient::connect(server.config)?;
        let result = client.call_tool(tool_name, args.clone()).await;
        self.audit_server(server_id, tool_name, &args, &result)?;
        result
    }

    fn audit_builtin(
        &self,
        tool_name: &str,
        args: &Value,
        result: &ArgusResult<ToolResult>,
    ) -> ArgusResult<()> {
        let (outcome, error) = match result {
            Ok(_) => (AuditOutcome::Success, None::<String>),
            Err(e) => {
                let text = format!("{e}");
                let outcome = if text.contains("Permission denied") || text.contains("blocked") {
                    AuditOutcome::Blocked
                } else {
                    AuditOutcome::Failure
                };
                (outcome, Some(text))
            }
        };

        self.audit.append(
            "role=developer",
            format!("builtin:{tool_name}"),
            outcome,
            None::<String>,
            Some(serde_json::json!({"args": args, "error": error})),
        )?;
        Ok(())
    }

    fn audit_server(
        &self,
        server_id: &str,
        tool_name: &str,
        args: &Value,
        result: &ArgusResult<ToolResult>,
    ) -> ArgusResult<()> {
        let (outcome, error) = match result {
            Ok(_) => (AuditOutcome::Success, None::<String>),
            Err(e) => {
                let text = format!("{e}");
                let outcome = if text.contains("Permission denied") || text.contains("blocked") {
                    AuditOutcome::Blocked
                } else {
                    AuditOutcome::Failure
                };
                (outcome, Some(text))
            }
        };

        self.audit.append(
            "role=developer",
            format!("server:{server_id}:{tool_name}"),
            outcome,
            Some(server_id),
            Some(serde_json::json!({"args": args, "error": error})),
        )?;
        Ok(())
    }
}

fn builtin_permission(tool_name: &str) -> Option<Permission> {
    match tool_name {
        "filesystem.read" | "database.query" => Some(Permission::ReadProject),
        "filesystem.write" => Some(Permission::WriteCode),
        "shell.run" => Some(Permission::ExecuteCommand),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use argus_core::types::{AuditOutcome, Role};
    use uuid::Uuid;

    use super::*;
    use crate::registry::ServerRegistry;

    fn unique_temp_root() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("argus-mcp-router-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[tokio::test]
    async fn test_rbac_denies_shell_for_viewer() {
        let root = unique_temp_root();
        let router = ToolRouter::new(ServerRegistry::new(), root);

        let err = router
            .call_builtin_with_roles(
                "shell.run",
                serde_json::json!({"command": "echo hello"}),
                &[Role::Viewer],
            )
            .await
            .unwrap_err();

        assert!(format!("{err}").contains("Permission denied"));

        let events = router.audit.events().unwrap();
        assert!(!events.is_empty());
        assert!(matches!(events[0].outcome, AuditOutcome::Blocked));
        assert!(router.audit.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_guardrail_blocks_protected_write() {
        let root = unique_temp_root();
        let router = ToolRouter::new(ServerRegistry::new(), root);

        let err = router
            .call_builtin(
                "filesystem.write",
                serde_json::json!({
                    "path": "crates/argus-security/src/rbac.rs",
                    "content": "blocked"
                }),
            )
            .await
            .unwrap_err();

        assert!(format!("{err}").contains("guardrail violation"));
        assert!(router.audit.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_secret_scanner_blocks_write() {
        let root = unique_temp_root();
        let router = ToolRouter::new(ServerRegistry::new(), root);

        let err = router
            .call_builtin(
                "filesystem.write",
                serde_json::json!({
                    "path": "docs/notes.txt",
                    "content": "OPENAI_API_KEY=sk-1234567890abcdefghijklmnop"
                }),
            )
            .await
            .unwrap_err();

        assert!(format!("{err}").contains("secret scanner blocked write"));
        assert!(router.audit.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_audit_chain_for_success_and_failure() {
        let root = unique_temp_root();
        fs::write(root.join("ok.txt"), "hello").unwrap();
        let router = ToolRouter::new(ServerRegistry::new(), &root);

        let ok = router
            .call_builtin(
                "filesystem.read",
                serde_json::json!({
                    "path": "ok.txt"
                }),
            )
            .await
            .unwrap();
        assert!(ok.success);

        let err = router
            .call_builtin(
                "shell.run",
                serde_json::json!({
                    "command": "rm -rf /"
                }),
            )
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("blocked"));

        let events = router.audit.events().unwrap();
        assert!(events.len() >= 2);
        assert!(router.audit.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_with_security_config_applies_overrides() {
        let root = unique_temp_root();
        let security = SecurityConfig {
            blocked_commands: vec!["echo".to_string()],
            protected_paths: vec!["custom/locked.txt".to_string()],
            forbidden_content_markers: vec!["CUSTOM_MARKER".to_string()],
            secret_keywords: vec!["credential".to_string()],
            ..SecurityConfig::default()
        };

        let router = ToolRouter::with_security_config(ServerRegistry::new(), &root, &security);

        let blocked_shell = router
            .call_builtin("shell.run", serde_json::json!({"command": "echo hi"}))
            .await;
        assert!(blocked_shell.is_err());

        let blocked_path = router
            .call_builtin(
                "filesystem.write",
                serde_json::json!({"path": "custom/locked.txt", "content": "ok"}),
            )
            .await;
        assert!(blocked_path.is_err());

        let blocked_secret = router
            .call_builtin(
                "filesystem.write",
                serde_json::json!({"path": "notes.txt", "content": "CREDENTIAL=abcd1234abcd1234"}),
            )
            .await;
        assert!(blocked_secret.is_err());
    }
}
