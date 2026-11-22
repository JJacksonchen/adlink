//! Lock-free in-memory analytics query cache (epoch-invalidated) and idempotency store.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde_json::Value;

struct CachedEntry {
    epoch: u64,
    expires_at: Instant,
    payload: Value,
}

/// Concurrent epoch-invalidated read-through cache for heavy analytics and attribution queries.
/// Any committed write in SQLite WAL bumps `storage.write_epoch()`, automatically invalidating
/// stale entries in O(1) without explicit key purging.
pub struct AnalyticsQueryCache {
    ttl: Duration,
    max_entries: usize,
    entries: DashMap<String, CachedEntry>,
}

impl AnalyticsQueryCache {
    pub fn new(ttl_secs: u64, max_entries: usize) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_secs),
            max_entries,
            entries: DashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.ttl.is_zero() && self.max_entries > 0
    }

    /// Returns cached payload only if TTL has not elapsed and `entry.epoch == current_epoch`.
    pub fn get(&self, key: &str, current_epoch: u64) -> Option<Value> {
        if !self.is_enabled() {
            return None;
        }
        let now = Instant::now();
        let valid = self
            .entries
            .get(key)
            .filter(|e| e.epoch == current_epoch && e.expires_at > now)
            .map(|e| e.payload.clone());
        if valid.is_none() {
            self.entries.remove(key);
        }
        valid
    }

    /// Inserts a fresh response for `current_epoch`, pruning expired or excess entries when full.
    pub fn insert(&self, key: String, current_epoch: u64, payload: Value) {
        if !self.is_enabled() {
            return;
        }
        let now = Instant::now();
        if self.entries.len() >= self.max_entries {
            self.entries.retain(|_, v| v.expires_at > now && v.epoch == current_epoch);
            if self.entries.len() >= self.max_entries {
                if let Some(oldest_key) = self
                    .entries
                    .iter()
                    .min_by_key(|e| e.expires_at)
                    .map(|e| e.key().clone())
                {
                    self.entries.remove(&oldest_key);
                }
            }
        }
        self.entries.insert(
            key,
            CachedEntry {
                epoch: current_epoch,
                expires_at: now + self.ttl,
                payload,
            },
        );
    }
}

struct StoredIdempotency {
    expires_at: Instant,
    status: u16,
    body: Value,
}

/// Deduplicates mutating requests carrying an `Idempotency-Key` HTTP header.
pub struct IdempotencyStore {
    ttl: Duration,
    max_entries: usize,
    entries: DashMap<String, StoredIdempotency>,
}

impl IdempotencyStore {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_secs),
            max_entries: 4096,
            entries: DashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.ttl.is_zero()
    }

    pub fn get(&self, key: &str) -> Option<(u16, Value)> {
        if !self.is_enabled() {
            return None;
        }
        let now = Instant::now();
        let found = self
            .entries
            .get(key)
            .filter(|e| e.expires_at > now)
            .map(|e| (e.status, e.body.clone()));
        if found.is_none() {
            self.entries.remove(key);
        }
        found
    }

    pub fn put(&self, key: String, status: u16, body: Value) {
        if !self.is_enabled() {
            return;
        }
        let now = Instant::now();
        if self.entries.len() >= self.max_entries {
            self.entries.retain(|_, v| v.expires_at > now);
            if self.entries.len() >= self.max_entries {
                if let Some(evict) = self
                    .entries
                    .iter()
                    .min_by_key(|e| e.expires_at)
                    .map(|e| e.key().clone())
                {
                    self.entries.remove(&evict);
                }
            }
        }
        self.entries.insert(
            key,
            StoredIdempotency {
                expires_at: now + self.ttl,
                status,
                body,
            },
        );
    }
}
