use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use argus_core::types::{ArgusError, ArgusResult};

use crate::adapter::PlatformAdapter;
use crate::types::{Notification, Platform, SocialMessage};

#[derive(Clone, Default)]
pub struct SocialGateway {
    adapters: Arc<RwLock<HashMap<Platform, Arc<dyn PlatformAdapter>>>>,
}

impl SocialGateway {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_adapter(&self, adapter: Arc<dyn PlatformAdapter>) -> ArgusResult<()> {
        let platform = adapter.platform();
        let mut map = self
            .adapters
            .write()
            .map_err(|e| ArgusError::Social(format!("gateway lock poisoned: {e}")))?;
        map.insert(platform, adapter);
        Ok(())
    }

    pub async fn send(&self, platform: Platform, channel_id: &str, text: &str) -> ArgusResult<()> {
        let adapter = {
            let map = self
                .adapters
                .read()
                .map_err(|e| ArgusError::Social(format!("gateway lock poisoned: {e}")))?;
            map.get(&platform).cloned()
        }
        .ok_or_else(|| ArgusError::Social(format!("adapter not found: {platform:?}")))?;

        adapter.send_message(channel_id, text).await
    }

    pub async fn broadcast_notification(
        &self,
        n: &Notification,
        default_channel: &str,
    ) -> ArgusResult<()> {
        let text = format!("[{:?}] {}\n{}", n.level, n.title, n.body);
        for platform in &n.platforms {
            self.send(*platform, default_channel, &text).await?;
        }
        Ok(())
    }

    pub async fn normalize(
        &self,
        platform: Platform,
        payload: serde_json::Value,
    ) -> ArgusResult<SocialMessage> {
        let adapter = {
            let map = self
                .adapters
                .read()
                .map_err(|e| ArgusError::Social(format!("gateway lock poisoned: {e}")))?;
            map.get(&platform).cloned()
        }
        .ok_or_else(|| ArgusError::Social(format!("adapter not found: {platform:?}")))?;

        adapter.normalize_incoming(payload).await
    }
}
