//! Asynchronous scoped event bus for decoupled inter-plugin communication.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

/// Structured event published through the microkernel event bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelEvent {
    /// Dot-separated event topic (e.g. `sync.completed`, `ad.mutated`, `plugin.mounted`).
    pub topic: String,
    /// ID of the plugin or subsystem that emitted the event.
    pub source_plugin: String,
    /// Arbitrary structured event payload.
    pub payload: Value,
    /// UTC timestamp of emission.
    pub timestamp: DateTime<Utc>,
}

impl KernelEvent {
    /// Convenience constructor stamping the current UTC time.
    pub fn new(
        topic: impl Into<String>,
        source_plugin: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            topic: topic.into(),
            source_plugin: source_plugin.into(),
            payload,
            timestamp: Utc::now(),
        }
    }
}

/// Scoped subscription handle; automatically stops receiving once the owning plugin unloads.
pub struct ScopedSubscription {
    topic_prefix: Option<String>,
    receiver: broadcast::Receiver<KernelEvent>,
    active: Arc<AtomicBool>,
}

impl ScopedSubscription {
    /// Receives the next matching [`KernelEvent`], returning `None` if the subscription was
    /// disposed (plugin unloaded) or the bus closed.
    pub async fn recv(&mut self) -> Option<KernelEvent> {
        loop {
            if !self.active.load(Ordering::SeqCst) {
                return None;
            }
            match self.receiver.recv().await {
                Ok(evt) => {
                    if !self.active.load(Ordering::SeqCst) {
                        return None;
                    }
                    if let Some(prefix) = &self.topic_prefix {
                        if !evt.topic.starts_with(prefix) {
                            continue;
                        }
                    }
                    return Some(evt);
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "event subscriber lagged behind broadcast channel");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

/// Broadcast-backed event bus shared across the microkernel.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<KernelEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl EventBus {
    /// Creates a new event bus with the specified bounded broadcast ring capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Emits an event to all active subscribers. Silently ignores zero-subscriber state.
    pub fn emit(&self, event: KernelEvent) {
        let _ = self.sender.send(event);
    }

    /// Subscribes to events matching an optional topic prefix, returning both the receiver and an
    /// activation flag bound to the caller's `EffectScope`.
    pub(crate) fn subscribe_scoped(
        &self,
        topic_prefix: Option<String>,
    ) -> (ScopedSubscription, Arc<AtomicBool>) {
        let active = Arc::new(AtomicBool::new(true));
        let sub = ScopedSubscription {
            topic_prefix,
            receiver: self.sender.subscribe(),
            active: Arc::clone(&active),
        };
        (sub, active)
    }
}
