use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use argus_core::types::{ArgusError, ArgusResult, TrustLevel};

use crate::client::McpServerConfig;

#[derive(Clone, Debug)]
pub struct RegisteredServer {
    pub config: McpServerConfig,
    pub trust_level: TrustLevel,
    pub enabled: bool,
}

#[derive(Clone, Default)]
pub struct ServerRegistry {
    inner: Arc<RwLock<HashMap<String, RegisteredServer>>>,
}

impl ServerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, server: RegisteredServer) -> ArgusResult<()> {
        let id = server.config.id.clone();
        let mut map = self
            .inner
            .write()
            .map_err(|e| ArgusError::Mcp(format!("registry lock poisoned: {e}")))?;
        map.insert(id, server);
        Ok(())
    }

    pub fn remove(&self, server_id: &str) -> ArgusResult<bool> {
        let mut map = self
            .inner
            .write()
            .map_err(|e| ArgusError::Mcp(format!("registry lock poisoned: {e}")))?;
        Ok(map.remove(server_id).is_some())
    }

    pub fn get(&self, server_id: &str) -> ArgusResult<Option<RegisteredServer>> {
        let map = self
            .inner
            .read()
            .map_err(|e| ArgusError::Mcp(format!("registry lock poisoned: {e}")))?;
        Ok(map.get(server_id).cloned())
    }

    pub fn list(&self) -> ArgusResult<Vec<RegisteredServer>> {
        let map = self
            .inner
            .read()
            .map_err(|e| ArgusError::Mcp(format!("registry lock poisoned: {e}")))?;
        Ok(map.values().cloned().collect())
    }
}
