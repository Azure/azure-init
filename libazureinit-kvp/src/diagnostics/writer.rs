// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use uuid::Uuid;

use super::diagnostic::{
    Diagnostic, DiagnosticEvent, DiagnosticFinish, DiagnosticKey,
    DiagnosticPayload, DiagnosticStart, Encoding, Outcome,
    DIAGNOSTIC_VERSION_ID,
};
use super::encoding::encode_payload;
use super::MAX_CHUNK_BYTES;
use crate::{KvpError, KvpPoolStore};

const MAX_AGENT_BYTES: usize = 32;
const DEFAULT_MAX_NAME_BYTES: usize = 64;
const MAX_UUID_BYTES: usize = 36;
const MAX_TIMESTAMP_BYTES: usize = 30;
const MAX_KEY_BYTES: usize = 254;
const MAX_CHUNKS: usize = 1023;

/// Fractional-second precision for emitted diagnostic timestamps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimestampPrecision {
    /// Whole seconds, no fractional digits.
    Seconds,
    /// Millisecond precision (three fractional digits). The default.
    #[default]
    Millis,
    /// Microsecond precision (six fractional digits).
    Micros,
    /// Nanosecond precision (nine fractional digits).
    Nanos,
}

impl TimestampPrecision {
    fn seconds_format(self) -> SecondsFormat {
        match self {
            Self::Seconds => SecondsFormat::Secs,
            Self::Millis => SecondsFormat::Millis,
            Self::Micros => SecondsFormat::Micros,
            Self::Nanos => SecondsFormat::Nanos,
        }
    }
}

/// Fractional-second precision for emitted durations.
///
/// Digits below the selected precision are discarded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DurationPrecision {
    /// Whole seconds, no fractional digits.
    Seconds,
    /// Milliseconds (three fractional digits).
    Millis,
    /// Microseconds (six fractional digits). The default.
    #[default]
    Micros,
    /// Nanoseconds (nine fractional digits).
    Nanos,
}

impl DurationPrecision {
    pub(crate) fn format(self, duration: Duration) -> String {
        let digits = match self {
            Self::Seconds => return duration.as_secs().to_string(),
            Self::Millis => 3,
            Self::Micros => 6,
            Self::Nanos => 9,
        };
        let fraction = duration.subsec_nanos() / 10u32.pow(9 - digits);
        let width = digits as usize;
        format!("{}.{fraction:0width$}", duration.as_secs())
    }
}

/// Emits diagnostics for one reporting agent and VM.
///
/// Use [`emit_event`](Self::emit_event) for a standalone observation. To record
/// an operation, call [`emit_start`](Self::emit_start) and
/// [`emit_finish`](Self::emit_finish) with the same event UUID.
///
/// Timestamps and event UUIDs for standalone observations are generated
/// automatically. The caller supplies operation IDs, outcomes and durations.
/// Pass `None` for plain text or an [`Encoding`] for compressed text or bytes.
///
/// Emission appends to the pool and splits large payloads automatically; it
/// never clears existing records. Invalid input writes nothing, while an I/O
/// error may leave part of a payload in the pool. See the
/// [diagnostics contract] for format details and limits.
///
/// [diagnostics contract]: https://github.com/Azure/azure-init/blob/main/doc/diagnostics.md
///
/// # Example
/// ```no_run
/// use std::time::Instant;
/// use libazureinit_kvp::{
///     DiagnosticWriter, KvpPool, KvpPoolStore, Outcome, PoolMode,
/// };
/// use uuid::Uuid;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let store = KvpPoolStore::new(KvpPool::Guest, PoolMode::Safe)?;
/// let writer = DiagnosticWriter::new(
///     store, "azure-init/0.1.1", "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
/// )?;
/// let event_id = Uuid::new_v4().to_string();
/// writer.emit_start(&event_id, "read:os-release", "reading OS information", None)?;
/// let started = Instant::now();
/// let contents = std::fs::read_to_string("/etc/os-release")?;
/// writer.emit_finish(
///     &event_id, "read:os-release", format!("read {} bytes", contents.len()),
///     None, Outcome::Success, started.elapsed(),
/// )?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct DiagnosticWriter {
    store: KvpPoolStore,
    agent: String,
    vm_id: String,
    max_name_bytes: usize,
    timestamp_precision: TimestampPrecision,
    duration_precision: DurationPrecision,
}

impl DiagnosticWriter {
    /// Creates a writer with a fixed reporting agent and VM UUID.
    ///
    /// Versioned agents conventionally use `name/VERSION`. Invalid agent or
    /// VM identifiers return [`KvpError`]; construction does not access the pool.
    /// Timestamp precision defaults to milliseconds and duration precision to
    /// microseconds.
    pub fn new(
        store: KvpPoolStore,
        agent: impl Into<String>,
        vm_id: impl Into<String>,
    ) -> Result<Self, KvpError> {
        let agent = agent.into();
        let vm_id = vm_id.into();
        validate_field("agent", &agent, MAX_AGENT_BYTES)?;
        validate_uuid("vm_id", &vm_id)?;
        Ok(Self {
            store,
            agent,
            vm_id,
            max_name_bytes: DEFAULT_MAX_NAME_BYTES,
            timestamp_precision: TimestampPrecision::default(),
            duration_precision: DurationPrecision::default(),
        })
    }

    /// Sets the maximum name length in UTF-8 bytes (default: 64).
    ///
    /// Names over the limit are rejected, never truncated. The complete key
    /// must still fit within 254 bytes, including its chunk index.
    pub fn with_max_name_bytes(mut self, max: usize) -> Self {
        self.max_name_bytes = max;
        self
    }

    /// Overrides the default millisecond precision for emitted timestamps.
    pub fn with_timestamp_precision(
        mut self,
        precision: TimestampPrecision,
    ) -> Self {
        self.timestamp_precision = precision;
        self
    }

    /// Sets duration precision independently of timestamp precision.
    pub fn with_duration_precision(
        mut self,
        precision: DurationPrecision,
    ) -> Self {
        self.duration_precision = precision;
        self
    }

    /// Records the beginning of an operation.
    ///
    /// Pass the same event UUID and name to
    /// [`emit_finish`](Self::emit_finish).
    pub fn emit_start(
        &self,
        event_id: &str,
        name: &str,
        payload: impl Into<DiagnosticPayload>,
        encoding: Option<Encoding>,
    ) -> Result<(), KvpError> {
        self.emit(Diagnostic::Start(DiagnosticStart {
            key: self.key(event_id, name, encoding),
            payload: payload.into(),
        }))
    }

    /// Records an operation's outcome and caller-measured elapsed time.
    ///
    /// Use the event UUID and name passed to [`emit_start`](Self::emit_start).
    /// The writer does not verify that a start exists or calculate the duration.
    pub fn emit_finish(
        &self,
        event_id: &str,
        name: &str,
        payload: impl Into<DiagnosticPayload>,
        encoding: Option<Encoding>,
        result: Outcome,
        duration: Duration,
    ) -> Result<(), KvpError> {
        self.emit(Diagnostic::Finish(DiagnosticFinish {
            key: self.key(event_id, name, encoding),
            payload: payload.into(),
            result,
            duration,
        }))
    }

    /// Records a standalone observation with a generated event UUID.
    ///
    /// `result` and `duration` may be supplied independently.
    pub fn emit_event(
        &self,
        name: &str,
        payload: impl Into<DiagnosticPayload>,
        encoding: Option<Encoding>,
        result: Option<Outcome>,
        duration: Option<Duration>,
    ) -> Result<(), KvpError> {
        self.emit(Diagnostic::Event(DiagnosticEvent {
            key: self.key(&Uuid::new_v4().to_string(), name, encoding),
            payload: payload.into(),
            result,
            duration,
        }))
    }

    fn key(
        &self,
        event_id: &str,
        name: &str,
        encoding: Option<Encoding>,
    ) -> DiagnosticKey {
        DiagnosticKey {
            agent: self.agent.clone(),
            vm_id: Some(self.vm_id.clone()),
            name: name.to_owned(),
            event_id: event_id.to_owned(),
            timestamp: Utc::now(),
            encoding,
        }
    }

    fn emit(&self, diagnostic: Diagnostic) -> Result<(), KvpError> {
        self.store.append_multiple(prepare_records(
            diagnostic,
            self.timestamp_precision,
            self.duration_precision,
            self.max_name_bytes,
        )?)
    }
}

fn prepare_records(
    diagnostic: Diagnostic,
    timestamp_precision: TimestampPrecision,
    duration_precision: DurationPrecision,
    max_name_bytes: usize,
) -> Result<Vec<(String, String)>, KvpError> {
    let kind = diagnostic.kind();
    let (key, payload, result, duration) = match diagnostic {
        Diagnostic::Start(start) => (start.key, start.payload, None, None),
        Diagnostic::Finish(finish) => (
            finish.key,
            finish.payload,
            Some(finish.result),
            Some(finish.duration),
        ),
        Diagnostic::Event(event) => {
            (event.key, event.payload, event.result, event.duration)
        }
    };

    validate_field("agent", &key.agent, MAX_AGENT_BYTES)?;
    let vm_id = key
        .vm_id
        .as_deref()
        .ok_or(KvpError::EmptyEventField { field: "vm_id" })?;
    validate_uuid("vm_id", vm_id)?;
    validate_field("name", &key.name, max_name_bytes)?;
    validate_uuid("event_id", &key.event_id)?;

    let duration = duration.map_or_else(String::new, |duration| {
        duration_precision.format(duration)
    });
    let timestamp = key
        .timestamp
        .to_rfc3339_opts(timestamp_precision.seconds_format(), true);
    validate_field("timestamp", &timestamp, MAX_TIMESTAMP_BYTES)?;

    let value = encode_payload(payload, key.encoding.as_ref())?;
    let encoding = key
        .encoding
        .as_ref()
        .map_or_else(|| "none".to_owned(), ToString::to_string);
    let result = result.map_or_else(String::new, |result| result.to_string());
    let base_key = format!(
        "{DIAGNOSTIC_VERSION_ID}|{}|{vm_id}|{kind}|{}|{}|{timestamp}|{encoding}|{result}|{duration}",
        key.agent, key.name, key.event_id,
    );
    frame_records(&base_key, &value)
}

fn validate_field(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), KvpError> {
    if value.is_empty() {
        return Err(KvpError::EmptyEventField { field });
    }
    if value.contains('|') {
        return Err(KvpError::EventFieldContainsDelimiter { field });
    }
    if value.contains('\0') {
        return Err(KvpError::KeyContainsNull);
    }
    if value.len() > max {
        return Err(KvpError::EventFieldTooLong {
            field,
            max,
            actual: value.len(),
        });
    }
    Ok(())
}

fn validate_uuid(field: &'static str, value: &str) -> Result<(), KvpError> {
    validate_field(field, value, MAX_UUID_BYTES)?;
    Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| KvpError::InvalidUuid { field })
}

fn frame_records(
    base_key: &str,
    mut value: &str,
) -> Result<Vec<(String, String)>, KvpError> {
    let mut records = Vec::new();
    loop {
        if records.len() == MAX_CHUNKS {
            return Err(KvpError::TooManyChunks { max: MAX_CHUNKS });
        }
        let key = format!("{base_key}|{}", records.len());
        if key.len() > MAX_KEY_BYTES {
            return Err(KvpError::KeyTooLarge {
                max: MAX_KEY_BYTES,
                actual: key.len(),
            });
        }

        let mut end = value.len().min(MAX_CHUNK_BYTES);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        let (chunk, remaining) = value.split_at(end);
        records.push((key, chunk.to_owned()));
        if remaining.is_empty() {
            return Ok(records);
        }
        value = remaining;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use chrono::DateTime;
    use rstest::rstest;
    use tempfile::TempDir;

    use super::super::encoding::decode_payload;
    use crate::store::{Handle, OsSysOps, StatInfo, SysOps};
    use crate::{DiagnosticReader, Entry, KvpPool, PoolMode};

    const AGENT: &str = "azure-init/0.1.1";
    const VM_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    const EVENT_ID: &str = "8f3e9c4a-1b2c-4d5e-9f01-234567890abc";
    const TIMESTAMP: &str = "2026-08-31T12:34:56.789Z";

    fn store(dir: &TempDir, mode: PoolMode) -> KvpPoolStore {
        KvpPoolStore::new_in(KvpPool::Guest, dir.path(), mode).unwrap()
    }

    fn key() -> DiagnosticKey {
        DiagnosticKey {
            agent: AGENT.into(),
            vm_id: Some(VM_ID.into()),
            name: "provision:run".into(),
            event_id: EVENT_ID.into(),
            timestamp: DateTime::parse_from_rfc3339(TIMESTAMP)
                .unwrap()
                .with_timezone(&Utc),
            encoding: None,
        }
    }

    fn event(key: DiagnosticKey, payload: DiagnosticPayload) -> Diagnostic {
        Diagnostic::Event(DiagnosticEvent {
            key,
            payload,
            result: None,
            duration: None,
        })
    }

    #[derive(Debug, Default)]
    struct WriterOps {
        os: OsSysOps,
        calls: AtomicUsize,
    }

    impl SysOps for WriterOps {
        fn open_read(&self, _: &Path) -> io::Result<Box<dyn Handle>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(io::ErrorKind::Unsupported.into())
        }

        fn open_read_write(&self, _: &Path) -> io::Result<Box<dyn Handle>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(io::ErrorKind::Unsupported.into())
        }

        fn open_read_write_create(
            &self,
            path: &Path,
        ) -> io::Result<Box<dyn Handle>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.os.open_read_write_create(path)
        }

        fn path_metadata(&self, _: &Path) -> io::Result<StatInfo> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(io::ErrorKind::Unsupported.into())
        }

        fn boot_time(&self) -> io::Result<i64> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    fn observed_writer(dir: &TempDir) -> (DiagnosticWriter, Arc<WriterOps>) {
        let ops = Arc::new(WriterOps::default());
        let store = KvpPoolStore::with_ops(
            KvpPool::Guest,
            dir.path(),
            PoolMode::Safe,
            ops.clone(),
        )
        .unwrap();
        (DiagnosticWriter::new(store, AGENT, VM_ID).unwrap(), ops)
    }

    #[test]
    fn writer_ops_rejects_non_append_operations() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let ops = WriterOps::default();
        assert_eq!(
            [
                ops.open_read(pool.path()).unwrap_err().kind(),
                ops.open_read_write(pool.path()).unwrap_err().kind(),
                ops.path_metadata(pool.path()).unwrap_err().kind(),
                ops.boot_time().unwrap_err().kind(),
            ],
            [io::ErrorKind::Unsupported; 4]
        );
        assert_eq!(ops.calls.load(Ordering::SeqCst), 4);
        assert!(!pool.path().exists());
    }

    fn assert_rejected_without_writes(
        operation: impl FnOnce(&DiagnosticWriter) -> Result<(), KvpError>,
    ) -> KvpError {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        pool.append("existing", "value").unwrap();
        let before = fs::read(pool.path()).unwrap();
        let (writer, ops) = observed_writer(&dir);
        let error = operation(&writer).unwrap_err();
        assert_eq!(ops.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read(pool.path()).unwrap(), before);
        error
    }

    #[test]
    fn constructor_does_no_io_and_emission_opens_once() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let (writer, ops) = observed_writer(&dir);
        assert_eq!(ops.calls.load(Ordering::SeqCst), 0);
        assert!(!pool.path().exists());

        let payload = "x".repeat(MAX_CHUNK_BYTES * 3 + 1);
        writer
            .emit_event("test", payload, None, None, None)
            .unwrap();
        assert_eq!(ops.calls.load(Ordering::SeqCst), 1);
        assert_eq!(pool.dump().unwrap().len(), 4);
    }

    #[test]
    fn constructor_rejects_empty_agent_without_io() {
        let dir = TempDir::new().unwrap();
        let (writer, ops) = observed_writer(&dir);
        assert!(matches!(
            DiagnosticWriter::new(writer.store, "", VM_ID),
            Err(KvpError::EmptyEventField { field: "agent" })
        ));
        assert_eq!(ops.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn constructor_rejects_invalid_vm_id_without_io() {
        let dir = TempDir::new().unwrap();
        let (writer, ops) = observed_writer(&dir);
        assert!(matches!(
            DiagnosticWriter::new(writer.store, AGENT, "vm-abc"),
            Err(KvpError::InvalidUuid { field: "vm_id" })
        ));
        assert_eq!(ops.calls.load(Ordering::SeqCst), 0);
    }

    #[rstest]
    #[case("invalid")]
    #[case("3f2504e0-4f89-41d3-9a0c-0305e82c330z")]
    fn uuid_validation_rejects_invalid_syntax(#[case] value: &str) {
        assert!(matches!(
            validate_uuid("event_id", value),
            Err(KvpError::InvalidUuid { field: "event_id" })
        ));
    }

    #[test]
    fn uuid_validation_preserves_field_errors() {
        assert!(matches!(
            validate_uuid("event_id", ""),
            Err(KvpError::EmptyEventField { field: "event_id" })
        ));
        assert!(matches!(
            validate_uuid("event_id", "bad|id"),
            Err(KvpError::EventFieldContainsDelimiter { field: "event_id" })
        ));
        assert!(matches!(
            validate_uuid("event_id", "bad\0id"),
            Err(KvpError::KeyContainsNull)
        ));
    }

    #[test]
    fn uuid_validation_rejects_oversized_representations() {
        assert!(matches!(
            validate_uuid("vm_id", &format!("{{{VM_ID}}}")),
            Err(KvpError::EventFieldTooLong {
                field: "vm_id",
                max: MAX_UUID_BYTES,
                actual: 38,
            })
        ));
    }

    #[rstest]
    #[case(VM_ID)]
    #[case("3F2504E0-4F89-41D3-9A0C-0305E82C3301")]
    #[case("3f2504e04f8941d39a0c0305e82c3301")]
    #[case("00000000-0000-0000-0000-000000000000")]
    fn valid_uuid_spellings_are_preserved(#[case] id: &str) {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, id).unwrap();
        writer.emit_start(id, "test", "starting", None).unwrap();
        let records = pool.dump().unwrap();
        let fields: Vec<_> = records[0].0.split('|').collect();
        assert_eq!(fields[2], id);
        assert_eq!(fields[5], id);
    }

    #[rstest]
    #[case("agent", "a".repeat(32), 32, None)]
    #[case("agent", "é".repeat(16), 32, None)]
    #[case("name", "n".repeat(64), 64, None)]
    #[case("name", "é".repeat(32), 64, None)]
    #[case("name", "n".repeat(32), 32, Some(32))]
    #[case("name", "n".repeat(96), 96, Some(96))]
    fn freeform_caps_count_bytes_without_truncation(
        #[case] field: &'static str,
        #[case] value: String,
        #[case] max: usize,
        #[case] max_name_bytes: Option<usize>,
        #[values(PoolMode::Safe, PoolMode::Unsafe)] mode: PoolMode,
    ) {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, mode);
        let (agent, name) = match field {
            "agent" => (value.as_str(), "test"),
            _ => (AGENT, value.as_str()),
        };
        let writer = DiagnosticWriter::new(pool.clone(), agent, VM_ID).unwrap();
        let writer = match max_name_bytes {
            Some(max) => writer.with_max_name_bytes(max),
            None => writer,
        };
        writer.emit_event(name, "ok", None, None, None).unwrap();
        let records = pool.dump().unwrap();
        let fields: Vec<_> = records[0].0.split('|').collect();
        assert_eq!(fields[1], agent);
        assert_eq!(fields[4], name);

        let before = fs::read(pool.path()).unwrap();
        let oversized = format!("{value}x");
        let error = match field {
            "agent" => DiagnosticWriter::new(pool.clone(), oversized, VM_ID)
                .unwrap_err(),
            _ => writer
                .emit_event(&oversized, "bad", None, None, None)
                .unwrap_err(),
        };
        assert!(matches!(
            error,
            KvpError::EventFieldTooLong { field: actual_field, max: cap, actual }
                if actual_field == field && cap == max && actual == max + 1
        ));
        assert_eq!(fs::read(pool.path()).unwrap(), before);
    }

    #[rstest]
    #[case("", "empty")]
    #[case("bad|field", "delimiter")]
    #[case("bad\0field", "null")]
    fn field_validation_reports_the_reason(
        #[case] value: &str,
        #[case] reason: &str,
    ) {
        let error =
            validate_field("name", value, DEFAULT_MAX_NAME_BYTES).unwrap_err();
        match reason {
            "empty" => {
                assert!(matches!(
                    error,
                    KvpError::EmptyEventField { field: "name" }
                ))
            }
            "delimiter" => assert!(matches!(
                error,
                KvpError::EventFieldContainsDelimiter { field: "name" }
            )),
            _ => assert!(matches!(error, KvpError::KeyContainsNull)),
        }
    }

    #[test]
    fn exact_start_key_matches_diag_layout() {
        let records = prepare_records(
            Diagnostic::Start(DiagnosticStart {
                key: key(),
                payload: "starting".into(),
            }),
            TimestampPrecision::Millis,
            DurationPrecision::default(),
            DEFAULT_MAX_NAME_BYTES,
        )
        .unwrap();
        assert_eq!(
            records,
            vec![(
                format!(
                    "DIAG|{AGENT}|{VM_ID}|start|provision:run|{EVENT_ID}|{TIMESTAMP}|none|||0"
                ),
                "starting".to_owned(),
            )]
        );
    }

    #[rstest]
    #[case::success(Outcome::Success, "success", 312_000, "0.312000")]
    #[case::failure(Outcome::Failure, "fail", 0, "0.000000")]
    fn exact_finish_key_matches_diag_layout(
        #[case] result: Outcome,
        #[case] token: &str,
        #[case] duration_us: u64,
        #[case] seconds: &str,
    ) {
        let records = prepare_records(
            Diagnostic::Finish(DiagnosticFinish {
                key: key(),
                payload: "finished".into(),
                result,
                duration: Duration::from_micros(duration_us),
            }),
            TimestampPrecision::Millis,
            DurationPrecision::default(),
            DEFAULT_MAX_NAME_BYTES,
        )
        .unwrap();
        assert_eq!(
            records[0].0,
            format!(
                "DIAG|{AGENT}|{VM_ID}|finish|provision:run|{EVENT_ID}|{TIMESTAMP}|none|{token}|{seconds}|0"
            )
        );
    }

    #[rstest]
    #[case::neither(None, None, "")]
    #[case::result_only(Some(Outcome::Success), None, "")]
    #[case::zero_duration(None, Some(0), "0.000000")]
    #[case::both(Some(Outcome::Failure), Some(52), "0.000052")]
    fn event_optional_fields_are_independent(
        #[case] result: Option<Outcome>,
        #[case] duration_us: Option<u64>,
        #[case] seconds: &str,
    ) {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID).unwrap();
        writer
            .emit_event(
                "test",
                "ok",
                None,
                result,
                duration_us.map(Duration::from_micros),
            )
            .unwrap();
        let records = pool.dump().unwrap();
        let fields: Vec<_> = records[0].0.split('|').collect();
        assert_eq!(fields.len(), 11);
        assert_eq!(fields[3], "event");
        assert_eq!(
            fields[8],
            result.map_or_else(String::new, |v| v.to_string())
        );
        assert_eq!(fields[9], seconds);
    }

    #[test]
    fn span_endpoints_reuse_the_supplied_event_id() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID).unwrap();
        writer
            .emit_start(EVENT_ID, "test", "starting", None)
            .unwrap();
        writer
            .emit_finish(
                EVENT_ID,
                "test",
                "finished",
                None,
                Outcome::Failure,
                Duration::from_micros(17),
            )
            .unwrap();
        let records = pool.dump().unwrap();
        assert_eq!(records.len(), 2);
        for (record, kind) in records.iter().zip(["start", "finish"]) {
            let fields: Vec<_> = record.0.split('|').collect();
            assert_eq!(fields[3], kind);
            assert_eq!(fields[5], EVENT_ID);
        }
    }

    #[test]
    fn events_receive_distinct_v4_ids() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID).unwrap();
        for _ in 0..2 {
            writer
                .emit_event("test", "message", None, None, None)
                .unwrap();
        }
        let ids: Vec<_> = pool
            .dump()
            .unwrap()
            .iter()
            .map(|(key, _)| {
                Uuid::parse_str(key.split('|').nth(5).unwrap()).unwrap()
            })
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        for id in ids {
            assert_eq!(id.get_version_num(), 4);
        }
    }

    #[test]
    fn emission_uses_a_current_utc_millisecond_timestamp() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID).unwrap();
        let before = Utc::now().timestamp_millis();
        writer
            .emit_event("test", "message", None, None, None)
            .unwrap();
        let after = Utc::now().timestamp_millis();
        let records = pool.dump().unwrap();
        assert_eq!(records.len(), 1);
        let timestamp = records[0].0.split('|').nth(6).unwrap();
        assert_eq!(timestamp.len(), 24);
        assert!(timestamp.ends_with('Z'));
        let parsed = DateTime::parse_from_rfc3339(timestamp).unwrap();
        assert!((before..=after).contains(&parsed.timestamp_millis()));
    }

    #[rstest]
    #[case(TimestampPrecision::Seconds, 20)]
    #[case(TimestampPrecision::Millis, 24)]
    #[case(TimestampPrecision::Micros, 27)]
    #[case(TimestampPrecision::Nanos, 30)]
    fn timestamp_precision_controls_emitted_width(
        #[case] precision: TimestampPrecision,
        #[case] expected_len: usize,
    ) {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID)
            .unwrap()
            .with_timestamp_precision(precision);
        writer.emit_event("test", "ok", None, None, None).unwrap();
        let records = pool.dump().unwrap();
        let timestamp = records[0].0.split('|').nth(6).unwrap();
        assert_eq!(timestamp.len(), expected_len);
        assert!(timestamp.ends_with('Z'));
    }

    #[rstest]
    #[case(DurationPrecision::Seconds, "1", Duration::from_secs(1))]
    #[case(DurationPrecision::Millis, "1.123", Duration::from_millis(1123))]
    #[case(
        DurationPrecision::Micros,
        "1.123456",
        Duration::from_micros(1_123_456)
    )]
    #[case(
        DurationPrecision::Nanos,
        "1.123456789",
        Duration::new(1, 123_456_789)
    )]
    fn duration_precision_controls_emitted_seconds(
        #[case] precision: DurationPrecision,
        #[case] seconds: &str,
        #[case] expected: Duration,
    ) {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID)
            .unwrap()
            .with_duration_precision(precision);
        writer
            .emit_event(
                "timed",
                "ok",
                None,
                None,
                Some(Duration::new(1, 123_456_789)),
            )
            .unwrap();
        let records = pool.dump().unwrap();
        let fields: Vec<_> = records[0].0.split('|').collect();
        assert_eq!(fields[9], seconds);
        assert_eq!(fields[6].len(), 24);
        let entries = DiagnosticReader::new(pool).entries().unwrap();
        assert!(
            matches!(&entries[0], Entry::Diagnostic(Diagnostic::Event(event))
            if event.duration == Some(expected))
        );
    }

    #[rstest]
    #[case(String::new(), vec![0])]
    #[case("x".repeat(MAX_CHUNK_BYTES), vec![MAX_CHUNK_BYTES])]
    #[case("x".repeat(MAX_CHUNK_BYTES + 1), vec![MAX_CHUNK_BYTES, 1])]
    #[case(format!("{}é", "x".repeat(1021)), vec![1021, 2])]
    #[case(format!("{}€", "x".repeat(1021)), vec![1021, 3])]
    #[case(format!("{}😀", "x".repeat(1021)), vec![1021, 4])]
    #[case("€".repeat(1000), vec![1020, 1020, 960])]
    fn framing_preserves_utf8_and_empty_values(
        #[case] value: String,
        #[case] lengths: Vec<usize>,
    ) {
        let records = frame_records("base", &value).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|(_, value)| value.len())
                .collect::<Vec<_>>(),
            lengths
        );
        assert_eq!(
            records
                .iter()
                .map(|(_, value)| value.as_str())
                .collect::<String>(),
            value
        );
        for (index, (key, value)) in records.iter().enumerate() {
            assert_eq!(key, &format!("base|{index}"));
            assert!(value.len() <= MAX_CHUNK_BYTES);
        }
    }

    #[rstest]
    fn compression_precedes_safe_framing_in_both_modes(
        #[values(PoolMode::Safe, PoolMode::Unsafe)] mode: PoolMode,
        #[values(Encoding::GzB64, Encoding::ZlibB64)] encoding: Encoding,
    ) {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, mode);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID).unwrap();
        let bytes: Vec<_> = (0..4096u32).flat_map(u32::to_le_bytes).collect();
        writer
            .emit_event(
                "provision:run",
                bytes.clone(),
                Some(encoding.clone()),
                None,
                None,
            )
            .unwrap();
        let records = pool.dump().unwrap();
        assert!(records.len() > 1);
        let base = records[0].0.rsplit_once('|').unwrap().0;
        for (index, (key, value)) in records.iter().enumerate() {
            assert_eq!(key, &format!("{base}|{index}"));
            let fields: Vec<_> = key.split('|').collect();
            assert_eq!(fields[3], "event");
            assert_eq!(fields[7], encoding.to_string());
            assert!(key.len() <= MAX_KEY_BYTES);
            assert!(value.len() <= MAX_CHUNK_BYTES);
        }
        let value: String = records.iter().map(|(_, v)| v.as_str()).collect();
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&encoding)).unwrap(),
            DiagnosticPayload::Bytes(bytes)
        );
    }

    #[test]
    fn maximum_chunk_count_is_accepted() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID).unwrap();
        let payload = "x".repeat(MAX_CHUNK_BYTES * MAX_CHUNKS);
        writer
            .emit_event("test", payload, None, None, None)
            .unwrap();
        let records = pool.dump().unwrap();
        assert_eq!(records.len(), MAX_CHUNKS);
        assert!(records.last().unwrap().0.ends_with("|1022"));
    }

    #[rstest]
    #[case("x".repeat(MAX_CHUNK_BYTES * MAX_CHUNKS + 1))]
    #[case("€".repeat((MAX_CHUNK_BYTES / 3) * MAX_CHUNKS + 1))]
    fn excess_chunks_do_not_open_or_modify_the_pool(#[case] payload: String) {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        pool.append("existing", "value").unwrap();
        let before = fs::read(pool.path()).unwrap();
        let (writer, ops) = observed_writer(&dir);
        assert!(matches!(
            writer.emit_event("test", payload, None, None, None),
            Err(KvpError::TooManyChunks { max: MAX_CHUNKS })
        ));
        assert_eq!(ops.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read(pool.path()).unwrap(), before);
    }

    #[test]
    fn chunk_limit_applies_after_compression() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID).unwrap();
        let bytes = vec![0; MAX_CHUNK_BYTES * MAX_CHUNKS + 1];
        writer
            .emit_event(
                "test",
                bytes.clone(),
                Some(Encoding::GzB64),
                None,
                None,
            )
            .unwrap();
        let records = pool.dump().unwrap();
        assert!(records.len() < MAX_CHUNKS);
        let value: String = records.iter().map(|(_, v)| v.as_str()).collect();
        assert_eq!(
            decode_payload(value.as_bytes(), Some(&Encoding::GzB64)).unwrap(),
            DiagnosticPayload::Bytes(bytes)
        );
    }

    #[test]
    fn key_limit_includes_every_chunk_suffix() {
        let base = "x".repeat(MAX_KEY_BYTES - 2);
        let records = frame_records(&base, "").unwrap();
        assert_eq!(records[0].0.len(), MAX_KEY_BYTES);
        let payload = "v".repeat(MAX_CHUNK_BYTES * 11);
        assert!(matches!(
            frame_records(&base, &payload),
            Err(KvpError::KeyTooLarge {
                max: MAX_KEY_BYTES,
                actual: 255
            })
        ));
    }

    #[test]
    fn default_name_limit_fits_with_four_digit_index() {
        let diagnostic = Diagnostic::Finish(DiagnosticFinish {
            key: DiagnosticKey {
                agent: "a".repeat(MAX_AGENT_BYTES),
                name: "n".repeat(DEFAULT_MAX_NAME_BYTES),
                encoding: Some(Encoding::ZlibB64),
                ..key()
            },
            payload: "test".into(),
            result: Outcome::Success,
            duration: Duration::from_millis(312),
        });
        let records = prepare_records(
            diagnostic,
            TimestampPrecision::Nanos,
            DurationPrecision::Nanos,
            DEFAULT_MAX_NAME_BYTES,
        )
        .unwrap();
        let base = records[0].0.rsplit_once('|').unwrap().0;
        let longest = format!("{base}|1022");
        assert_eq!(longest.len(), 248);
        assert!(longest.len() <= MAX_KEY_BYTES);
    }

    #[test]
    fn name_allowance_does_not_override_full_key_limit() {
        let error = assert_rejected_without_writes(|writer| {
            DiagnosticWriter::new(
                writer.store.clone(),
                "a".repeat(MAX_AGENT_BYTES),
                VM_ID,
            )?
            .with_timestamp_precision(TimestampPrecision::Nanos)
            .with_duration_precision(DurationPrecision::Nanos)
            .emit_finish(
                EVENT_ID,
                &"n".repeat(DEFAULT_MAX_NAME_BYTES),
                "message",
                Some(Encoding::ZlibB64),
                Outcome::Success,
                Duration::MAX,
            )
        });
        assert!(matches!(
            error,
            KvpError::KeyTooLarge {
                max: MAX_KEY_BYTES,
                actual: 264
            }
        ));
    }

    #[test]
    fn start_rejects_invalid_event_id_before_io() {
        let error = assert_rejected_without_writes(|writer| {
            writer.emit_start("invalid", "test", "message", None)
        });
        assert!(matches!(error, KvpError::InvalidUuid { field: "event_id" }));
    }

    #[test]
    fn finish_rejects_invalid_name_before_io() {
        let error = assert_rejected_without_writes(|writer| {
            writer.emit_finish(
                EVENT_ID,
                "bad|name",
                "message",
                None,
                Outcome::Failure,
                Duration::ZERO,
            )
        });
        assert!(matches!(
            error,
            KvpError::EventFieldContainsDelimiter { field: "name" }
        ));
    }

    #[test]
    fn event_rejects_empty_name_before_io() {
        let error = assert_rejected_without_writes(|writer| {
            writer.emit_event("", "message", None, None, None)
        });
        assert!(matches!(error, KvpError::EmptyEventField { field: "name" }));
    }

    #[test]
    fn start_rejects_invalid_utf8_before_io() {
        let error = assert_rejected_without_writes(|writer| {
            writer.emit_start(EVENT_ID, "test", vec![0xff], None)
        });
        assert!(matches!(error, KvpError::PayloadNotUtf8));
    }

    #[test]
    fn finish_rejects_late_nul_before_writing_any_chunks() {
        let payload = format!("{}\0", "x".repeat(MAX_CHUNK_BYTES));
        let error = assert_rejected_without_writes(|writer| {
            writer.emit_finish(
                EVENT_ID,
                "test",
                payload,
                None,
                Outcome::Failure,
                Duration::ZERO,
            )
        });
        assert!(matches!(error, KvpError::ValueContainsNull));
    }

    #[test]
    fn event_rejects_unsupported_encoding_before_io() {
        let error = assert_rejected_without_writes(|writer| {
            writer.emit_event(
                "test",
                "message",
                Some(Encoding::Other("zstd".into())),
                None,
                None,
            )
        });
        assert!(matches!(
            error,
            KvpError::UnsupportedEncoding { token } if token == "zstd"
        ));
    }

    #[test]
    fn writer_rejects_absent_vm_identity() {
        let mut missing_vm = key();
        missing_vm.vm_id = None;
        assert!(matches!(
            prepare_records(
                event(missing_vm, "bad".into()),
                TimestampPrecision::Millis,
                DurationPrecision::default(),
                DEFAULT_MAX_NAME_BYTES,
            ),
            Err(KvpError::EmptyEventField { field: "vm_id" })
        ));
    }

    #[test]
    fn writer_rejects_expanded_timestamp_years() {
        let mut expanded_year = key();
        expanded_year.timestamp =
            DateTime::from_timestamp(253_402_300_800, 0).unwrap();
        assert!(matches!(
            prepare_records(
                event(expanded_year, "bad".into()),
                TimestampPrecision::Nanos,
                DurationPrecision::default(),
                DEFAULT_MAX_NAME_BYTES,
            ),
            Err(KvpError::EventFieldTooLong {
                field: "timestamp",
                max: 30,
                ..
            })
        ));
    }

    #[test]
    fn appends_preserve_existing_records_and_duplicates() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        pool.append("existing", "first").unwrap();
        pool.append("existing", "second").unwrap();
        let writer = DiagnosticWriter::new(pool.clone(), AGENT, VM_ID).unwrap();
        writer.emit_event("test", "new", None, None, None).unwrap();
        let records = pool.dump().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0], ("existing".into(), "first".into()));
        assert_eq!(records[1], ("existing".into(), "second".into()));
    }

    #[test]
    fn constructor_defers_storage_errors_until_emission() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir, PoolMode::Safe);
        fs::create_dir(pool.path()).unwrap();
        let writer = DiagnosticWriter::new(pool, AGENT, VM_ID).unwrap();
        assert!(matches!(
            writer.emit_event("test", "value", None, None, None),
            Err(KvpError::Io(_))
        ));
    }
}
