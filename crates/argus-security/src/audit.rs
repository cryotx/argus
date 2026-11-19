use std::sync::RwLock;

use argus_core::types::{ArgusError, ArgusResult, AuditOutcome};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use tokio::sync::RwLock as AsyncRwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub outcome: AuditOutcome,
    pub target: Option<String>,
    pub metadata: Value,
    pub previous_hash: Option<String>,
    pub hash: String,
}

impl AuditEvent {
    /// Build a hash-linked event without advancing a logger. Async callers
    /// can serialize persistence and advance their head only after a write succeeds.
    pub fn new(
        previous: Option<&Self>,
        actor: impl Into<String>,
        action: impl Into<String>,
        outcome: AuditOutcome,
        target: Option<String>,
        metadata: Value,
    ) -> ArgusResult<Self> {
        build_event(
            previous,
            actor.into(),
            action.into(),
            outcome,
            target,
            metadata,
        )
    }
}

#[derive(Debug, Clone)]
pub struct AuditAppendOutcome {
    pub event: AuditEvent,
    pub persisted: bool,
    pub persistence_error: Option<String>,
}

#[derive(Default)]
pub struct AuditLogger {
    chain: RwLock<Vec<AuditEvent>>,
}

#[async_trait]
pub trait AuditStore: Send + Sync {
    async fn append_event(&self, event: &AuditEvent) -> ArgusResult<()>;
    async fn load_events(&self, limit: usize) -> ArgusResult<Vec<AuditEvent>>;
}

#[derive(Clone)]
pub struct PostgresAuditStore {
    pool: PgPool,
}

impl PostgresAuditStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuditStore for PostgresAuditStore {
    async fn append_event(&self, event: &AuditEvent) -> ArgusResult<()> {
        let actor_json = serde_json::json!({"id": event.actor});
        let action_json = serde_json::json!({"name": event.action});
        let resource_json = serde_json::json!({"target": event.target});
        let details_json = details_payload(event);

        sqlx::query(
            "INSERT INTO audit_log (id, timestamp, actor, action, resource, outcome, details, ip_address, session_id) VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, NULL)",
        )
        .bind(event.id)
        .bind(event.timestamp)
        .bind(actor_json)
        .bind(action_json)
        .bind(resource_json)
        .bind(event.outcome.clone())
        .bind(details_json)
        .execute(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        Ok(())
    }

    async fn load_events(&self, limit: usize) -> ArgusResult<Vec<AuditEvent>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut rows: Vec<AuditLogRow> = sqlx::query_as(
            "SELECT id, timestamp, actor, action, resource, outcome, details FROM audit_log ORDER BY timestamp DESC LIMIT $1",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(ArgusError::Database)?;

        rows.reverse();

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            events.push(row.into_event()?);
        }
        Ok(events)
    }
}

#[derive(Default)]
pub struct MemoryAuditStore {
    events: AsyncRwLock<Vec<AuditEvent>>,
}

impl MemoryAuditStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AuditStore for MemoryAuditStore {
    async fn append_event(&self, event: &AuditEvent) -> ArgusResult<()> {
        self.events.write().await.push(event.clone());
        Ok(())
    }

    async fn load_events(&self, limit: usize) -> ArgusResult<Vec<AuditEvent>> {
        let guard = self.events.read().await;
        if limit >= guard.len() {
            return Ok(guard.clone());
        }
        Ok(guard[guard.len() - limit..].to_vec())
    }
}

impl AuditLogger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append<T>(
        &self,
        actor: impl Into<String>,
        action: impl Into<String>,
        outcome: AuditOutcome,
        target: Option<T>,
        metadata: Option<Value>,
    ) -> ArgusResult<AuditEvent>
    where
        T: Into<String>,
    {
        let mut guard = self
            .chain
            .write()
            .map_err(|e| ArgusError::Security(format!("audit chain lock poisoned: {e}")))?;

        let event = build_event(
            guard.last(),
            actor.into(),
            action.into(),
            outcome,
            target.map(Into::into),
            metadata.unwrap_or(Value::Null),
        )?;

        guard.push(event.clone());
        Ok(event)
    }

    #[allow(clippy::await_holding_lock)]
    pub async fn append_persisted<T>(
        &self,
        actor: impl Into<String>,
        action: impl Into<String>,
        outcome: AuditOutcome,
        target: Option<T>,
        metadata: Option<Value>,
        store: &impl AuditStore,
    ) -> ArgusResult<AuditEvent>
    where
        T: Into<String>,
    {
        let mut guard = self
            .chain
            .write()
            .map_err(|e| ArgusError::Security(format!("audit chain lock poisoned: {e}")))?;

        let event = build_event(
            guard.last(),
            actor.into(),
            action.into(),
            outcome,
            target.map(Into::into),
            metadata.unwrap_or(Value::Null),
        )?;

        store.append_event(&event).await?;
        guard.push(event.clone());
        Ok(event)
    }

    #[allow(clippy::await_holding_lock)]
    pub async fn append_persisted_with_degradation<T>(
        &self,
        actor: impl Into<String>,
        action: impl Into<String>,
        outcome: AuditOutcome,
        target: Option<T>,
        metadata: Option<Value>,
        store: &impl AuditStore,
    ) -> ArgusResult<AuditAppendOutcome>
    where
        T: Into<String>,
    {
        let mut guard = self
            .chain
            .write()
            .map_err(|e| ArgusError::Security(format!("audit chain lock poisoned: {e}")))?;

        let event = build_event(
            guard.last(),
            actor.into(),
            action.into(),
            outcome,
            target.map(Into::into),
            metadata.unwrap_or(Value::Null),
        )?;

        let persisted = match store.append_event(&event).await {
            Ok(()) => true,
            Err(err) => {
                guard.push(event.clone());
                return Ok(AuditAppendOutcome {
                    event,
                    persisted: false,
                    persistence_error: Some(format!("{err}")),
                });
            }
        };

        guard.push(event.clone());
        Ok(AuditAppendOutcome {
            event,
            persisted,
            persistence_error: None,
        })
    }

    pub async fn hydrate_from_store(
        &self,
        store: &impl AuditStore,
        limit: usize,
    ) -> ArgusResult<usize> {
        let events = store.load_events(limit).await?;
        if !verify_events_chain(&events)? {
            return Err(ArgusError::Security(
                "cannot hydrate audit chain: persisted chain verification failed".to_string(),
            ));
        }

        let mut guard = self
            .chain
            .write()
            .map_err(|e| ArgusError::Security(format!("audit chain lock poisoned: {e}")))?;
        let count = events.len();
        *guard = events;
        Ok(count)
    }

    pub fn events(&self) -> ArgusResult<Vec<AuditEvent>> {
        let guard = self
            .chain
            .read()
            .map_err(|e| ArgusError::Security(format!("audit chain lock poisoned: {e}")))?;
        Ok(guard.clone())
    }

    pub fn verify_chain(&self) -> ArgusResult<bool> {
        let guard = self
            .chain
            .read()
            .map_err(|e| ArgusError::Security(format!("audit chain lock poisoned: {e}")))?;
        verify_events_chain(&guard)
    }
}

fn build_event(
    previous: Option<&AuditEvent>,
    actor: String,
    action: String,
    outcome: AuditOutcome,
    target: Option<String>,
    metadata: Value,
) -> ArgusResult<AuditEvent> {
    let timestamp = Utc::now();
    let previous_hash = previous.map(|e| e.hash.clone());

    let hash = compute_hash(
        &timestamp,
        &actor,
        &action,
        &outcome,
        target.as_deref(),
        &metadata,
        previous_hash.as_deref(),
    )?;

    Ok(AuditEvent {
        id: Uuid::new_v4(),
        timestamp,
        actor,
        action,
        outcome,
        target,
        metadata,
        previous_hash,
        hash,
    })
}

fn verify_events_chain(events: &[AuditEvent]) -> ArgusResult<bool> {
    let mut previous_hash: Option<String> = None;
    for event in events {
        if event.previous_hash != previous_hash {
            return Ok(false);
        }

        let expected = compute_hash(
            &event.timestamp,
            &event.actor,
            &event.action,
            &event.outcome,
            event.target.as_deref(),
            &event.metadata,
            event.previous_hash.as_deref(),
        )?;

        if expected != event.hash {
            return Ok(false);
        }
        previous_hash = Some(event.hash.clone());
    }
    Ok(true)
}

#[derive(Debug, Clone, FromRow)]
struct AuditLogRow {
    id: Uuid,
    timestamp: DateTime<Utc>,
    actor: Value,
    action: Value,
    resource: Value,
    outcome: AuditOutcome,
    details: Value,
}

impl AuditLogRow {
    fn into_event(self) -> ArgusResult<AuditEvent> {
        let actor = self
            .actor
            .get("id")
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.actor.to_string());

        let action = self
            .action
            .get("name")
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.action.to_string());

        let target = self
            .resource
            .get("target")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);

        let metadata = self.details.get("metadata").cloned().unwrap_or(Value::Null);
        let previous_hash = self
            .details
            .get("previous_hash")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);

        let hash = if let Some(hash) = self
            .details
            .get("hash")
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
        {
            hash
        } else {
            compute_hash(
                &self.timestamp,
                &actor,
                &action,
                &self.outcome,
                target.as_deref(),
                &metadata,
                previous_hash.as_deref(),
            )?
        };

        Ok(AuditEvent {
            id: self.id,
            timestamp: self.timestamp,
            actor,
            action,
            outcome: self.outcome,
            target,
            metadata,
            previous_hash,
            hash,
        })
    }
}

fn details_payload(event: &AuditEvent) -> Value {
    serde_json::json!({
        "metadata": &event.metadata,
        "previous_hash": &event.previous_hash,
        "hash": &event.hash,
    })
}

fn compute_hash(
    timestamp: &DateTime<Utc>,
    actor: &str,
    action: &str,
    outcome: &AuditOutcome,
    target: Option<&str>,
    metadata: &Value,
    previous_hash: Option<&str>,
) -> ArgusResult<String> {
    let payload = serde_json::json!({
        "timestamp": timestamp.to_rfc3339(),
        "actor": actor,
        "action": action,
        "outcome": outcome,
        "target": target,
        "metadata": metadata,
        "previous_hash": previous_hash,
    });
    let bytes = serde_json::to_vec(&payload)?;

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();

    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AlwaysFailStore;

    #[async_trait]
    impl AuditStore for AlwaysFailStore {
        async fn append_event(&self, _event: &AuditEvent) -> ArgusResult<()> {
            Err(ArgusError::Database(sqlx::Error::Protocol(
                "simulated write failure".to_string(),
            )))
        }

        async fn load_events(&self, _limit: usize) -> ArgusResult<Vec<AuditEvent>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn test_chain_verify_success() {
        let logger = AuditLogger::new();
        logger
            .append(
                "owner",
                "create_task",
                AuditOutcome::Success,
                Some("t1"),
                None,
            )
            .unwrap();
        logger
            .append("owner", "run_task", AuditOutcome::Warned, Some("t1"), None)
            .unwrap();

        assert!(logger.verify_chain().unwrap());
    }

    #[test]
    fn test_chain_linked_hashes() {
        let logger = AuditLogger::new();
        let first = logger
            .append("owner", "one", AuditOutcome::Success, None::<String>, None)
            .unwrap();
        let second = logger
            .append("owner", "two", AuditOutcome::Success, None::<String>, None)
            .unwrap();
        assert_eq!(second.previous_hash, Some(first.hash));
    }

    #[test]
    fn test_chain_detects_tamper() {
        let logger = AuditLogger::new();
        logger
            .append("owner", "one", AuditOutcome::Success, None::<String>, None)
            .unwrap();

        {
            let mut guard = logger.chain.write().unwrap();
            guard[0].action = "tampered".to_string();
        }

        assert!(!logger.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_append_persisted_with_memory_store() {
        let logger = AuditLogger::new();
        let store = MemoryAuditStore::new();

        logger
            .append_persisted(
                "owner",
                "one",
                AuditOutcome::Success,
                Some("t1"),
                Some(serde_json::json!({"k": "v"})),
                &store,
            )
            .await
            .unwrap();

        logger
            .append_persisted(
                "owner",
                "two",
                AuditOutcome::Warned,
                None::<String>,
                None,
                &store,
            )
            .await
            .unwrap();

        let persisted = store.load_events(10).await.unwrap();
        assert_eq!(persisted.len(), 2);
        assert!(logger.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_hydrate_from_store() {
        let logger = AuditLogger::new();
        let source = AuditLogger::new();
        let store = MemoryAuditStore::new();

        source
            .append_persisted(
                "owner",
                "a",
                AuditOutcome::Success,
                None::<String>,
                None,
                &store,
            )
            .await
            .unwrap();
        source
            .append_persisted(
                "owner",
                "b",
                AuditOutcome::Failure,
                None::<String>,
                None,
                &store,
            )
            .await
            .unwrap();

        let hydrated = logger.hydrate_from_store(&store, 10).await.unwrap();
        assert_eq!(hydrated, 2);
        assert!(logger.verify_chain().unwrap());
    }

    #[tokio::test]
    async fn test_append_persisted_degrades_on_store_failure() {
        let logger = AuditLogger::new();
        let outcome = logger
            .append_persisted_with_degradation(
                "owner",
                "degraded-event",
                AuditOutcome::Warned,
                None::<String>,
                None,
                &AlwaysFailStore,
            )
            .await
            .unwrap();

        assert!(!outcome.persisted);
        assert!(outcome.persistence_error.is_some());
        assert!(logger.verify_chain().unwrap());
        assert_eq!(logger.events().unwrap().len(), 1);
    }
}
