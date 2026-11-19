use std::collections::{HashMap, HashSet};

use argus_core::types::{ArgusError, ArgusResult, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ReadProject,
    WriteCode,
    ExecuteCommand,
    ManageUsers,
    ManageSecrets,
    ConfigureSystem,
    ReadAudit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub role: Role,
    pub permission: Permission,
    pub effect: PolicyEffect,
}

#[derive(Debug, Clone)]
pub struct AccessDecision {
    pub allowed: bool,
    pub reason: String,
}

pub struct RbacPolicyEngine {
    rules: Vec<PolicyRule>,
    index: HashMap<(Role, Permission), PolicyEffect>,
}

impl RbacPolicyEngine {
    pub fn new() -> Self {
        let mut engine = Self {
            rules: Vec::new(),
            index: HashMap::new(),
        };

        engine.load_default_rules();
        engine
    }

    pub fn with_rules(rules: Vec<PolicyRule>) -> Self {
        let mut index = HashMap::new();
        for rule in &rules {
            index.insert((rule.role.clone(), rule.permission), rule.effect);
        }
        Self { rules, index }
    }

    pub fn rules(&self) -> &[PolicyRule] {
        &self.rules
    }

    pub fn check_roles(&self, roles: &[Role], permission: Permission) -> AccessDecision {
        let role_set: HashSet<Role> = roles.iter().cloned().collect();

        if role_set.contains(&Role::Owner) {
            return AccessDecision {
                allowed: true,
                reason: "owner override".to_string(),
            };
        }

        for role in &role_set {
            if self
                .index
                .get(&(role.clone(), permission))
                .is_some_and(|effect| *effect == PolicyEffect::Deny)
            {
                return AccessDecision {
                    allowed: false,
                    reason: format!(
                        "explicit deny for role {role:?} and permission {permission:?}"
                    ),
                };
            }
        }

        for role in &role_set {
            if self
                .index
                .get(&(role.clone(), permission))
                .is_some_and(|effect| *effect == PolicyEffect::Allow)
            {
                return AccessDecision {
                    allowed: true,
                    reason: format!("allowed by role {role:?}"),
                };
            }
        }

        AccessDecision {
            allowed: false,
            reason: "default deny".to_string(),
        }
    }

    pub fn require_roles(&self, roles: &[Role], permission: Permission) -> ArgusResult<()> {
        let decision = self.check_roles(roles, permission);
        if decision.allowed {
            Ok(())
        } else {
            Err(ArgusError::PermissionDenied(decision.reason))
        }
    }

    fn load_default_rules(&mut self) {
        let mut rules = Vec::new();

        rules.extend(Self::allow_all_for(Role::Admin));

        rules.extend([
            PolicyRule {
                role: Role::Developer,
                permission: Permission::ReadProject,
                effect: PolicyEffect::Allow,
            },
            PolicyRule {
                role: Role::Developer,
                permission: Permission::WriteCode,
                effect: PolicyEffect::Allow,
            },
            PolicyRule {
                role: Role::Developer,
                permission: Permission::ExecuteCommand,
                effect: PolicyEffect::Allow,
            },
            PolicyRule {
                role: Role::Developer,
                permission: Permission::ReadAudit,
                effect: PolicyEffect::Allow,
            },
            PolicyRule {
                role: Role::Developer,
                permission: Permission::ManageUsers,
                effect: PolicyEffect::Deny,
            },
            PolicyRule {
                role: Role::Developer,
                permission: Permission::ManageSecrets,
                effect: PolicyEffect::Deny,
            },
            PolicyRule {
                role: Role::Viewer,
                permission: Permission::ReadProject,
                effect: PolicyEffect::Allow,
            },
            PolicyRule {
                role: Role::Viewer,
                permission: Permission::ReadAudit,
                effect: PolicyEffect::Allow,
            },
            PolicyRule {
                role: Role::Viewer,
                permission: Permission::WriteCode,
                effect: PolicyEffect::Deny,
            },
            PolicyRule {
                role: Role::Viewer,
                permission: Permission::ExecuteCommand,
                effect: PolicyEffect::Deny,
            },
            PolicyRule {
                role: Role::Viewer,
                permission: Permission::ManageUsers,
                effect: PolicyEffect::Deny,
            },
            PolicyRule {
                role: Role::Viewer,
                permission: Permission::ManageSecrets,
                effect: PolicyEffect::Deny,
            },
            PolicyRule {
                role: Role::Viewer,
                permission: Permission::ConfigureSystem,
                effect: PolicyEffect::Deny,
            },
        ]);

        self.index.clear();
        for rule in &rules {
            self.index
                .insert((rule.role.clone(), rule.permission), rule.effect);
        }
        self.rules = rules;
    }

    fn allow_all_for(role: Role) -> Vec<PolicyRule> {
        [
            Permission::ReadProject,
            Permission::WriteCode,
            Permission::ExecuteCommand,
            Permission::ManageUsers,
            Permission::ManageSecrets,
            Permission::ConfigureSystem,
            Permission::ReadAudit,
        ]
        .into_iter()
        .map(|permission| PolicyRule {
            role: role.clone(),
            permission,
            effect: PolicyEffect::Allow,
        })
        .collect()
    }
}

impl Default for RbacPolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_owner_override() {
        let engine = RbacPolicyEngine::new();
        let decision = engine.check_roles(&[Role::Owner], Permission::ManageSecrets);
        assert!(decision.allowed);
    }

    #[test]
    fn test_developer_allow_and_deny() {
        let engine = RbacPolicyEngine::new();
        assert!(
            engine
                .check_roles(&[Role::Developer], Permission::WriteCode)
                .allowed
        );
        assert!(
            !engine
                .check_roles(&[Role::Developer], Permission::ManageUsers)
                .allowed
        );
    }

    #[test]
    fn test_default_deny_for_unknown_combo() {
        let engine = RbacPolicyEngine::with_rules(vec![]);
        let decision = engine.check_roles(&[Role::Admin], Permission::ReadProject);
        assert!(!decision.allowed);
    }

    #[test]
    fn test_require_roles_permission_denied() {
        let engine = RbacPolicyEngine::new();
        let err = engine
            .require_roles(&[Role::Viewer], Permission::ExecuteCommand)
            .unwrap_err();
        assert!(format!("{err}").contains("Permission denied"));
    }
}
