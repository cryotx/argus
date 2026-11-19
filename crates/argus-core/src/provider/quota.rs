//! Quota providers.
//!
//! Most subscription CLIs do not expose an exact machine-readable remaining
//! allowance. When they don't, ARGUS records `Unknown` — never zero, never
//! unlimited. Refresh is on demand with cache TTL; there is no background poll.

use async_trait::async_trait;
use uuid::Uuid;

use super::types::*;

#[async_trait]
pub trait QuotaProvider: Send + Sync {
    /// Best-effort quota observation for a provider/auth profile. Returns
    /// `None` when nothing can be safely determined (caller records Unknown).
    async fn refresh(
        &self,
        provider: RuntimeProvider,
        auth_profile_id: Option<Uuid>,
    ) -> Option<QuotaSnapshot>;
}

/// Default provider: no machine-readable quota. The router treats this as
/// `Unknown` and proceeds best-effort rather than assuming exhaustion.
pub struct UnknownQuotaProvider;

#[async_trait]
impl QuotaProvider for UnknownQuotaProvider {
    async fn refresh(
        &self,
        _provider: RuntimeProvider,
        _auth_profile_id: Option<Uuid>,
    ) -> Option<QuotaSnapshot> {
        None
    }
}

/// Deterministic provider for tests.
pub struct StaticQuotaProvider {
    snapshot: Option<QuotaSnapshot>,
}

impl StaticQuotaProvider {
    pub fn with_snapshot(snapshot: QuotaSnapshot) -> Self {
        Self {
            snapshot: Some(snapshot),
        }
    }

    pub fn unknown() -> Self {
        Self { snapshot: None }
    }
}

#[async_trait]
impl QuotaProvider for StaticQuotaProvider {
    async fn refresh(
        &self,
        _provider: RuntimeProvider,
        _auth_profile_id: Option<Uuid>,
    ) -> Option<QuotaSnapshot> {
        self.snapshot.clone()
    }
}

/// Build a quota snapshot representing an unknown subscription allowance.
pub fn unknown_snapshot(provider: RuntimeProvider, auth_profile_id: Option<Uuid>) -> QuotaSnapshot {
    QuotaSnapshot {
        quota_snapshot_id: Uuid::new_v4(),
        provider_id: provider,
        auth_profile_id,
        quota_kind: QuotaKind::Unknown,
        availability: QuotaAvailability::Unknown,
        remaining: None,
        quota_limit: None,
        unit: "unknown".to_string(),
        reset_at: None,
        observed_at: chrono::Utc::now(),
        source: QuotaSource::Unknown,
        confidence: QuotaConfidence::Unknown,
        raw_metadata: serde_json::json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_provider_yields_no_snapshot() {
        let q = UnknownQuotaProvider;
        assert!(
            q.refresh(RuntimeProvider::ClaudeCodeCli, None)
                .await
                .is_none()
        );
    }

    #[test]
    fn unknown_snapshot_is_neither_zero_nor_unlimited() {
        let s = unknown_snapshot(RuntimeProvider::CodexCli, None);
        assert_eq!(s.availability, QuotaAvailability::Unknown);
        assert!(s.remaining.is_none());
        assert!(!s.is_known_exhausted());
    }
}
