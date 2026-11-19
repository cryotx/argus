use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use argus_core::types::{ArgusError, ArgusResult};

use crate::manifest::SkillManifest;

#[derive(Clone, Default)]
pub struct SkillRegistry {
    inner: Arc<RwLock<HashMap<String, SkillManifest>>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, skill: SkillManifest) -> ArgusResult<()> {
        let mut map = self
            .inner
            .write()
            .map_err(|e| ArgusError::Task(format!("skill registry poisoned: {e}")))?;
        map.insert(skill.id.clone(), skill);
        Ok(())
    }

    pub fn get(&self, id: &str) -> ArgusResult<Option<SkillManifest>> {
        let map = self
            .inner
            .read()
            .map_err(|e| ArgusError::Task(format!("skill registry poisoned: {e}")))?;
        Ok(map.get(id).cloned())
    }

    pub fn list(&self) -> ArgusResult<Vec<SkillManifest>> {
        let map = self
            .inner
            .read()
            .map_err(|e| ArgusError::Task(format!("skill registry poisoned: {e}")))?;
        Ok(map.values().cloned().collect())
    }
}
