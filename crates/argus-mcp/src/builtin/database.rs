use argus_core::types::{ArgusError, ArgusResult};

#[derive(Clone, Debug, Default)]
pub struct DatabaseServer;

impl DatabaseServer {
    pub fn new() -> Self {
        Self
    }

    pub async fn query_readonly(&self, sql: &str) -> ArgusResult<String> {
        let normalized = sql.trim().to_lowercase();
        if !normalized.starts_with("select") {
            return Err(ArgusError::Security("only SELECT is allowed".to_string()));
        }

        // Placeholder in P8; real DB execution in later phase.
        Ok("readonly query accepted".to_string())
    }
}
