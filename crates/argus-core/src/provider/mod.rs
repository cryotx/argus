//! V0.44 — Provider / Auth / Subscription / Quota.
//!
//! Maps V0.42 capability requirements onto concrete runtime providers, models,
//! auth profiles, and quota state:
//!
//! ```text
//! Agent Role -> Capability Requirement -> Provider Capability -> Auth Profile
//!   -> Quota / Availability -> Cost / Policy -> Concrete Execution Target
//! ```
//!
//! `Role != Model != Provider != Auth != Subscription != API billing`. No raw
//! secret is stored or returned — only a credential locator. Provider failover
//! is separate from model tier escalation.

pub mod adapter;
pub mod quota;
pub mod registry;
pub mod repository;
pub mod router;
pub mod service;
pub mod types;

pub use adapter::{CliProviderAdapter, FakeProviderAdapter, ProviderAdapter, discover_cli};
pub use quota::{QuotaProvider, StaticQuotaProvider, UnknownQuotaProvider};
pub use registry::{ProviderOverride, ProviderRegistry};
pub use repository::{PostgresProviderRepository, ProviderRepository};
pub use router::{ProviderRouter, ProviderRoutingInput, RouterAttempt, RouterOutput};
pub use service::{
    BudgetGate, BudgetGateDecision, FakeBudgetGate, ProviderService, RoutingOutcome,
};
pub use types::*;
