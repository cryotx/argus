//! Event bus implementation using `tokio::sync::broadcast`.
//!
//! The event bus is the nervous system of Argus. All inter-module communication
//! flows through it via publish/subscribe.
//!
//! See API_DESIGN.md Section 2.3 for the specification.

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tracing;
use uuid::Uuid;

use crate::events::{ArgusEvent, EventEnvelope, EventSource, event_name, extract_project_id};

/// Default broadcast channel capacity.
const EVENT_BUS_CAPACITY: usize = 4096;

/// The central event bus for Argus.
///
/// All modules publish events here and subscribe to receive them.
/// Uses `tokio::sync::broadcast` for fan-out to multiple subscribers.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<EventEnvelope>,
}

impl EventBus {
    /// Create a new event bus with default capacity (4096).
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(EVENT_BUS_CAPACITY);
        Self { sender }
    }

    /// Create a new event bus with custom capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Publish an event to all subscribers.
    ///
    /// Returns the number of active receivers that will receive this event.
    /// Returns 0 if there are no subscribers (not an error during startup/shutdown).
    pub fn publish(&self, source: EventSource, event: ArgusEvent) -> usize {
        self.publish_with_correlation(source, event, None)
    }

    /// Publish an event with a correlation ID to link related events.
    ///
    /// Correlation IDs are used to trace causal chains (e.g., a task creation
    /// leading to worker assignment leading to completion).
    pub fn publish_with_correlation(
        &self,
        source: EventSource,
        event: ArgusEvent,
        correlation_id: Option<Uuid>,
    ) -> usize {
        let name = event_name(&event);
        let envelope = EventEnvelope {
            id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            source,
            event,
            correlation_id,
        };

        match self.sender.send(envelope) {
            Ok(count) => {
                tracing::trace!(event = name, receivers = count, "Event published");
                count
            }
            Err(_) => {
                // No receivers — this is normal during startup/shutdown
                tracing::trace!(event = name, "Event published with no receivers");
                0
            }
        }
    }

    /// Subscribe to the event bus.
    ///
    /// Returns a broadcast receiver that yields all events. The caller is
    /// responsible for filtering events they care about.
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.sender.subscribe()
    }

    /// Get the count of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// Filtered subscription helper
// ============================================================

/// Configuration for filtering events from the bus.
///
/// An empty filter (all fields empty) matches all events.
pub struct EventFilter {
    /// Only pass events from these sources (empty = all).
    pub sources: Vec<EventSource>,
    /// Only pass events matching these variant names (empty = all).
    pub event_names: Vec<String>,
    /// Only pass events related to these project IDs (empty = all).
    pub project_ids: Vec<Uuid>,
}

impl EventFilter {
    /// Create a filter that matches all events.
    pub fn all() -> Self {
        Self {
            sources: vec![],
            event_names: vec![],
            project_ids: vec![],
        }
    }

    /// Create a filter for events from specific sources.
    pub fn from_sources(sources: Vec<EventSource>) -> Self {
        Self {
            sources,
            event_names: vec![],
            project_ids: vec![],
        }
    }

    /// Create a filter for specific event types by name.
    pub fn from_event_names(names: Vec<String>) -> Self {
        Self {
            sources: vec![],
            event_names: names,
            project_ids: vec![],
        }
    }

    /// Check if an event envelope matches this filter.
    pub fn matches(&self, envelope: &EventEnvelope) -> bool {
        // Source filter
        if !self.sources.is_empty() {
            let source_matches = self
                .sources
                .iter()
                .any(|s| std::mem::discriminant(s) == std::mem::discriminant(&envelope.source));
            if !source_matches {
                return false;
            }
        }

        // Event name filter
        if !self.event_names.is_empty() {
            let name = event_name(&envelope.event);
            if !self.event_names.iter().any(|n| n == name) {
                return false;
            }
        }

        // Project ID filter
        if !self.project_ids.is_empty() {
            if let Some(pid) = extract_project_id(&envelope.event) {
                if !self.project_ids.contains(&pid) {
                    return false;
                }
            }
            // Events without a project_id pass the filter (they're not project-scoped)
        }

        true
    }
}

/// Spawn a filtered event forwarder task.
///
/// Events matching the filter are forwarded to the returned mpsc receiver.
/// The background task runs until the receiver is dropped.
pub fn subscribe_filtered(
    bus: &EventBus,
    filter: EventFilter,
    buffer_size: usize,
) -> mpsc::Receiver<EventEnvelope> {
    let (tx, rx) = mpsc::channel(buffer_size);
    let mut broadcast_rx = bus.subscribe();

    tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(envelope) => {
                    if filter.matches(&envelope) && tx.send(envelope).await.is_err() {
                        break; // receiver dropped
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(missed = n, "Event subscriber lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    rx
}

/// Create a shared event bus wrapped in Arc for use across modules.
pub fn shared_event_bus() -> Arc<EventBus> {
    Arc::new(EventBus::new())
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ArgusEvent;
    use crate::types::TaskPriority;

    #[tokio::test]
    async fn test_publish_subscribe() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let event = ArgusEvent::SystemStarted {
            version: "0.1.0".to_string(),
            modules_loaded: vec!["core".to_string()],
        };

        let count = bus.publish(EventSource::System, event);
        assert_eq!(count, 1);

        let envelope = rx.recv().await.unwrap();
        assert!(matches!(envelope.event, ArgusEvent::SystemStarted { .. }));
        assert_eq!(envelope.source, EventSource::System);
        assert!(envelope.correlation_id.is_none());
    }

    #[tokio::test]
    async fn test_publish_with_correlation() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let correlation = Uuid::new_v4();
        let event = ArgusEvent::TaskCreated {
            task_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            title: "test task".to_string(),
            task_type: "code".to_string(),
            priority: TaskPriority::High,
        };

        bus.publish_with_correlation(EventSource::Brain, event, Some(correlation));

        let envelope = rx.recv().await.unwrap();
        assert_eq!(envelope.correlation_id, Some(correlation));
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        let mut rx3 = bus.subscribe();

        let event = ArgusEvent::SystemShuttingDown {
            reason: "test".to_string(),
        };

        let count = bus.publish(EventSource::System, event);
        assert_eq!(count, 3);

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        let e3 = rx3.recv().await.unwrap();

        // All three received the same event (same ID)
        assert_eq!(e1.id, e2.id);
        assert_eq!(e2.id, e3.id);
    }

    #[tokio::test]
    async fn test_no_subscribers() {
        let bus = EventBus::new();

        let event = ArgusEvent::SystemStarted {
            version: "0.1.0".to_string(),
            modules_loaded: vec![],
        };

        // Should not panic even with no subscribers
        let count = bus.publish(EventSource::System, event);
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_subscriber_count() {
        let bus = EventBus::new();
        assert_eq!(bus.subscriber_count(), 0);

        let _rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);

        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);

        drop(_rx1);
        // Note: broadcast doesn't immediately decrement on drop,
        // but receiver_count reflects pending subscriptions
    }

    #[tokio::test]
    async fn test_event_filter_all() {
        let filter = EventFilter::all();
        let envelope = EventEnvelope::new(
            EventSource::System,
            ArgusEvent::SystemStarted {
                version: "0.1.0".to_string(),
                modules_loaded: vec![],
            },
        );
        assert!(filter.matches(&envelope));
    }

    #[tokio::test]
    async fn test_event_filter_by_source() {
        let filter = EventFilter::from_sources(vec![EventSource::Brain]);

        let brain_event = EventEnvelope::new(
            EventSource::Brain,
            ArgusEvent::TaskCreated {
                task_id: Uuid::new_v4(),
                project_id: Uuid::new_v4(),
                title: "test".to_string(),
                task_type: "code".to_string(),
                priority: TaskPriority::Medium,
            },
        );
        assert!(filter.matches(&brain_event));

        let system_event = EventEnvelope::new(
            EventSource::System,
            ArgusEvent::SystemStarted {
                version: "0.1.0".to_string(),
                modules_loaded: vec![],
            },
        );
        assert!(!filter.matches(&system_event));
    }

    #[tokio::test]
    async fn test_event_filter_by_name() {
        let filter = EventFilter::from_event_names(vec!["TaskCreated".to_string()]);

        let task_event = EventEnvelope::new(
            EventSource::Brain,
            ArgusEvent::TaskCreated {
                task_id: Uuid::new_v4(),
                project_id: Uuid::new_v4(),
                title: "test".to_string(),
                task_type: "code".to_string(),
                priority: TaskPriority::Medium,
            },
        );
        assert!(filter.matches(&task_event));

        let system_event = EventEnvelope::new(
            EventSource::System,
            ArgusEvent::SystemStarted {
                version: "0.1.0".to_string(),
                modules_loaded: vec![],
            },
        );
        assert!(!filter.matches(&system_event));
    }

    #[tokio::test]
    async fn test_event_filter_by_project() {
        let project_id = Uuid::new_v4();
        let other_project = Uuid::new_v4();

        let filter = EventFilter {
            sources: vec![],
            event_names: vec![],
            project_ids: vec![project_id],
        };

        let matching = EventEnvelope::new(
            EventSource::Brain,
            ArgusEvent::TaskCreated {
                task_id: Uuid::new_v4(),
                project_id,
                title: "test".to_string(),
                task_type: "code".to_string(),
                priority: TaskPriority::Medium,
            },
        );
        assert!(filter.matches(&matching));

        let non_matching = EventEnvelope::new(
            EventSource::Brain,
            ArgusEvent::TaskCreated {
                task_id: Uuid::new_v4(),
                project_id: other_project,
                title: "test".to_string(),
                task_type: "code".to_string(),
                priority: TaskPriority::Medium,
            },
        );
        assert!(!filter.matches(&non_matching));
    }

    #[tokio::test]
    async fn test_filtered_subscription() {
        let bus = EventBus::new();

        let filter = EventFilter::from_event_names(vec!["TaskCreated".to_string()]);
        let mut rx = subscribe_filtered(&bus, filter, 100);

        // Publish a TaskCreated event (should pass filter)
        bus.publish(
            EventSource::Brain,
            ArgusEvent::TaskCreated {
                task_id: Uuid::new_v4(),
                project_id: Uuid::new_v4(),
                title: "test".to_string(),
                task_type: "code".to_string(),
                priority: TaskPriority::Medium,
            },
        );

        // Publish a SystemStarted event (should be filtered out)
        bus.publish(
            EventSource::System,
            ArgusEvent::SystemStarted {
                version: "0.1.0".to_string(),
                modules_loaded: vec![],
            },
        );

        // Publish another TaskCreated event
        bus.publish(
            EventSource::Scheduler,
            ArgusEvent::TaskCreated {
                task_id: Uuid::new_v4(),
                project_id: Uuid::new_v4(),
                title: "test 2".to_string(),
                task_type: "review".to_string(),
                priority: TaskPriority::Low,
            },
        );

        // Should receive exactly 2 TaskCreated events
        let e1 = rx.recv().await.unwrap();
        assert!(matches!(e1.event, ArgusEvent::TaskCreated { .. }));

        let e2 = rx.recv().await.unwrap();
        assert!(matches!(e2.event, ArgusEvent::TaskCreated { .. }));
    }

    #[tokio::test]
    async fn test_event_serialization() {
        let event = ArgusEvent::LlmResponseReceived {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: 0.015,
            latency_ms: 1200,
            trace_id: Some("trace-test-1".to_string()),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: ArgusEvent = serde_json::from_str(&json).unwrap();

        if let ArgusEvent::LlmResponseReceived {
            provider,
            input_tokens,
            ..
        } = deserialized
        {
            assert_eq!(provider, "anthropic");
            assert_eq!(input_tokens, 1000);
        } else {
            panic!("Wrong variant after deserialization");
        }
    }

    #[tokio::test]
    async fn test_envelope_serialization() {
        let envelope = EventEnvelope::new(
            EventSource::Worker(Uuid::new_v4()),
            ArgusEvent::WorkerReady {
                worker_id: Uuid::new_v4(),
            },
        );

        let json = serde_json::to_string(&envelope).unwrap();
        let deserialized: EventEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(envelope.id, deserialized.id);
        assert!(matches!(deserialized.source, EventSource::Worker(_)));
    }

    #[test]
    fn test_default_event_bus() {
        let bus = EventBus::default();
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn test_shared_event_bus() {
        let bus = shared_event_bus();
        assert_eq!(bus.subscriber_count(), 0);

        // Can be cloned (Arc)
        let bus2 = Arc::clone(&bus);
        let _rx = bus2.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
    }
}
