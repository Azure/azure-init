// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory `KvpStore` implementation for testing.
//!
//! `InMemoryKvpStore` is a HashMap-backed test double with no
//! filesystem access. It implements `KvpStore` so that any higher
//! layer (`DiagnosticsKvp`, `ProvisioningReport`, `TracingKvpLayer`)
//! can be tested without binary encoding, flock, or tempfiles.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use crate::KvpStore;

/// A HashMap-backed KVP store with no filesystem access.
///
/// Thread-safe via `Arc<Mutex<…>>`. Drop-in replacement for any layer
/// in unit and integration tests.
#[derive(Default, Clone)]
pub struct InMemoryKvpStore {
    inner: Arc<Mutex<HashMap<String, String>>>,
}

impl KvpStore for InMemoryKvpStore {
    fn write(&self, key: &str, value: &str) -> io::Result<()> {
        let mut map = self.inner.lock().unwrap();
        map.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn read(&self, key: &str) -> io::Result<Option<String>> {
        let map = self.inner.lock().unwrap();
        Ok(map.get(key).cloned())
    }

    fn entries(&self) -> io::Result<Vec<(String, String)>> {
        let map = self.inner.lock().unwrap();
        Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }

    fn delete(&self, key: &str) -> io::Result<bool> {
        let mut map = self.inner.lock().unwrap();
        Ok(map.remove(key).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read() {
        let store = InMemoryKvpStore::default();
        store.write("key1", "value1").unwrap();
        assert_eq!(store.read("key1").unwrap(), Some("value1".to_string()));
        assert_eq!(store.read("missing").unwrap(), None);
    }

    #[test]
    fn test_write_overwrites() {
        let store = InMemoryKvpStore::default();
        store.write("key", "first").unwrap();
        store.write("key", "second").unwrap();
        assert_eq!(store.read("key").unwrap(), Some("second".to_string()));
    }

    #[test]
    fn test_entries() {
        let store = InMemoryKvpStore::default();
        store.write("a", "1").unwrap();
        store.write("b", "2").unwrap();

        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&("a".to_string(), "1".to_string())));
        assert!(entries.contains(&("b".to_string(), "2".to_string())));
    }

    #[test]
    fn test_delete() {
        let store = InMemoryKvpStore::default();
        store.write("key", "value").unwrap();

        assert!(store.delete("key").unwrap());
        assert_eq!(store.read("key").unwrap(), None);
        assert!(!store.delete("key").unwrap());
    }

    #[test]
    fn test_clone_shares_state() {
        let store = InMemoryKvpStore::default();
        let clone = store.clone();

        store.write("key", "value").unwrap();
        assert_eq!(clone.read("key").unwrap(), Some("value".to_string()));
    }
}
