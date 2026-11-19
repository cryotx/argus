use serde::{Deserialize, Serialize};

use argus_core::types::TrustLevel;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillRuntime {
    Native,
    Shell,
    Python,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub runtime: SkillRuntime,
    pub command: String,
    pub permissions: Vec<String>,
    pub trust_level: TrustLevel,
}
