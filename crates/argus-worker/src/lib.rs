//! argus-worker: Worker pool, subprocess drivers, sandboxing, resource monitoring
//!
//! Provides independent worker process management.

pub mod agent;
pub mod agent_executor;
pub mod drivers;
pub mod pool;
pub mod sandbox;
pub mod types;

// Re-export primary types explicitly to avoid ambiguous re-exports
pub use agent::{AgentRun, AgentTask};
pub use agent_executor::{AgentTaskExecutor, DEFAULT_AGENT_TIMEOUT, agent_executor_for};
pub use drivers::{AiderDriver, ClaudeCodeDriver, GenericCliDriver, OpenCodeDriver};
pub use pool::{Worker, WorkerPool};
pub use sandbox::SandboxHandle;
pub use types::{WorkerConfig, WorkerType};
