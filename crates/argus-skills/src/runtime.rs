use tokio::process::Command;

use argus_core::types::{ArgusError, ArgusResult, AuditOutcome, TrustLevel};
use argus_security::audit::AuditLogger;
use argus_security::command_guard::CommandSafetyGuard;

use crate::manifest::{SkillManifest, SkillRuntime};

#[derive(Clone, Debug)]
pub struct SkillOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub struct SkillRuntimeEngine {
    command_guard: CommandSafetyGuard,
    audit: AuditLogger,
}

impl SkillRuntimeEngine {
    pub fn new() -> Self {
        Self {
            command_guard: CommandSafetyGuard::new(),
            audit: AuditLogger::new(),
        }
    }

    pub async fn execute(
        &self,
        skill: &SkillManifest,
        args: &[String],
    ) -> ArgusResult<SkillOutput> {
        if skill.trust_level == TrustLevel::Untrusted
            && matches!(skill.runtime, SkillRuntime::Shell | SkillRuntime::Native)
        {
            self.audit.append(
                "skill-runtime",
                format!("execute:{}", skill.id),
                AuditOutcome::Blocked,
                Some(skill.id.clone()),
                Some(serde_json::json!({"reason": "untrusted shell/native skill blocked"})),
            )?;
            return Err(ArgusError::Security(
                "untrusted shell/native skill is blocked".to_string(),
            ));
        }

        let mut cmd = match skill.runtime {
            SkillRuntime::Shell | SkillRuntime::Native => {
                let mut c = Command::new("bash");
                let full = if args.is_empty() {
                    skill.command.clone()
                } else {
                    format!("{} {}", skill.command, args.join(" "))
                };
                self.command_guard.enforce(&full).map_err(|e| {
                    ArgusError::Security(format!("skill command blocked by command guard: {e}"))
                })?;
                c.arg("-c").arg(full);
                c
            }
            SkillRuntime::Python => {
                let mut c = Command::new("python3");
                c.arg("-c").arg(&skill.command);
                c.args(args);
                c
            }
        };

        let out = cmd
            .output()
            .await
            .map_err(|e| ArgusError::Task(format!("skill exec failed: {e}")))?;
        let output = SkillOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        };

        self.audit.append(
            "skill-runtime",
            format!("execute:{}", skill.id),
            if output.success {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failure
            },
            Some(skill.id.clone()),
            Some(serde_json::json!({
                "runtime": format!("{:?}", skill.runtime),
                "args_count": args.len(),
                "stderr": output.stderr.clone(),
            })),
        )?;

        Ok(output)
    }

    pub fn audit_chain_ok(&self) -> ArgusResult<bool> {
        self.audit.verify_chain()
    }
}

impl Default for SkillRuntimeEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::SkillManifest;

    fn mk_skill(
        id: &str,
        runtime: SkillRuntime,
        command: &str,
        trust_level: TrustLevel,
    ) -> SkillManifest {
        SkillManifest {
            id: id.to_string(),
            name: id.to_string(),
            description: "test skill".to_string(),
            runtime,
            command: command.to_string(),
            permissions: vec![],
            trust_level,
        }
    }

    #[tokio::test]
    async fn test_block_untrusted_shell_skill() {
        let engine = SkillRuntimeEngine::new();
        let skill = mk_skill("s1", SkillRuntime::Shell, "echo hi", TrustLevel::Untrusted);
        let err = engine.execute(&skill, &[]).await.unwrap_err();
        assert!(format!("{err}").contains("blocked"));
    }

    #[tokio::test]
    async fn test_block_dangerous_shell_command() {
        let engine = SkillRuntimeEngine::new();
        let skill = mk_skill("s2", SkillRuntime::Shell, "rm -rf /", TrustLevel::Verified);
        let err = engine.execute(&skill, &[]).await.unwrap_err();
        assert!(format!("{err}").contains("command guard"));
    }

    #[tokio::test]
    async fn test_run_safe_shell_command() {
        let engine = SkillRuntimeEngine::new();
        let skill = mk_skill("s3", SkillRuntime::Shell, "echo hi", TrustLevel::Verified);
        let out = engine.execute(&skill, &[]).await.unwrap();
        assert!(out.success);
        assert!(out.stdout.contains("hi"));
        assert!(engine.audit_chain_ok().unwrap());
    }
}
