use argus_core::types::TrustLevel;

use crate::manifest::{SkillManifest, SkillRuntime};

pub struct SkillGenerator;

impl SkillGenerator {
    pub fn new() -> Self {
        Self
    }

    /// P8 placeholder: generate a shell skill from NL description.
    pub fn generate_from_description(
        &self,
        id: &str,
        description: &str,
        command: &str,
    ) -> SkillManifest {
        SkillManifest {
            id: id.to_string(),
            name: id.to_string(),
            description: description.to_string(),
            runtime: SkillRuntime::Shell,
            command: command.to_string(),
            permissions: vec!["workspace.exec".to_string()],
            trust_level: TrustLevel::ThirdParty,
        }
    }
}

impl Default for SkillGenerator {
    fn default() -> Self {
        Self::new()
    }
}
