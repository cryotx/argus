//! Accounting state tracking for spend ledger health.
//!
//! When SpendRepository operations fail, the system enters DEGRADED state
//! and blocks all subsequent paid Agent executions until explicitly recovered.

use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Accounting system state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountingStatus {
    /// All spend operations are healthy.
    Healthy,
    /// Spend recording has failed; paid agents are blocked.
    Degraded,
}

impl std::fmt::Display for AccountingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountingStatus::Healthy => write!(f, "healthy"),
            AccountingStatus::Degraded => write!(f, "degraded"),
        }
    }
}

/// Accounting state with timestamp and reason.
#[derive(Clone, Debug)]
pub struct AccountingState {
    pub status: AccountingStatus,
    pub reason: Option<String>,
    pub last_updated: DateTime<Utc>,
}

impl AccountingState {
    pub fn new() -> Self {
        Self {
            status: AccountingStatus::Healthy,
            reason: None,
            last_updated: Utc::now(),
        }
    }

    pub fn mark_degraded(&mut self, reason: impl Into<String>) {
        self.status = AccountingStatus::Degraded;
        self.reason = Some(reason.into());
        self.last_updated = Utc::now();
    }

    pub fn mark_healthy(&mut self) {
        self.status = AccountingStatus::Healthy;
        self.reason = None;
        self.last_updated = Utc::now();
    }

    pub fn is_degraded(&self) -> bool {
        self.status == AccountingStatus::Degraded
    }
}

impl Default for AccountingState {
    fn default() -> Self {
        Self::new()
    }
}

/// Runtime accounting state manager.
///
/// Shared across the application to track spend ledger health.
pub struct RuntimeAccountingState {
    state: Arc<RwLock<AccountingState>>,
}

impl RuntimeAccountingState {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(AccountingState::new())),
        }
    }

    pub async fn status(&self) -> AccountingStatus {
        self.state.read().await.status
    }

    pub async fn mark_degraded(&self, reason: impl Into<String>) {
        self.state.write().await.mark_degraded(reason);
    }

    pub async fn mark_healthy(&self) {
        self.state.write().await.mark_healthy();
    }

    pub async fn get_state(&self) -> AccountingState {
        self.state.read().await.clone()
    }

    pub async fn is_degraded(&self) -> bool {
        self.state.read().await.is_degraded()
    }
}

impl Clone for RuntimeAccountingState {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl Default for RuntimeAccountingState {
    fn default() -> Self {
        Self::new()
    }
}
