use argus_core::types::TrustLevel;

use crate::manifest::{SkillManifest, SkillRuntime};

pub fn builtin_skills() -> Vec<SkillManifest> {
    vec![
        SkillManifest {
            id: "lint".to_string(),
            name: "Lint".to_string(),
            description: "Run lint checks".to_string(),
            runtime: SkillRuntime::Shell,
            command: "cargo clippy -q".to_string(),
            permissions: vec!["workspace.read".to_string(), "workspace.exec".to_string()],
            trust_level: TrustLevel::Builtin,
        },
        SkillManifest {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Run tests".to_string(),
            runtime: SkillRuntime::Shell,
            command: "cargo test -q".to_string(),
            permissions: vec!["workspace.read".to_string(), "workspace.exec".to_string()],
            trust_level: TrustLevel::Builtin,
        },
    ]
}
