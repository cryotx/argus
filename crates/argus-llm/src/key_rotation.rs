//! API key rotation with cooldown and error handling.
//!
//! Each provider can have multiple API keys that are rotated round-robin.
//! When a key encounters a rate-limit or auth error, it enters cooldown
//! and the rotator moves to the next key.

use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

// ============================================================
// Key state
// ============================================================

/// State of a single API key.
#[derive(Clone, Debug)]
pub struct ApiKeyState {
    /// The actual key value (in production, should use SecretString).
    key: String,
    /// Whether this key has been permanently exhausted (e.g., quota exceeded).
    pub is_exhausted: bool,
    /// Whether this key is temporarily rate-limited.
    pub is_rate_limited: bool,
    /// Cooldown expiry time (key is unusable until this time).
    pub cooldown_until: Option<DateTime<Utc>>,
    /// Total tokens consumed by this key.
    pub total_tokens_used: u64,
    /// Last error message.
    pub last_error: Option<String>,
    /// When this key was last used.
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ApiKeyState {
    fn new(key: String) -> Self {
        Self {
            key,
            is_exhausted: false,
            is_rate_limited: false,
            cooldown_until: None,
            total_tokens_used: 0,
            last_error: None,
            last_used_at: None,
        }
    }

    /// Check if this key is currently available.
    fn is_available(&self) -> bool {
        if self.is_exhausted {
            return false;
        }
        if self.is_rate_limited {
            // Check if cooldown has expired
            if let Some(until) = self.cooldown_until {
                if Utc::now() < until {
                    return false;
                }
                // Cooldown expired — key is available again
                // (we can't mutate here, but the caller will clear it)
            }
        }
        true
    }
}

// ============================================================
// Key rotator
// ============================================================

/// Manages round-robin rotation across multiple API keys.
pub struct KeyRotator {
    /// All keys with their state.
    keys: RwLock<Vec<ApiKeyState>>,
    /// Current rotation index.
    current_index: AtomicUsize,
    /// Provider ID (for logging).
    provider_id: String,
}

/// Result of getting an available key.
#[derive(Clone, Debug)]
pub struct SelectedKey {
    /// The API key value.
    pub key: String,
    /// The index into the key array (needed for error reporting).
    pub index: usize,
}

/// Classification of API errors for key rotation decisions.
#[derive(Debug)]
pub enum ApiErrorKind {
    /// 429: Rate limited — temporary cooldown.
    RateLimited { retry_after_seconds: u64 },
    /// 401/403: Unauthorized or insufficient quota — permanent.
    Unauthorized,
    /// Insufficient quota / billing issue — permanent.
    QuotaExhausted,
    /// Transient error (500, timeout, network) — don't mark key as bad.
    Transient,
}

impl KeyRotator {
    /// Create a new key rotator with the given keys.
    pub fn new(provider_id: &str, keys: Vec<String>) -> Self {
        let key_states: Vec<ApiKeyState> = keys.into_iter().map(ApiKeyState::new).collect();
        Self {
            keys: RwLock::new(key_states),
            current_index: AtomicUsize::new(0),
            provider_id: provider_id.to_string(),
        }
    }

    /// Create a key rotator for providers that don't need API keys (e.g., Ollama).
    pub fn none(provider_id: &str) -> Self {
        Self::new(provider_id, vec!["none".to_string()])
    }

    /// Get the next available key (round-robin with skip).
    ///
    /// Returns `None` if all keys are exhausted or rate-limited.
    pub async fn get_available_key(&self) -> Option<SelectedKey> {
        let mut keys = self.keys.write().await;
        let len = keys.len();
        if len == 0 {
            return None;
        }

        let start = self.current_index.load(Ordering::Relaxed);

        for i in 0..len {
            let idx = (start + i) % len;
            let key_state = &mut keys[idx];

            // Clear expired cooldowns
            if key_state.is_rate_limited {
                if let Some(until) = key_state.cooldown_until {
                    if Utc::now() >= until {
                        key_state.is_rate_limited = false;
                        key_state.cooldown_until = None;
                        tracing::info!(
                            provider = self.provider_id,
                            key_index = idx,
                            "API key cooldown expired, key available again"
                        );
                    }
                }
            }

            if key_state.is_available() {
                key_state.last_used_at = Some(Utc::now());
                self.current_index.store((idx + 1) % len, Ordering::Relaxed);
                return Some(SelectedKey {
                    key: key_state.key.clone(),
                    index: idx,
                });
            }
        }

        tracing::warn!(
            provider = self.provider_id,
            total_keys = len,
            "All API keys exhausted or rate-limited"
        );
        None
    }

    /// Report an error for a specific key.
    pub async fn report_error(&self, key_index: usize, error: ApiErrorKind) {
        let mut keys = self.keys.write().await;
        if key_index >= keys.len() {
            return;
        }

        let key_state = &mut keys[key_index];

        match error {
            ApiErrorKind::RateLimited {
                retry_after_seconds,
            } => {
                key_state.is_rate_limited = true;
                key_state.cooldown_until =
                    Some(Utc::now() + chrono::Duration::seconds(retry_after_seconds as i64));
                key_state.last_error =
                    Some(format!("Rate limited, cooldown {}s", retry_after_seconds));

                tracing::info!(
                    provider = self.provider_id,
                    key_index = key_index,
                    retry_after_seconds = retry_after_seconds,
                    "API key rate-limited, entering cooldown"
                );
            }
            ApiErrorKind::Unauthorized | ApiErrorKind::QuotaExhausted => {
                key_state.is_exhausted = true;
                key_state.last_error = Some("Permanently exhausted".to_string());

                tracing::warn!(
                    provider = self.provider_id,
                    key_index = key_index,
                    "API key permanently exhausted"
                );
            }
            ApiErrorKind::Transient => {
                key_state.last_error = Some("Transient error".to_string());
                // Don't mark as exhausted or rate limited
            }
        }
    }

    /// Report successful usage for a key.
    pub async fn report_success(&self, key_index: usize, tokens_used: u32) {
        let mut keys = self.keys.write().await;
        if key_index >= keys.len() {
            return;
        }

        keys[key_index].total_tokens_used += tokens_used as u64;
        keys[key_index].last_error = None;
    }

    /// Get the number of available keys.
    pub async fn available_count(&self) -> usize {
        let keys = self.keys.read().await;
        keys.iter().filter(|k| k.is_available()).count()
    }

    /// Get the total number of keys.
    pub async fn total_count(&self) -> usize {
        self.keys.read().await.len()
    }

    /// Add a new key to the rotator.
    pub async fn add_key(&self, key: String) {
        let mut keys = self.keys.write().await;
        keys.push(ApiKeyState::new(key));
        tracing::info!(
            provider = self.provider_id,
            total_keys = keys.len(),
            "New API key added"
        );
    }

    /// Remove a key by index.
    pub async fn remove_key(&self, key_index: usize) -> bool {
        let mut keys = self.keys.write().await;
        if key_index >= keys.len() {
            return false;
        }
        keys.remove(key_index);
        tracing::info!(
            provider = self.provider_id,
            removed_index = key_index,
            total_keys = keys.len(),
            "API key removed"
        );
        true
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_round_robin_rotation() {
        let rotator = KeyRotator::new(
            "test",
            vec![
                "key-a".to_string(),
                "key-b".to_string(),
                "key-c".to_string(),
            ],
        );

        let k1 = rotator.get_available_key().await.unwrap();
        assert_eq!(k1.key, "key-a");
        assert_eq!(k1.index, 0);

        let k2 = rotator.get_available_key().await.unwrap();
        assert_eq!(k2.key, "key-b");
        assert_eq!(k2.index, 1);

        let k3 = rotator.get_available_key().await.unwrap();
        assert_eq!(k3.key, "key-c");
        assert_eq!(k3.index, 2);

        // Wraps around
        let k4 = rotator.get_available_key().await.unwrap();
        assert_eq!(k4.key, "key-a");
        assert_eq!(k4.index, 0);
    }

    #[tokio::test]
    async fn test_skip_exhausted_key() {
        let rotator = KeyRotator::new(
            "test",
            vec![
                "key-a".to_string(),
                "key-b".to_string(),
                "key-c".to_string(),
            ],
        );

        // Exhaust key-a
        rotator.report_error(0, ApiErrorKind::Unauthorized).await;

        let k1 = rotator.get_available_key().await.unwrap();
        assert_eq!(k1.key, "key-b");
    }

    #[tokio::test]
    async fn test_all_keys_exhausted() {
        let rotator = KeyRotator::new("test", vec!["key-a".to_string(), "key-b".to_string()]);

        rotator.report_error(0, ApiErrorKind::Unauthorized).await;
        rotator.report_error(1, ApiErrorKind::QuotaExhausted).await;

        assert!(rotator.get_available_key().await.is_none());
    }

    #[tokio::test]
    async fn test_rate_limited_with_cooldown() {
        let rotator = KeyRotator::new("test", vec!["key-a".to_string(), "key-b".to_string()]);

        // Rate-limit key-a for 60 seconds
        rotator
            .report_error(
                0,
                ApiErrorKind::RateLimited {
                    retry_after_seconds: 60,
                },
            )
            .await;

        // Should skip key-a
        let k1 = rotator.get_available_key().await.unwrap();
        assert_eq!(k1.key, "key-b");
    }

    #[tokio::test]
    async fn test_report_success() {
        let rotator = KeyRotator::new("test", vec!["key-a".to_string()]);

        let k = rotator.get_available_key().await.unwrap();
        rotator.report_success(k.index, 1000).await;

        // Key should still be available
        let k2 = rotator.get_available_key().await.unwrap();
        assert_eq!(k2.key, "key-a");
    }

    #[tokio::test]
    async fn test_add_and_remove_key() {
        let rotator = KeyRotator::new("test", vec!["key-a".to_string()]);
        assert_eq!(rotator.total_count().await, 1);

        rotator.add_key("key-b".to_string()).await;
        assert_eq!(rotator.total_count().await, 2);

        assert!(rotator.remove_key(0).await);
        assert_eq!(rotator.total_count().await, 1);

        // Out of bounds
        assert!(!rotator.remove_key(5).await);
    }

    #[tokio::test]
    async fn test_available_count() {
        let rotator = KeyRotator::new(
            "test",
            vec![
                "key-a".to_string(),
                "key-b".to_string(),
                "key-c".to_string(),
            ],
        );
        assert_eq!(rotator.available_count().await, 3);

        rotator.report_error(0, ApiErrorKind::Unauthorized).await;
        assert_eq!(rotator.available_count().await, 2);
    }

    #[tokio::test]
    async fn test_empty_rotator() {
        let rotator = KeyRotator::new("test", vec![]);
        assert!(rotator.get_available_key().await.is_none());
        assert_eq!(rotator.available_count().await, 0);
    }

    #[tokio::test]
    async fn test_transient_error_doesnt_exhaust() {
        let rotator = KeyRotator::new("test", vec!["key-a".to_string()]);

        rotator.report_error(0, ApiErrorKind::Transient).await;

        // Key should still be available
        let k = rotator.get_available_key().await.unwrap();
        assert_eq!(k.key, "key-a");
    }
}
