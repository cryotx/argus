use async_trait::async_trait;

use argus_core::types::ArgusResult;

use crate::types::{Platform, SocialMessage};

#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    fn platform(&self) -> Platform;

    async fn send_message(&self, channel_id: &str, text: &str) -> ArgusResult<()>;

    async fn health_check(&self) -> ArgusResult<bool>;

    async fn normalize_incoming(&self, payload: serde_json::Value) -> ArgusResult<SocialMessage>;
}
