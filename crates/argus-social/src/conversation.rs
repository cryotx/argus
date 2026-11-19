use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use argus_core::types::{ArgusError, ArgusResult};

use crate::types::SocialMessage;

#[derive(Clone, Debug)]
pub struct ConversationSession {
    pub id: Uuid,
    pub channel_id: String,
    pub user_id: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<SocialMessage>,
}

impl ConversationSession {
    pub fn new(channel_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            channel_id: channel_id.into(),
            user_id: user_id.into(),
            started_at: now,
            updated_at: now,
            messages: Vec::new(),
        }
    }
}

#[derive(Clone, Default)]
pub struct ConversationManager {
    sessions: Arc<RwLock<HashMap<(String, String), ConversationSession>>>,
}

impl ConversationManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&self, msg: SocialMessage) -> ArgusResult<Uuid> {
        let mut map = self
            .sessions
            .write()
            .map_err(|e| ArgusError::Social(format!("conversation lock poisoned: {e}")))?;

        let key = (msg.channel_id.clone(), msg.user_id.clone());
        let session = map.entry(key).or_insert_with(|| {
            ConversationSession::new(msg.channel_id.clone(), msg.user_id.clone())
        });
        session.updated_at = Utc::now();
        session.messages.push(msg);
        Ok(session.id)
    }

    pub fn get(&self, channel_id: &str, user_id: &str) -> ArgusResult<Option<ConversationSession>> {
        let map = self
            .sessions
            .read()
            .map_err(|e| ArgusError::Social(format!("conversation lock poisoned: {e}")))?;
        Ok(map
            .get(&(channel_id.to_string(), user_id.to_string()))
            .cloned())
    }
}
