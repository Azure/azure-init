// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed access to diagnostic key-value entries.
//!
//! Keys follow the `prefix|vm_id|level|name|span_id` convention.
//! Values exceeding the Azure platform's 1,022-byte read limit are
//! split across multiple records.

use std::fmt;
use std::io;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::KvpStore;

/// The Azure platform reads at most 1,022 bytes from the value field
/// of each KVP record (UTF-16: 511 characters + null terminator).
/// Values longer than this must be split across multiple records.
pub const HV_KVP_AZURE_MAX_VALUE_SIZE: usize = 1022;

/// A structured diagnostic event.
///
/// Each field maps directly to a segment of the KVP key or value:
///
/// - `level`: Severity level (e.g. "INFO", "WARN", "ERROR", "DEBUG").
/// - `name`: Logical event name (e.g. "provision:user:create_user").
/// - `span_id`: Unique identifier tying the event to a span/operation.
/// - `message`: Human-readable message / payload.
/// - `timestamp`: When the event occurred.
pub struct DiagnosticEvent {
    pub level: String,
    pub name: String,
    pub span_id: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

impl DiagnosticEvent {
    pub fn new(
        level: impl Into<String>,
        name: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            level: level.into(),
            name: name.into(),
            span_id: Uuid::new_v4().to_string(),
            message: message.into(),
            timestamp: Utc::now(),
        }
    }
}

impl fmt::Display for DiagnosticEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} ({}): {}",
            self.level, self.name, self.span_id, self.message
        )
    }
}

/// Generates a unique event key by combining the event prefix, VM ID,
/// level, name, and span ID.
///
/// Format: `prefix|vm_id|level|name|span_id`
pub fn generate_event_key(
    event_prefix: &str,
    vm_id: &str,
    event_level: &str,
    event_name: &str,
    span_id: &str,
) -> String {
    format!("{event_prefix}|{vm_id}|{event_level}|{event_name}|{span_id}")
}

/// Typed diagnostic access layered on top of any `KvpStore`.
///
/// Handles event key generation and value splitting. The Azure
/// platform only reads the first 1,022 bytes of the value field per
/// record, so values exceeding that limit are split across multiple
/// `store.write()` calls with the same key.
pub struct DiagnosticsKvp<S: KvpStore> {
    store: S,
    vm_id: String,
    event_prefix: String,
}

impl<S: KvpStore> DiagnosticsKvp<S> {
    pub fn new(store: S, vm_id: &str, event_prefix: &str) -> Self {
        Self {
            store,
            vm_id: vm_id.to_string(),
            event_prefix: event_prefix.to_string(),
        }
    }

    /// Write a diagnostic event to the store.
    ///
    /// The key is generated from the event's metadata using
    /// [`generate_event_key`]. If the value exceeds 1,022 bytes it is
    /// split across multiple records with the same key.
    pub fn emit(&self, event: &DiagnosticEvent) -> io::Result<()> {
        let key = generate_event_key(
            &self.event_prefix,
            &self.vm_id,
            &event.level,
            &event.name,
            &event.span_id,
        );

        let value = &event.message;

        if value.len() <= HV_KVP_AZURE_MAX_VALUE_SIZE {
            self.store.write(&key, value)?;
        } else {
            for chunk in value.as_bytes().chunks(HV_KVP_AZURE_MAX_VALUE_SIZE) {
                let chunk_str = String::from_utf8_lossy(chunk);
                self.store.write(&key, &chunk_str)?;
            }
        }

        Ok(())
    }

    /// Read all diagnostic entries from the store, parsed into
    /// `DiagnosticEvent` structs.
    ///
    /// Only entries whose key matches the
    /// `prefix|vm_id|level|name|span_id` pattern (with 5 pipe-separated
    /// segments) are returned.
    pub fn entries(&self) -> io::Result<Vec<DiagnosticEvent>> {
        let all = self.store.entries()?;
        let mut events = Vec::new();

        for (key, value) in all {
            let parts: Vec<&str> = key.splitn(5, '|').collect();
            if parts.len() == 5 {
                events.push(DiagnosticEvent {
                    level: parts[2].to_string(),
                    name: parts[3].to_string(),
                    span_id: parts[4].to_string(),
                    message: value,
                    timestamp: Utc::now(),
                });
            }
        }

        Ok(events)
    }

    /// Access the underlying store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The VM ID this diagnostics instance is configured with.
    pub fn vm_id(&self) -> &str {
        &self.vm_id
    }

    /// The event prefix this diagnostics instance is configured with.
    pub fn event_prefix(&self) -> &str {
        &self.event_prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryKvpStore;

    #[test]
    fn test_generate_event_key() {
        let key = generate_event_key(
            "azure-init-0.1.1",
            "vm-123",
            "INFO",
            "provision:user",
            "span-abc",
        );
        assert_eq!(key, "azure-init-0.1.1|vm-123|INFO|provision:user|span-abc");
    }

    #[test]
    fn test_emit_short_value() {
        let store = InMemoryKvpStore::default();
        let diag = DiagnosticsKvp::new(store.clone(), "vm-1", "prefix");

        let event = DiagnosticEvent {
            level: "INFO".to_string(),
            name: "test_event".to_string(),
            span_id: "span-1".to_string(),
            message: "hello world".to_string(),
            timestamp: Utc::now(),
        };

        diag.emit(&event).unwrap();

        let key = "prefix|vm-1|INFO|test_event|span-1";
        assert_eq!(store.read(key).unwrap(), Some("hello world".to_string()));
    }

    #[test]
    fn test_emit_splits_long_value() {
        let store = InMemoryKvpStore::default();
        let diag = DiagnosticsKvp::new(store.clone(), "vm-1", "prefix");

        let long_message = "A".repeat(HV_KVP_AZURE_MAX_VALUE_SIZE * 2 + 50);
        let event = DiagnosticEvent {
            level: "DEBUG".to_string(),
            name: "big_event".to_string(),
            span_id: "span-2".to_string(),
            message: long_message.clone(),
            timestamp: Utc::now(),
        };

        diag.emit(&event).unwrap();

        // With InMemoryKvpStore (HashMap), only the last chunk is
        // retained since write overwrites. Verify the key exists and
        // the stored value is at most one chunk long.
        let key = "prefix|vm-1|DEBUG|big_event|span-2";
        let stored = store.read(key).unwrap().unwrap();
        assert!(stored.len() <= HV_KVP_AZURE_MAX_VALUE_SIZE);
    }

    #[test]
    fn test_emit_splits_long_value_on_hyperv_store() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::HyperVKvpStore::new(tmp.path());
        let diag = DiagnosticsKvp::new(store, "vm-1", "prefix");

        let long_message = "B".repeat(HV_KVP_AZURE_MAX_VALUE_SIZE * 3 + 10);
        let event = DiagnosticEvent {
            level: "INFO".to_string(),
            name: "split_test".to_string(),
            span_id: "span-3".to_string(),
            message: long_message,
            timestamp: Utc::now(),
        };

        diag.emit(&event).unwrap();

        // HyperVKvpStore is append-only, so entries() returns all
        // records including the split chunks.
        let store2 = crate::HyperVKvpStore::new(tmp.path());
        let entries = store2.entries().unwrap();
        assert_eq!(entries.len(), 4); // ceil((1022*3+10) / 1022) = 4

        let expected_key = "prefix|vm-1|INFO|split_test|span-3";
        for (k, v) in &entries {
            assert_eq!(k, expected_key);
            assert!(v.len() <= HV_KVP_AZURE_MAX_VALUE_SIZE);
        }
    }

    #[test]
    fn test_entries_parses_diagnostic_keys() {
        let store = InMemoryKvpStore::default();
        store
            .write("prefix|vm-1|INFO|my_event|span-1", "msg1")
            .unwrap();
        store
            .write("prefix|vm-1|ERROR|other|span-2", "msg2")
            .unwrap();
        // Non-diagnostic key should be skipped.
        store
            .write("PROVISIONING_REPORT", "result=success")
            .unwrap();

        let diag = DiagnosticsKvp::new(store, "vm-1", "prefix");
        let events = diag.entries().unwrap();

        assert_eq!(events.len(), 2);

        let levels: Vec<&str> =
            events.iter().map(|e| e.level.as_str()).collect();
        assert!(levels.contains(&"INFO"));
        assert!(levels.contains(&"ERROR"));
    }

    #[test]
    fn test_emit_uses_new_helper() {
        let store = InMemoryKvpStore::default();
        let diag = DiagnosticsKvp::new(store.clone(), "vm-1", "prefix");

        let event = DiagnosticEvent::new("WARN", "test_op", "warning msg");
        diag.emit(&event).unwrap();

        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 1);

        let (key, value) = &entries[0];
        assert!(key.contains("WARN"));
        assert!(key.contains("test_op"));
        assert_eq!(value, "warning msg");
    }
}
