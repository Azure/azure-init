// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed diagnostics layer over the raw
//! [`KvpPoolStore`](crate::KvpPoolStore) key/value API.
//!
//! Where [`KvpPoolStore`](crate::KvpPoolStore) treats keys and values as
//! opaque bytes, [`DiagnosticsKvp`] understands the telemetry
//! conventions azure-init writes into the guest pool:
//!
//! - **Event keys** encode structured metadata as a five-segment,
//!   pipe-delimited string
//!   (`<prefix>|<vm_id>|<level>|<name>|<event_id>`).
//! - **Chunking**: a single KVP record caps the value at the store's
//!   per-record limit (see [`MAX_CHUNK_BYTES`] for the safe-mode value).
//!   Longer messages are split at UTF-8 codepoint boundaries into
//!   multiple records written atomically under a single lock. Each chunk
//!   gets a unique key — the event key with a `|<subevent_index>` suffix
//!   (`<prefix>|<vm_id>|<level>|<name>|<event_id>|<subevent_index>`),
//!   matching cloud-init's naming — so the Hyper-V host, which keeps only
//!   one record per key, retains every chunk. The chunks are regrouped
//!   into one event on read.
//! - **Classification**: [`records`](DiagnosticsKvp::records) sorts every
//!   stored record into a [`DiagnosticRecord`] — a reassembled
//!   [`DiagnosticEvent`], an unstructured [`Raw`](DiagnosticRecord::Raw)
//!   record such as `PROVISIONING_REPORT`, or a
//!   [`Malformed`](DiagnosticRecord::Malformed) event key.
//!
//! This module is policy only: all locking, size enforcement, and
//! on-disk encoding stay in [`KvpPoolStore`](crate::KvpPoolStore).
//!
//! # Example
//!
//! ```
//! use libazureinit_kvp::{
//!     DiagnosticEvent, DiagnosticsKvp, KvpPool, KvpPoolStore, PoolMode,
//!     MAX_CHUNK_BYTES,
//! };
//! use tracing::Level;
//!
//! # fn main() -> Result<(), libazureinit_kvp::KvpError> {
//! let dir = std::env::temp_dir()
//!     .join(format!("libazureinit-kvp-doc-{}", std::process::id()));
//! std::fs::create_dir_all(&dir)?;
//! let store = KvpPoolStore::new_in(KvpPool::Guest, &dir, PoolMode::Safe)?;
//! store.clear()?;
//!
//! let diagnostics =
//!     DiagnosticsKvp::new(store, "vm-1234", "azure-init-doc");
//!
//! // A short event lands in a single record.
//! diagnostics.emit(&DiagnosticEvent::new(
//!     Level::INFO,
//!     "user:create_user",
//!     "Creating user azureuser",
//! ))?;
//!
//! // A long message is split across records each with a unique
//! // `|<subevent_index>`-suffixed key, and reassembled on read.
//! let long = "x".repeat(MAX_CHUNK_BYTES * 2 + 10);
//! diagnostics
//!     .emit(&DiagnosticEvent::new(Level::DEBUG, "config:dump", &long))?;
//!
//! let events = diagnostics.events()?;
//! assert_eq!(events.len(), 2);
//! assert_eq!(events[1].message.len(), MAX_CHUNK_BYTES * 2 + 10);
//!
//! # std::fs::remove_dir_all(&dir).ok();
//! # Ok(())
//! # }
//! ```

use tracing::Level;
use uuid::Uuid;

use crate::{KvpError, KvpPoolStore};

/// Maximum number of value bytes per KVP record under
/// [`PoolMode::Safe`](crate::PoolMode::Safe).
///
/// This matches a safe-mode store's
/// [`KvpPoolStore::max_value_size`](crate::KvpPoolStore::max_value_size).
/// [`DiagnosticsKvp::emit`] splits messages longer than the store's
/// actual limit, so an [`Unsafe`](crate::PoolMode::Unsafe) store uses
/// its larger capacity; this constant is the conservative reference
/// value used throughout the diagnostics conventions.
pub const MAX_CHUNK_BYTES: usize = 1022;

/// Delimiter separating the segments of a diagnostic event key.
const EVENT_KEY_DELIMITER: char = '|';

/// Format a diagnostic event key as its `|`-delimited on-disk string:
/// `<prefix>|<vm_id>|<level>|<name>|<event_id>`.
///
/// [`classify_key`] is the inverse. For example:
///
/// ```text
/// azure-init-0.1.0|3f2504e0-4f89-41d3-9a0c-0305e82c3301|INFO|user:create_user|8f3e9c4a-...
/// ```
fn format_event_key(
    prefix: &str,
    vm_id: &str,
    level: Level,
    name: &str,
    event_id: &str,
) -> String {
    let d = EVENT_KEY_DELIMITER;
    format!("{prefix}{d}{vm_id}{d}{level}{d}{name}{d}{event_id}")
}

/// Outcome of inspecting a raw pool key.
enum KeyClass<'a> {
    /// The key is a well-formed event key.
    Event {
        prefix: &'a str,
        vm_id: &'a str,
        level: Level,
        name: &'a str,
        event_id: &'a str,
    },
    /// The key has five segments but is not a valid event.
    Malformed { reason: String },
    /// The key is not an event key (e.g. `PROVISIONING_REPORT`).
    Raw,
}

/// Classify a raw pool key without allocating.
fn classify_key(key: &str) -> KeyClass<'_> {
    let mut segments = key.split(EVENT_KEY_DELIMITER);

    // `str::split` always yields at least one element.
    let prefix = segments.next().unwrap_or_default();
    let (Some(vm_id), Some(level), Some(name), Some(event_id)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return KeyClass::Raw;
    };
    if segments.next().is_some() {
        // More than five segments: a `|` leaked into a field.
        return KeyClass::Raw;
    }

    match level.parse::<Level>() {
        Ok(level) => KeyClass::Event {
            prefix,
            vm_id,
            level,
            name,
            event_id,
        },
        Err(_) => KeyClass::Malformed {
            reason: format!("unrecognized level {level:?}"),
        },
    }
}

/// Split `value` into pieces of at most `max_bytes` bytes each, always
/// at UTF-8 codepoint boundaries.
///
/// An empty input yields a single empty chunk so callers still write one
/// record. A codepoint wider than `max_bytes` (only possible for tiny
/// `max_bytes`, never for [`MAX_CHUNK_BYTES`]) is emitted whole so the
/// split always makes progress.
fn chunk_at_char_boundary(value: &str, max_bytes: usize) -> Vec<&str> {
    debug_assert!(max_bytes > 0, "max_bytes must be positive");
    if value.is_empty() {
        return vec![""];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < value.len() {
        if value.len() - start <= max_bytes {
            chunks.push(&value[start..]);
            break;
        }

        // Walk back from the byte limit to the nearest codepoint boundary.
        let mut end = start + max_bytes;
        while end > start && !value.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            // One codepoint spans the whole window; take it whole.
            end = start + max_bytes + 1;
            while end < value.len() && !value.is_char_boundary(end) {
                end += 1;
            }
        }

        chunks.push(&value[start..end]);
        start = end;
    }
    chunks
}

/// Reject the `|` key delimiter in an event field so the formatted key
/// round-trips through [`classify_key`].
fn reject_delimiter(field: &'static str, value: &str) -> Result<(), KvpError> {
    if value.contains(EVENT_KEY_DELIMITER) {
        return Err(KvpError::EventFieldContainsDelimiter { field });
    }
    Ok(())
}

/// A single diagnostic event.
///
/// Construct one with [`new`](Self::new) (which generates a fresh
/// `event_id`) and hand it to [`DiagnosticsKvp::emit`]; events read back
/// via [`DiagnosticsKvp::records`] carry the same fields, decoded from
/// the pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticEvent {
    /// Severity of the event.
    pub level: Level,
    /// Formatted event name, e.g. `user:create_user`.
    pub name: String,
    /// Per-emit identifier. [`new`](Self::new) generates a UUIDv4;
    /// every chunk of one emitted event shares this value.
    pub event_id: String,
    /// Literal value bytes written to the pool. The diagnostics layer
    /// imposes no format on this string.
    pub message: String,
}

impl DiagnosticEvent {
    /// Create an event with a freshly generated `event_id` (UUIDv4).
    pub fn new(
        level: Level,
        name: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            level,
            name: name.into(),
            event_id: Uuid::new_v4().to_string(),
            message: message.into(),
        }
    }
}

/// A single record read back from the pool and classified by
/// [`DiagnosticsKvp::records`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticRecord {
    /// A reassembled diagnostic event.
    Event {
        /// The decoded event.
        event: DiagnosticEvent,
        /// Number of on-disk records the value spanned (1 when short).
        chunks: usize,
    },
    /// An unstructured record whose key is not an event key, such as
    /// `PROVISIONING_REPORT`.
    Raw {
        /// The record key.
        key: String,
        /// The reassembled record value.
        value: String,
    },
    /// A record whose key has five segments but is not a valid event
    /// (for example, an unrecognized level).
    Malformed {
        /// The record key.
        key: String,
        /// The reassembled record value.
        value: String,
        /// Why the key failed to parse as an event.
        reason: String,
    },
}

/// A typed diagnostics view over a [`KvpPoolStore`].
///
/// Owns the event-key `prefix` and `vm_id` used to format and
/// [`scope`](Self::clear) this layer's events. See the
/// [module documentation](self) for the on-disk format.
#[derive(Clone, Debug)]
pub struct DiagnosticsKvp {
    store: KvpPoolStore,
    vm_id: String,
    event_prefix: String,
}

impl DiagnosticsKvp {
    /// Wrap `store` with the `vm_id` and `event_prefix` stamped into
    /// this layer's event keys.
    pub fn new(
        store: KvpPoolStore,
        vm_id: impl Into<String>,
        event_prefix: impl Into<String>,
    ) -> Self {
        Self {
            store,
            vm_id: vm_id.into(),
            event_prefix: event_prefix.into(),
        }
    }

    /// The underlying store.
    pub fn store(&self) -> &KvpPoolStore {
        &self.store
    }

    /// The VM identifier stamped into event keys.
    pub fn vm_id(&self) -> &str {
        &self.vm_id
    }

    /// The prefix stamped into event keys.
    pub fn event_prefix(&self) -> &str {
        &self.event_prefix
    }

    /// Write `event`.
    ///
    /// Messages longer than the store's per-record value limit are split
    /// at UTF-8 codepoint boundaries and written as multiple records
    /// atomically under a single lock via
    /// [`KvpPoolStore::append_multiple`]. Each chunk is keyed by the event
    /// key with a `|<subevent_index>` suffix so every record is unique —
    /// the Hyper-V host keeps only one record per key — and the chunks are
    /// regrouped by [`records`](Self::records) on read.
    ///
    /// Returns [`KvpError::EventFieldContainsDelimiter`] if the
    /// `event_prefix`, `vm_id`, `name`, or `event_id` contains the `|`
    /// key delimiter, which would make the key ambiguous to
    /// [`records`](Self::records).
    pub fn emit(&self, event: &DiagnosticEvent) -> Result<(), KvpError> {
        reject_delimiter("event_prefix", &self.event_prefix)?;
        reject_delimiter("vm_id", &self.vm_id)?;
        reject_delimiter("name", &event.name)?;
        reject_delimiter("event_id", &event.event_id)?;

        let key = format_event_key(
            &self.event_prefix,
            &self.vm_id,
            event.level,
            &event.name,
            &event.event_id,
        );

        self.write_chunked(&key, &event.message)
    }

    /// Split `value` at the store's per-record limit and append the
    /// chunks under `key` in one atomic batch.
    ///
    /// A single-record value keeps the bare event `key`. A value that
    /// spans multiple records gets one record per chunk, each keyed
    /// `<key>|<subevent_index>` (`0`, `1`, …) so no two records collide —
    /// the Hyper-V host keeps only one record per key. [`reassemble`]
    /// strips the subevent index to regroup the chunks on read.
    fn write_chunked(&self, key: &str, value: &str) -> Result<(), KvpError> {
        let chunks = chunk_at_char_boundary(value, self.store.max_value_size());
        if chunks.len() == 1 {
            return self
                .store
                .append_multiple(chunks.into_iter().map(|chunk| (key, chunk)));
        }
        let records: Vec<(String, &str)> = chunks
            .into_iter()
            .enumerate()
            .map(|(subevent_index, chunk)| {
                let chunk_key =
                    format!("{key}{EVENT_KEY_DELIMITER}{subevent_index}");
                (chunk_key, chunk)
            })
            .collect();
        self.store.append_multiple(records)
    }

    /// Read every record, reassembling chunked events and classifying
    /// each into a [`DiagnosticRecord`].
    ///
    /// Records are returned in on-disk order. Consecutive records that
    /// share an event key — ignoring the `|<subevent_index>` suffix — are
    /// one event; because [`emit`](Self::emit) writes an event's chunks
    /// contiguously under a single lock, reassembly is correct even under
    /// concurrent writers.
    pub fn records(&self) -> Result<Vec<DiagnosticRecord>, KvpError> {
        Ok(reassemble(self.store.dump()?))
    }

    /// Read back only the records that decode as diagnostic events, in
    /// on-disk order.
    pub fn events(&self) -> Result<Vec<DiagnosticEvent>, KvpError> {
        Ok(self
            .records()?
            .into_iter()
            .filter_map(|record| match record {
                DiagnosticRecord::Event { event, .. } => Some(event),
                DiagnosticRecord::Raw { .. }
                | DiagnosticRecord::Malformed { .. } => None,
            })
            .collect())
    }

    /// Remove this layer's events — keys that parse as an event key
    /// (including every `|<subevent_index>` chunk of a multi-record
    /// event) whose `prefix` and `vm_id` match this instance — under a
    /// single lock. Raw records such as `PROVISIONING_REPORT` are left
    /// intact.
    pub fn clear(&self) -> Result<(), KvpError> {
        let keys: Vec<String> = self
            .store
            .dump()?
            .into_iter()
            .filter_map(|(key, _)| {
                let is_mine = matches!(
                    classify_key(base_event_key(&key)),
                    KeyClass::Event { prefix, vm_id, .. }
                        if prefix == self.event_prefix
                            && vm_id == self.vm_id
                );
                is_mine.then_some(key)
            })
            .collect();
        self.store.delete_multiple(keys)?;
        Ok(())
    }
}

/// The event key a chunk belongs to.
///
/// [`DiagnosticsKvp::write_chunked`] gives each chunk of a multi-record
/// event a unique key by appending a `|<subevent_index>` (cloud-init's
/// term) to the event key, so the Hyper-V host — which keeps only one
/// record per key — retains every chunk. This returns the shared event
/// key used to regroup them on read: for a chunk key
/// `<event-key>|<subevent_index>` it strips the trailing index; any other
/// key (a single-record event, `PROVISIONING_REPORT`, a malformed key, …)
/// is returned unchanged.
fn base_event_key(key: &str) -> &str {
    if let Some((base, subevent_index)) = key.rsplit_once(EVENT_KEY_DELIMITER) {
        if subevent_index.parse::<u32>().is_ok()
            && matches!(classify_key(base), KeyClass::Event { .. })
        {
            return base;
        }
    }
    key
}

/// Group consecutive records sharing an event key — chunk
/// `|<subevent_index>` suffixes stripped — from [`KvpPoolStore::dump`]
/// and classify each group into a [`DiagnosticRecord`].
fn reassemble(dumped: Vec<(String, String)>) -> Vec<DiagnosticRecord> {
    let mut records = Vec::new();
    let mut dumped = dumped.into_iter().peekable();

    while let Some((key, value)) = dumped.next() {
        let base = base_event_key(&key).to_string();
        let mut message = value;
        let mut chunks = 1;
        while dumped
            .peek()
            .is_some_and(|(next, _)| base_event_key(next) == base)
        {
            let (_, next_value) = dumped.next().expect("peeked value exists");
            message.push_str(&next_value);
            chunks += 1;
        }
        records.push(classify_record(base, message, chunks));
    }

    records
}

/// Turn one reassembled key/value group into a [`DiagnosticRecord`].
fn classify_record(
    key: String,
    value: String,
    chunks: usize,
) -> DiagnosticRecord {
    match classify_key(&key) {
        KeyClass::Event {
            level,
            name,
            event_id,
            ..
        } => DiagnosticRecord::Event {
            event: DiagnosticEvent {
                level,
                name: name.to_string(),
                event_id: event_id.to_string(),
                message: value,
            },
            chunks,
        },
        KeyClass::Malformed { reason } => {
            DiagnosticRecord::Malformed { key, value, reason }
        }
        KeyClass::Raw => DiagnosticRecord::Raw { key, value },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const PREFIX: &str = "azure-init-0.1.0";
    const VM_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    const EVENT_ID: &str = "8f3e9c4a-1b2c-4d5e-9f01-234567890abc";

    #[test]
    fn event_key_formats_and_classifies() {
        let formatted = format_event_key(
            PREFIX,
            VM_ID,
            Level::INFO,
            "user:create_user",
            EVENT_ID,
        );
        assert_eq!(
            formatted,
            format!("{PREFIX}|{VM_ID}|INFO|user:create_user|{EVENT_ID}")
        );
        assert!(matches!(
            classify_key(&formatted),
            KeyClass::Event {
                prefix,
                vm_id,
                level,
                name,
                event_id,
            } if prefix == PREFIX
                && vm_id == VM_ID
                && level == Level::INFO
                && name == "user:create_user"
                && event_id == EVENT_ID
        ));
    }

    #[test]
    fn classify_round_trips_every_level() {
        for expected in [
            Level::ERROR,
            Level::WARN,
            Level::INFO,
            Level::DEBUG,
            Level::TRACE,
        ] {
            let key = format_event_key(
                PREFIX,
                VM_ID,
                expected,
                "span:event",
                EVENT_ID,
            );
            assert!(matches!(
                classify_key(&key),
                KeyClass::Event { level, .. } if level == expected
            ));
        }
    }

    /// Map a key to its [`KeyClass`] discriminant for table-driven tests.
    fn class_of(key: &str) -> &'static str {
        match classify_key(key) {
            KeyClass::Event { .. } => "event",
            KeyClass::Malformed { .. } => "malformed",
            KeyClass::Raw => "raw",
        }
    }

    #[rstest]
    #[case::event("p|vm|INFO|name|id", "event")]
    #[case::raw_single_segment("PROVISIONING_REPORT", "raw")]
    #[case::raw_too_few_segments("a|b|INFO|c", "raw")]
    #[case::raw_too_many_segments("a|b|INFO|c|d|e", "raw")]
    #[case::malformed_bad_level("p|vm|NOTALEVEL|name|id", "malformed")]
    #[case::malformed_other_level("p|vm|NOPE|name|id", "malformed")]
    fn classify_key_categorizes(#[case] key: &str, #[case] expected: &str) {
        assert_eq!(class_of(key), expected);
    }

    #[rstest]
    #[case::empty("", 4, vec![""])]
    #[case::shorter_than_max("abc", 8, vec!["abc"])]
    #[case::exact_multiple("abcdef", 2, vec!["ab", "cd", "ef"])]
    #[case::ascii_remainder("abcde", 2, vec!["ab", "cd", "e"])]
    #[case::two_byte_boundary("aéb", 2, vec!["a", "é", "b"])]
    #[case::oversized_three_byte("€", 1, vec!["€"])]
    #[case::oversized_repeated("€€", 1, vec!["€", "€"])]
    fn chunk_splits_at_utf8_boundaries(
        #[case] input: &str,
        #[case] max_bytes: usize,
        #[case] expected: Vec<&str>,
    ) {
        assert_eq!(chunk_at_char_boundary(input, max_bytes), expected);
    }

    #[test]
    fn chunk_reassembles_multibyte_payload() {
        let payload = "🚀".repeat(100);
        let chunks = chunk_at_char_boundary(&payload, 7);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 7));
        assert_eq!(chunks.concat(), payload);
    }

    #[test]
    fn diagnostic_event_new_generates_uuid_event_id() {
        let event = DiagnosticEvent::new(Level::INFO, "span:name", "message");
        assert!(Uuid::parse_str(&event.event_id).is_ok());
    }

    #[test]
    fn reject_delimiter_flags_pipe() {
        assert!(reject_delimiter("name", "no pipe here").is_ok());
        let err = reject_delimiter("name", "has|pipe").unwrap_err();
        assert!(matches!(
            err,
            KvpError::EventFieldContainsDelimiter { field: "name" }
        ));
        assert_eq!(
            err.to_string(),
            "event key field 'name' must not contain '|'"
        );
    }

    #[test]
    fn reassemble_groups_chunks_and_classifies() {
        let key = format_event_key(
            PREFIX,
            VM_ID,
            Level::INFO,
            "config:dump",
            EVENT_ID,
        );

        let dumped = vec![
            (key.clone(), "part-one/".to_string()),
            (key.clone(), "part-two".to_string()),
            (
                "PROVISIONING_REPORT".to_string(),
                "result=success".to_string(),
            ),
            ("p|vm|NOPE|name|id".to_string(), "junk".to_string()),
        ];

        let records = reassemble(dumped);
        assert_eq!(records.len(), 3);

        assert_eq!(
            records[0],
            DiagnosticRecord::Event {
                event: DiagnosticEvent {
                    level: Level::INFO,
                    name: "config:dump".to_string(),
                    event_id: EVENT_ID.to_string(),
                    message: "part-one/part-two".to_string(),
                },
                chunks: 2,
            }
        );
        assert!(matches!(&records[1], DiagnosticRecord::Raw { key, .. }
            if key == "PROVISIONING_REPORT"));
        assert!(matches!(&records[2], DiagnosticRecord::Malformed { .. }));
    }

    #[test]
    fn reassemble_keeps_distinct_adjacent_keys_separate() {
        let make = |event_id: &str| {
            format_event_key(PREFIX, VM_ID, Level::INFO, "span:name", event_id)
        };
        let dumped = vec![
            (make("id-1"), "first".to_string()),
            (make("id-2"), "second".to_string()),
        ];
        let records = reassemble(dumped);
        assert_eq!(records.len(), 2);
        assert!(matches!(
            &records[0],
            DiagnosticRecord::Event { chunks: 1, .. }
        ));
        assert!(matches!(
            &records[1],
            DiagnosticRecord::Event { chunks: 1, .. }
        ));
    }

    #[rstest]
    #[case::indexed_chunk("p|vm|INFO|name|id|0", "p|vm|INFO|name|id")]
    #[case::indexed_chunk_multi_digit(
        "p|vm|INFO|name|id|12",
        "p|vm|INFO|name|id"
    )]
    #[case::single_event_unchanged("p|vm|INFO|name|id", "p|vm|INFO|name|id")]
    #[case::raw_unchanged("PROVISIONING_REPORT", "PROVISIONING_REPORT")]
    #[case::non_event_numeric_tail_unchanged("foo|3", "foo|3")]
    #[case::malformed_unchanged("p|vm|NOPE|name|id", "p|vm|NOPE|name|id")]
    fn base_event_key_strips_event_subevent_index(
        #[case] key: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(base_event_key(key), expected);
    }

    #[test]
    fn reassemble_groups_indexed_chunk_keys() {
        let base = format_event_key(
            PREFIX,
            VM_ID,
            Level::INFO,
            "config:dump",
            EVENT_ID,
        );
        let dumped = vec![
            (format!("{base}|0"), "part-one/".to_string()),
            (format!("{base}|1"), "part-two/".to_string()),
            (format!("{base}|2"), "part-three".to_string()),
        ];

        let records = reassemble(dumped);
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0],
            DiagnosticRecord::Event {
                event: DiagnosticEvent {
                    level: Level::INFO,
                    name: "config:dump".to_string(),
                    event_id: EVENT_ID.to_string(),
                    message: "part-one/part-two/part-three".to_string(),
                },
                chunks: 3,
            }
        );
    }
}
