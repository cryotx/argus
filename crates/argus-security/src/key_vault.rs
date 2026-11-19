use std::collections::HashMap;
use std::sync::RwLock;

use argus_core::types::{ArgusError, ArgusResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRecord {
    pub name: String,
    pub value: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub trait KeyVault: Send + Sync {
    fn put(&self, name: &str, value: &str) -> ArgusResult<()>;
    fn get(&self, name: &str) -> ArgusResult<Option<String>>;
    fn delete(&self, name: &str) -> ArgusResult<bool>;
    fn list_names(&self) -> ArgusResult<Vec<String>>;
}

#[derive(Default)]
pub struct InMemoryKeyVault {
    store: RwLock<HashMap<String, SecretRecord>>,
}

impl InMemoryKeyVault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_record(&self, name: &str, value: &str) -> ArgusResult<SecretRecord> {
        validate_name(name)?;
        validate_value(value)?;

        let mut guard = self
            .store
            .write()
            .map_err(|e| ArgusError::Security(format!("key vault lock poisoned: {e}")))?;

        let now = Utc::now();
        let created_at = guard.get(name).map_or(now, |r| r.created_at);
        let record = SecretRecord {
            name: name.to_string(),
            value: value.to_string(),
            created_at,
            updated_at: now,
        };

        guard.insert(name.to_string(), record.clone());
        Ok(record)
    }
}

impl KeyVault for InMemoryKeyVault {
    fn put(&self, name: &str, value: &str) -> ArgusResult<()> {
        self.upsert_record(name, value)?;
        Ok(())
    }

    fn get(&self, name: &str) -> ArgusResult<Option<String>> {
        validate_name(name)?;
        let guard = self
            .store
            .read()
            .map_err(|e| ArgusError::Security(format!("key vault lock poisoned: {e}")))?;
        Ok(guard.get(name).map(|r| r.value.clone()))
    }

    fn delete(&self, name: &str) -> ArgusResult<bool> {
        validate_name(name)?;
        let mut guard = self
            .store
            .write()
            .map_err(|e| ArgusError::Security(format!("key vault lock poisoned: {e}")))?;
        Ok(guard.remove(name).is_some())
    }

    fn list_names(&self) -> ArgusResult<Vec<String>> {
        let guard = self
            .store
            .read()
            .map_err(|e| ArgusError::Security(format!("key vault lock poisoned: {e}")))?;

        let mut keys = guard.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        Ok(keys)
    }
}

fn validate_name(name: &str) -> ArgusResult<()> {
    if name.trim().is_empty() {
        return Err(ArgusError::Validation(
            "secret name cannot be empty".to_string(),
        ));
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(ArgusError::Validation(
            "secret name contains invalid characters".to_string(),
        ));
    }

    Ok(())
}

fn validate_value(value: &str) -> ArgusResult<()> {
    if value.trim().is_empty() {
        return Err(ArgusError::Validation(
            "secret value cannot be empty".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_get_delete() {
        let vault = InMemoryKeyVault::new();
        vault.put("db.password", "s3cr3t").unwrap();

        assert_eq!(
            vault.get("db.password").unwrap(),
            Some("s3cr3t".to_string())
        );
        assert!(vault.delete("db.password").unwrap());
        assert_eq!(vault.get("db.password").unwrap(), None);
    }

    #[test]
    fn test_update_keeps_creation_time() {
        let vault = InMemoryKeyVault::new();
        let first = vault.upsert_record("api.key", "v1").unwrap();
        let second = vault.upsert_record("api.key", "v2").unwrap();

        assert_eq!(first.created_at, second.created_at);
        assert!(second.updated_at >= first.updated_at);
    }

    #[test]
    fn test_list_names_sorted() {
        let vault = InMemoryKeyVault::new();
        vault.put("b", "x1xxxxxxxxxxxxxxx").unwrap();
        vault.put("a", "x2xxxxxxxxxxxxxxx").unwrap();
        assert_eq!(vault.list_names().unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn test_reject_bad_name() {
        let vault = InMemoryKeyVault::new();
        let err = vault
            .put("name with spaces", "value-123456789")
            .unwrap_err();
        assert!(format!("{err}").contains("Validation error"));
    }
}
