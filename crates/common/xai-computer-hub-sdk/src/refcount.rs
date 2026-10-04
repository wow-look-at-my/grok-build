//! Generic refcounted-binding helper used by the connection's bound-session set.

use std::hash::Hash;

use dashmap::DashMap;

/// Refcounted set keyed by `K`.
#[derive(Debug, Default)]
pub struct RefCountedSet<K: Eq + Hash> {
    counts: DashMap<K, u64>,
}

impl<K: Eq + Hash> RefCountedSet<K> {
    /// Empty set.
    pub fn new() -> Self {
        Self {
            counts: DashMap::new(),
        }
    }

    /// Increment `key`'s refcount.
    pub fn increment(&self, key: K) -> (u64, u64)
    where
        K: Clone,
    {
        let mut entry = self.counts.entry(key).or_insert(0);
        let prev = *entry;
        *entry = prev.saturating_add(1);
        (prev, *entry)
    }

    /// Decrement `key`'s refcount. Returns the post-decrement count;
    /// `Some(0)` means the entry was removed and callers should fire the
    /// protocol-level unregister.
    pub fn decrement(&self, key: &K) -> Option<u64> {
        let mut current = None;
        self.counts.remove_if_mut(key, |_, value| {
            *value = value.saturating_sub(1);
            current = Some(*value);
            *value == 0
        });
        current
    }

    /// Snapshot the live keys. Allocates a fresh `Vec` — only used by
    /// the reconnect-replay path which fires once per disconnect.
    pub fn snapshot_keys(&self) -> Vec<K>
    where
        K: Clone,
    {
        self.counts.iter().map(|kv| kv.key().clone()).collect()
    }

    /// `true` when no key has a non-zero refcount.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// Number of distinct live keys.
    pub fn len(&self) -> usize {
        self.counts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_returns_new_count() {
        let set = RefCountedSet::<&'static str>::new();
        assert_eq!(set.increment("a"), (0, 1));
        assert_eq!(set.increment("a"), (1, 2));
        assert_eq!(set.increment("b"), (0, 1));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn decrement_removes_at_zero() {
        let set = RefCountedSet::<&'static str>::new();
        set.increment("a");
        set.increment("a");
        assert_eq!(set.decrement(&"a"), Some(1));
        assert!(!set.is_empty());
        assert_eq!(set.decrement(&"a"), Some(0));
        assert!(set.is_empty());
    }

    #[test]
    fn decrement_unknown_returns_none() {
        let set = RefCountedSet::<&'static str>::new();
        assert!(set.decrement(&"missing").is_none());
    }

    #[test]
    fn increment_saturates_at_u64_max() {
        let set = RefCountedSet::<&'static str>::new();
        // Pre-load the entry to MAX-1 via direct DashMap access.
        set.counts.insert("max", u64::MAX - 1);
        assert_eq!(set.increment("max"), (u64::MAX - 1, u64::MAX));
        assert_eq!(set.increment("max"), (u64::MAX, u64::MAX));
    }
}
