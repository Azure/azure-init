// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::cloud_init;
use super::diagnostic::{
    DecodeError, Diagnostic, DiagnosticEvent, DiagnosticFinish, DiagnosticKey,
    DiagnosticPayload, DiagnosticStart, Encoding, Entry, Outcome, RawKeyValue,
    DIAGNOSTIC_VERSION_ID,
};
use super::encoding::decode_payload;
use super::parse_unsigned;
use crate::{
    KvpError, KvpPoolStore, ProvisioningReport, PROVISIONING_REPORT_KEY,
};

/// Reads diagnostics and provisioning reports from a KVP pool.
///
/// Native and cloud-init diagnostics are decoded. Other or invalid records are
/// returned as [`Entry::Raw`]. Agent and VM identities come from the records,
/// so no local identity is required.
#[derive(Clone, Debug)]
pub struct DiagnosticReader {
    store: KvpPoolStore,
}

impl DiagnosticReader {
    /// Creates a reader for `store`; the pool is accessed by [`entries`](Self::entries).
    pub fn new(store: KvpPoolStore) -> Self {
        Self { store }
    }

    /// Reads the current pool contents without modifying them.
    ///
    /// Returns an empty list if the pool file does not exist. Entries retain
    /// pool order, and unrelated or invalid records remain [`Entry::Raw`].
    ///
    /// # Errors
    /// Returns [`KvpError`] if the pool cannot be read or has invalid storage
    /// layout or UTF-8; no entries are returned. Per-record decoding failures
    /// are returned as [`Entry::Raw`] values with a [`DecodeError`].
    pub fn entries(&self) -> Result<Vec<Entry>, KvpError> {
        Ok(decode_entries(self.store.dump()?))
    }
}

struct Chunk {
    position: usize,
    index: u64,
    raw: RawKeyValue,
}

struct ChunkGroup {
    first_position: usize,
    chunks: Vec<Chunk>,
}

fn decode_entries(records: Vec<(String, String)>) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut groups = HashMap::<String, ChunkGroup>::new();

    for (position, (key, value)) in records.into_iter().enumerate() {
        let mut raw = RawKeyValue {
            key,
            value,
            error: None,
        };
        let prefix = raw.key.split('|').next().unwrap_or_default();
        let parsed = match prefix {
            DIAGNOSTIC_VERSION_ID => split_chunk_key(&raw.key)
                .map(|(base, index)| (base, Some(index))),
            cloud_init::PREFIX => cloud_init::split_key(&raw.key),
            PROVISIONING_REPORT_KEY if raw.key == PROVISIONING_REPORT_KEY => {
                match raw.value.parse::<ProvisioningReport>() {
                    Ok(report) => {
                        entries.push((position, Entry::Report(report)))
                    }
                    Err(error) => {
                        raw.error = Some(error);
                        entries.push((position, Entry::Raw(raw)));
                    }
                }
                continue;
            }
            version if version.starts_with("DIAG_V") => {
                Err(DecodeError::UnsupportedVersion)
            }
            _ => {
                entries.push((position, Entry::Raw(raw)));
                continue;
            }
        };
        match parsed {
            Ok((base, Some(index))) => {
                groups
                    .entry(base.to_owned())
                    .or_insert_with(|| ChunkGroup {
                        first_position: position,
                        chunks: Vec::new(),
                    })
                    .chunks
                    .push(Chunk {
                        position,
                        index,
                        raw,
                    });
            }
            Ok((base, None)) => {
                match cloud_init::decode_single(base, &raw.value) {
                    Ok(diagnostic) => {
                        entries.push((position, Entry::Diagnostic(diagnostic)))
                    }
                    Err(error) => {
                        raw.error = Some(error);
                        entries.push((position, Entry::Raw(raw)));
                    }
                }
            }
            Err(error) => {
                raw.error = Some(error);
                entries.push((position, Entry::Raw(raw)));
            }
        }
    }

    for (base, mut group) in groups {
        let decoded = if base.split('|').next() == Some(cloud_init::PREFIX) {
            order_chunks(&mut group.chunks).and_then(|()| {
                cloud_init::decode_chunks(
                    &base,
                    group
                        .chunks
                        .iter()
                        .map(|chunk| (chunk.index, chunk.raw.value.as_str())),
                )
            })
        } else {
            decode_diag_group(&base, &mut group.chunks)
        };
        match decoded {
            Ok(diagnostic) => {
                entries.push((
                    group.first_position,
                    Entry::Diagnostic(diagnostic),
                ));
            }
            Err(error) => {
                for mut chunk in group.chunks {
                    chunk.raw.error = Some(error);
                    entries.push((chunk.position, Entry::Raw(chunk.raw)));
                }
            }
        }
    }

    // Failed groups retain every physical record at its original position.
    entries.sort_by_key(|(position, _)| *position);
    entries.into_iter().map(|(_, entry)| entry).collect()
}

fn split_chunk_key(key: &str) -> Result<(&str, u64), DecodeError> {
    let (base, index) = key.rsplit_once('|').ok_or(DecodeError::Malformed)?;
    Ok((base, parse_unsigned(index)?))
}

fn decode_diag_group(
    base: &str,
    chunks: &mut [Chunk],
) -> Result<Diagnostic, DecodeError> {
    let [version, agent, vm_id, kind, name, event_id, timestamp, encoding, result, duration]: [&str; 10] =
        base.split('|')
            .collect::<Vec<_>>()
            .try_into()
            .map_err(|_| DecodeError::Malformed)?;
    if version != DIAGNOSTIC_VERSION_ID
        || base.contains('\0')
        || [agent, vm_id, name, event_id, timestamp, encoding]
            .iter()
            .any(|field| field.is_empty())
    {
        return Err(DecodeError::Malformed);
    }
    Uuid::parse_str(vm_id).map_err(|_| DecodeError::Malformed)?;
    Uuid::parse_str(event_id).map_err(|_| DecodeError::Malformed)?;
    let parsed_timestamp = DateTime::parse_from_rfc3339(timestamp)
        .map_err(|_| DecodeError::Malformed)?
        .with_timezone(&Utc);
    let result = match result {
        "" => None,
        "success" => Some(Outcome::Success),
        "fail" => Some(Outcome::Failure),
        _ => return Err(DecodeError::Malformed),
    };
    let duration = if duration.is_empty() {
        None
    } else {
        Some(parse_duration(duration)?)
    };
    let encoding = match encoding {
        "none" => None,
        "zlib+b64" => Some(Encoding::ZlibB64),
        "gz+b64" => Some(Encoding::GzB64),
        other => Some(Encoding::Other(other.to_owned())),
    };
    let key = DiagnosticKey {
        agent: agent.to_owned(),
        vm_id: Some(vm_id.to_owned()),
        name: name.to_owned(),
        event_id: event_id.to_owned(),
        timestamp: parsed_timestamp,
        encoding,
    };

    match (kind, result, duration) {
        ("start", None, None) => {
            let payload = decode_chunks(chunks, key.encoding.as_ref())?;
            Ok(Diagnostic::Start(DiagnosticStart { key, payload }))
        }
        ("finish", Some(result), Some(duration)) => {
            let payload = decode_chunks(chunks, key.encoding.as_ref())?;
            Ok(Diagnostic::Finish(DiagnosticFinish {
                key,
                payload,
                result,
                duration,
            }))
        }
        ("event", result, duration) => {
            let payload = decode_chunks(chunks, key.encoding.as_ref())?;
            Ok(Diagnostic::Event(DiagnosticEvent {
                key,
                payload,
                result,
                duration,
            }))
        }
        _ => Err(DecodeError::Malformed),
    }
}

fn parse_duration(value: &str) -> Result<Duration, DecodeError> {
    parse_decimal_duration(value).or_else(|_| {
        let seconds =
            value.parse::<f64>().map_err(|_| DecodeError::Malformed)?;
        Duration::try_from_secs_f64(seconds).map_err(|_| DecodeError::Malformed)
    })
}

fn parse_decimal_duration(value: &str) -> Result<Duration, DecodeError> {
    let (seconds, fraction) = value
        .split_once('.')
        .map_or((value, None), |(seconds, fraction)| {
            (seconds, Some(fraction))
        });
    let seconds = parse_unsigned(seconds)?;
    let nanos = match fraction {
        None => 0,
        Some(fraction) => {
            if fraction.is_empty()
                || fraction.len() > 9
                || !fraction.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(DecodeError::Malformed);
            }
            fraction
                .parse::<u32>()
                .map_err(|_| DecodeError::Malformed)?
                * 10u32.pow(9 - fraction.len() as u32)
        }
    };
    Ok(Duration::new(seconds, nanos))
}

fn decode_chunks(
    chunks: &mut [Chunk],
    encoding: Option<&Encoding>,
) -> Result<DiagnosticPayload, DecodeError> {
    order_chunks(chunks)?;
    let value: String = chunks
        .iter()
        .map(|chunk| chunk.raw.value.as_str())
        .collect();
    decode_payload(value.as_bytes(), encoding)
}

fn order_chunks(chunks: &mut [Chunk]) -> Result<(), DecodeError> {
    chunks.sort_by_key(|chunk| chunk.index);
    if chunks.windows(2).any(|pair| pair[0].index == pair[1].index) {
        return Err(DecodeError::DuplicateChunk);
    }
    if chunks
        .iter()
        .enumerate()
        .any(|(expected, chunk)| usize::try_from(chunk.index) != Ok(expected))
    {
        return Err(DecodeError::IncompleteGroup);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use chrono::SecondsFormat;
    use rstest::rstest;
    use tempfile::TempDir;

    use super::super::diagnostic::Kind;
    use crate::store::{Handle, OsSysOps, StatInfo, SysOps};
    use crate::{write_report, KvpPool, PoolMode, ReportPpsType};

    const AGENT: &str = "azure-init/0.1.1";
    const VM_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    const EVENT_ID: &str = "8f3e9c4a-1b2c-4d5e-9f01-234567890abc";
    const TIMESTAMP: &str = "2026-08-31T12:34:56.789Z";
    const GZIP_HELLO: &str = "H4sIAAAAAAAC/8tIzcnJBwCGphA2BQAAAA==";

    fn key(index: u64) -> String {
        format!(
            "DIAG|{AGENT}|{VM_ID}|event|test|{EVENT_ID}|{TIMESTAMP}|none|||{index}"
        )
    }

    fn with_field(key: &str, index: usize, value: &str) -> String {
        let mut fields: Vec<_> = key.split('|').collect();
        fields[index] = value;
        fields.join("|")
    }

    fn raw_entries(
        records: &[(String, String)],
        error: Option<DecodeError>,
    ) -> Vec<Entry> {
        records
            .iter()
            .map(|(key, value)| {
                Entry::Raw(RawKeyValue {
                    key: key.clone(),
                    value: value.clone(),
                    error,
                })
            })
            .collect()
    }

    fn only_diagnostic(mut entries: Vec<Entry>) -> Diagnostic {
        assert_eq!(entries.len(), 1);
        let mut diagnostic = None;
        if let Some(Entry::Diagnostic(value)) = entries.pop() {
            diagnostic = Some(value);
        }
        diagnostic.expect("expected a diagnostic")
    }

    fn store(dir: &TempDir) -> KvpPoolStore {
        KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe)
            .unwrap()
    }

    #[derive(Debug, Default)]
    struct ReaderOps {
        os: OsSysOps,
        calls: AtomicUsize,
        open_error: Option<io::ErrorKind>,
    }

    impl SysOps for ReaderOps {
        fn open_read(&self, path: &Path) -> io::Result<Box<dyn Handle>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.open_error {
                return Err(error.into());
            }
            self.os.open_read(path)
        }

        fn open_read_write(&self, _: &Path) -> io::Result<Box<dyn Handle>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(io::ErrorKind::Unsupported.into())
        }

        fn open_read_write_create(
            &self,
            _: &Path,
        ) -> io::Result<Box<dyn Handle>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(io::ErrorKind::Unsupported.into())
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

    fn observed_reader(dir: &TempDir) -> (DiagnosticReader, Arc<ReaderOps>) {
        let ops = Arc::new(ReaderOps::default());
        let observed = KvpPoolStore::with_ops(
            KvpPool::Guest,
            dir.path(),
            PoolMode::Safe,
            ops.clone(),
        )
        .unwrap();
        (DiagnosticReader::new(observed), ops)
    }

    #[test]
    fn reader_ops_rejects_non_read_operations() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir);
        let ops = ReaderOps::default();
        assert_eq!(
            [
                ops.open_read_write(pool.path()).unwrap_err().kind(),
                ops.open_read_write_create(pool.path()).unwrap_err().kind(),
                ops.path_metadata(pool.path()).unwrap_err().kind(),
                ops.boot_time().unwrap_err().kind(),
            ],
            [io::ErrorKind::Unsupported; 4]
        );
        assert_eq!(ops.calls.load(Ordering::SeqCst), 4);
        assert!(!pool.path().exists());
    }

    #[test]
    fn constructor_does_no_io() {
        let dir = TempDir::new().unwrap();
        let (reader, ops) = observed_reader(&dir);
        assert_eq!(ops.calls.load(Ordering::SeqCst), 0);
        assert!(!reader.store.path().exists());
    }

    #[test]
    fn entries_reads_one_fresh_snapshot_without_modifying_the_pool() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir);
        let (reader, ops) = observed_reader(&dir);
        assert_eq!(ops.calls.load(Ordering::SeqCst), 0);
        assert!(reader.entries().unwrap().is_empty());
        assert_eq!(ops.calls.load(Ordering::SeqCst), 1);
        assert!(!pool.path().exists());

        pool.append("unrelated", "unchanged").unwrap();
        let before = fs::read(pool.path()).unwrap();
        assert_eq!(
            reader.entries().unwrap(),
            raw_entries(&[("unrelated".into(), "unchanged".into())], None)
        );
        assert_eq!(ops.calls.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read(pool.path()).unwrap(), before);
    }

    #[test]
    fn snapshot_open_errors_propagate() {
        let dir = TempDir::new().unwrap();
        let ops = Arc::new(ReaderOps {
            open_error: Some(io::ErrorKind::PermissionDenied),
            ..ReaderOps::default()
        });
        let pool = KvpPoolStore::with_ops(
            KvpPool::Guest,
            dir.path(),
            PoolMode::Safe,
            ops.clone(),
        )
        .unwrap();
        let reader = DiagnosticReader::new(pool);
        assert_eq!(ops.calls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            reader.entries(),
            Err(KvpError::Io(error))
                if error.kind() == io::ErrorKind::PermissionDenied
        ));
        assert_eq!(ops.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn truncated_physical_pool_is_a_snapshot_error() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir);
        fs::write(pool.path(), b"partial record").unwrap();
        let reader = DiagnosticReader::new(pool);
        assert!(matches!(
            reader.entries(),
            Err(KvpError::Io(error)) if error.kind() == io::ErrorKind::Other
        ));
    }

    #[rstest]
    #[case::key(0)]
    #[case::value(512)]
    fn non_utf8_record_fails_the_entire_snapshot(#[case] offset: usize) {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir);
        pool.append(&with_field(&key(0), 4, "before"), "valid")
            .unwrap();
        let record_start = fs::read(pool.path()).unwrap().len();
        pool.append(&key(0), "value").unwrap();
        pool.append(&with_field(&key(0), 4, "after"), "valid")
            .unwrap();
        let reader = DiagnosticReader::new(pool.clone());
        assert_eq!(reader.entries().unwrap().len(), 3);

        let mut bytes = fs::read(pool.path()).unwrap();
        bytes[record_start + offset] = 0xff;
        fs::write(pool.path(), &bytes).unwrap();
        assert!(matches!(
            reader.entries(),
            Err(KvpError::Io(error)) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert_eq!(fs::read(pool.path()).unwrap(), bytes);
    }

    #[rstest]
    #[case("")]
    #[case("unrelated")]
    #[case("other|0")]
    #[case("DIAG_OTHER|0")]
    #[case("diag|0")]
    #[case("prefixDIAG_V2|0")]
    #[case("azure-init-0.1.1|1700000000|vm-abc|event|imds|id|2026-08-31T12:34:56.789Z|0")]
    fn unrelated_and_pre_adoption_records_remain_raw(#[case] key: &str) {
        let records = vec![
            (key.into(), "first\nvalue".into()),
            (key.into(), "second \"value\"".into()),
        ];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, None)
        );
    }

    #[rstest]
    #[case::missing_fields("result=success".into())]
    #[case::conflicting_results(format!(
        "result=success|agent={AGENT}|pps_type=None|vm_id={VM_ID}|timestamp={TIMESTAMP}|result=error"
    ))]
    fn malformed_report_is_preserved_without_losing_other_entries(
        #[case] value: String,
    ) {
        let records = vec![(PROVISIONING_REPORT_KEY.into(), value)];
        let mut mixed = records.clone();
        mixed.push((key(0), "valid diagnostic".into()));
        let entries = decode_entries(mixed);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            raw_entries(&records, Some(DecodeError::Malformed))[0]
        );
        assert!(matches!(entries[1], Entry::Diagnostic(_)));
    }

    #[test]
    fn report_is_typed_in_first_seen_order() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir);
        let report = ProvisioningReport::failure(
            AGENT,
            VM_ID,
            "failed | with details",
            ReportPpsType::None,
        )
        .with_extra("detail", "first")
        .with_extra("detail", "second");
        pool.append(&key(1), "second chunk").unwrap();
        write_report(&pool, &report).unwrap();
        pool.append(&key(0), "first chunk").unwrap();
        let before = fs::read(pool.path()).unwrap();
        let entries = DiagnosticReader::new(pool.clone()).entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0], Entry::Diagnostic(_)));
        assert_eq!(entries[1], Entry::Report(report));
        assert_eq!(fs::read(pool.path()).unwrap(), before);
    }

    #[test]
    fn repeated_report_records_are_not_deduplicated() {
        let dir = TempDir::new().unwrap();
        let pool = store(&dir);
        let report =
            ProvisioningReport::success(AGENT, VM_ID, ReportPpsType::None);
        write_report(&pool, &report).unwrap();
        let value = pool.read(PROVISIONING_REPORT_KEY).unwrap().unwrap();
        pool.append(PROVISIONING_REPORT_KEY, &value).unwrap();
        assert_eq!(
            DiagnosticReader::new(pool).entries().unwrap(),
            vec![Entry::Report(report.clone()), Entry::Report(report)]
        );
    }

    #[test]
    fn report_prefix_is_not_a_chunked_report_key() {
        let records = vec![("PROVISIONING_REPORT|0".into(), "value".into())];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, None)
        );
    }

    #[rstest]
    #[case("DIAG_V0")]
    #[case("DIAG_V1")]
    #[case("DIAG_V2")]
    #[case("DIAG_V999")]
    #[case("DIAG_V1_extra")]
    #[case("DIAG_V")]
    fn unsupported_versions_do_not_interpret_later_fields(
        #[case] version: &str,
    ) {
        let records = vec![
            (version.into(), "value".into()),
            (format!("{version}|bad metadata|0"), "first".into()),
            (format!("{version}|bad metadata|0"), "duplicate".into()),
            (format!("{version}|bad metadata|2"), "gap".into()),
        ];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::UnsupportedVersion))
        );
    }

    #[test]
    fn native_event_decodes_without_local_identity() {
        let payload = "héllo\n\"message\" | =";
        let diagnostic =
            only_diagnostic(decode_entries(vec![(key(0), payload.into())]));
        assert_eq!(diagnostic.kind(), Kind::Event);
        assert_eq!(diagnostic.key().agent, AGENT);
        assert_eq!(diagnostic.key().vm_id.as_deref(), Some(VM_ID));
        assert_eq!(diagnostic.key().name, "test");
        assert_eq!(diagnostic.key().event_id, EVENT_ID);
        assert_eq!(diagnostic.key().encoding, None);
        assert_eq!(
            diagnostic
                .key()
                .timestamp
                .to_rfc3339_opts(SecondsFormat::Millis, true),
            TIMESTAMP
        );
        assert_eq!(
            diagnostic.payload(),
            &DiagnosticPayload::Text(payload.into())
        );
    }

    #[rstest]
    #[case::unmatched_start("start", "", "")]
    #[case::orphan_finish("finish", "fail", "0.000312")]
    fn isolated_span_endpoint_decodes(
        #[case] kind: &str,
        #[case] result: &str,
        #[case] duration: &str,
    ) {
        let key = with_field(&key(0), 3, kind);
        let key = with_field(&key, 8, result);
        let key = with_field(&key, 9, duration);
        let diagnostic =
            only_diagnostic(decode_entries(vec![(key, "message".into())]));
        assert_eq!(diagnostic.kind().to_string(), kind);
    }

    #[test]
    fn span_endpoints_with_a_shared_id_decode_separately() {
        let start = with_field(&key(0), 3, "start");
        let finish = with_field(&key(0), 3, "finish");
        let finish = with_field(&finish, 8, "fail");
        let finish = with_field(&finish, 9, "0.000312");
        let entries = decode_entries(vec![
            (start, "starting".into()),
            (finish, "failed".into()),
        ]);
        assert!(matches!(
            entries.as_slice(),
            [Entry::Diagnostic(Diagnostic::Start(start)), Entry::Diagnostic(Diagnostic::Finish(finish))]
                if start.key.event_id == EVENT_ID
                    && finish.key.event_id == EVENT_ID
                    && start.payload == DiagnosticPayload::Text("starting".into())
                    && finish.payload == DiagnosticPayload::Text("failed".into())
                    && finish.result == Outcome::Failure
                    && finish.duration == Duration::from_micros(312)
        ));
    }

    #[rstest]
    #[case::neither(None, None)]
    #[case::result_only(Some(Outcome::Success), None)]
    #[case::zero_duration(None, Some(0))]
    #[case::both(Some(Outcome::Failure), Some(52))]
    fn event_result_and_duration_are_independent(
        #[case] result: Option<Outcome>,
        #[case] duration_secs: Option<u64>,
    ) {
        let result_token = result.map_or_else(String::new, |v| v.to_string());
        let duration_token =
            duration_secs.map_or_else(String::new, |value| value.to_string());
        let key = with_field(&key(0), 8, &result_token);
        let key = with_field(&key, 9, &duration_token);
        let diagnostic =
            only_diagnostic(decode_entries(vec![(key, "value".into())]));
        assert!(matches!(&diagnostic, Diagnostic::Event(event)
                if event.result == result
                    && event.duration == duration_secs.map(Duration::from_secs)));
    }

    #[rstest]
    #[case("start", "success", "")]
    #[case("start", "", "0")]
    #[case("start", "fail", "52")]
    #[case("finish", "", "")]
    #[case("finish", "success", "")]
    #[case("finish", "", "0")]
    #[case("finish", "error", "52")]
    fn invalid_kind_fields_are_malformed(
        #[case] kind: &str,
        #[case] result: &str,
        #[case] duration: &str,
    ) {
        let base = with_field(&key(0), 3, kind);
        let base = with_field(&base, 8, result);
        let base = with_field(&base, 9, duration);
        let records = vec![(base, "value".into())];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::Malformed))
        );
    }

    #[rstest]
    #[case::empty_agent(1, "")]
    #[case::invalid_vm_id(2, "vm-abc")]
    #[case::invalid_kind(3, "compressed")]
    #[case::empty_name(4, "")]
    #[case::null_in_name(4, "bad\0name")]
    #[case::invalid_event_id(5, "bad-uuid")]
    #[case::empty_encoding(7, "")]
    #[case::invalid_result(8, "SUCCESS")]
    #[case::negative_duration(9, "-0.1")]
    #[case::non_finite_duration(9, "NaN")]
    #[case::infinite_duration(9, "inf")]
    #[case::duration_whitespace(9, " 1")]
    #[case::float_overflow(9, "1e100")]
    #[case::signed_fraction(9, "1.+2")]
    #[case::numeric_overflow(9, "18446744073709551616")]
    #[case::missing_chunk_index(10, "")]
    #[case::non_numeric_chunk_index(10, "x")]
    fn malformed_native_fields_preserve_the_original_record(
        #[case] field: usize,
        #[case] value: &str,
    ) {
        let records = vec![(with_field(&key(0), field, value), "value".into())];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::Malformed))
        );
    }

    #[test]
    fn native_format_requires_the_exact_key_layout_and_an_index() {
        let complete = key(0);
        let records = vec![
            ("DIAG".into(), "value".into()),
            (complete.rsplit_once('|').unwrap().0.into(), "value".into()),
            (format!("{complete}|1"), "value".into()),
        ];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::Malformed))
        );
    }

    #[rstest]
    #[case::unparsable("not a timestamp")]
    #[case::missing_offset("2026-08-31T12:34:56.789")]
    #[case::invalid_month("2026-13-31T12:34:56.789Z")]
    fn native_format_rejects_invalid_timestamps(#[case] timestamp: &str) {
        let records = vec![(with_field(&key(0), 6, timestamp), "value".into())];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::Malformed))
        );
    }

    #[rstest]
    #[case::seconds("2026-08-31T12:34:56Z", 0)]
    #[case::milliseconds("2026-08-31T12:34:56.789Z", 789_000_000)]
    #[case::millisecond_whole("2026-08-31T12:34:56.000Z", 0)]
    #[case::microseconds("2026-08-31T12:34:56.789123Z", 789_123_000)]
    #[case::microsecond_trailing_zeros(
        "2026-08-31T12:34:56.789000Z",
        789_000_000
    )]
    #[case::nanoseconds("2026-08-31T12:34:56.789123456Z", 789_123_456)]
    #[case::two_fraction_digits("2026-08-31T12:34:56.78Z", 780_000_000)]
    #[case::four_fraction_digits("2026-08-31T12:34:56.7890Z", 789_000_000)]
    #[case::numeric_offset("2026-08-31T14:34:56.789+02:00", 789_000_000)]
    #[case::lowercase("2026-08-31t12:34:56.789z", 789_000_000)]
    #[case::subnanoseconds("2026-08-31T12:34:56.7891234567Z", 789_123_456)]
    fn native_format_accepts_rfc3339_timestamps(
        #[case] timestamp: &str,
        #[case] expected_nanos: u32,
    ) {
        let key = with_field(&key(0), 6, timestamp);
        let diagnostic =
            only_diagnostic(decode_entries(vec![(key, "value".into())]));
        assert_eq!(
            diagnostic
                .key()
                .timestamp
                .to_rfc3339_opts(SecondsFormat::Nanos, true),
            format!("2026-08-31T12:34:56.{expected_nanos:09}Z")
        );
    }

    #[rstest]
    #[case([1, 0, 2])]
    #[case([2, 1, 0])]
    fn chunk_permutations_decode_in_index_order(#[case] order: [u64; 3]) {
        let records = order
            .into_iter()
            .map(|index| (key(index), ["a", "é", "c"][index as usize].into()))
            .collect();
        let diagnostic = only_diagnostic(decode_entries(records));
        assert_eq!(
            diagnostic.payload(),
            &DiagnosticPayload::Text("aéc".into())
        );
    }

    #[test]
    fn logical_groups_use_first_seen_order_not_timestamps() {
        let later = with_field(&key(0), 6, "2026-08-31T12:35:00.000Z");
        let earlier = with_field(&key(0), 4, "earlier");
        let records = vec![
            (with_field(&later, 10, "1"), "b".into()),
            ("raw".into(), "untouched".into()),
            (earlier, "early".into()),
            (later, "a".into()),
        ];
        let entries = decode_entries(records);
        assert!(matches!(
            entries.as_slice(),
            [Entry::Diagnostic(first), Entry::Raw(raw), Entry::Diagnostic(last)]
                if first.payload() == &DiagnosticPayload::Text("ab".into())
                    && raw.key == "raw"
                    && first.key().timestamp > last.key().timestamp
        ));
    }

    #[rstest]
    #[case(vec![1])]
    #[case(vec![0, 2])]
    #[case(vec![2, 0])]
    #[case(vec![u64::MAX])]
    #[case(vec![0, u64::MAX])]
    fn missing_chunks_preserve_all_members(#[case] indices: Vec<u64>) {
        let records: Vec<_> = indices
            .into_iter()
            .map(|index| (key(index), format!("chunk {index}")))
            .collect();
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::IncompleteGroup))
        );
    }

    #[rstest]
    #[case(vec![0, 0])]
    #[case(vec![1, 1])]
    #[case(vec![0, 1, 1])]
    #[case(vec![2, 0, 2])]
    fn duplicates_take_precedence_over_gaps(#[case] indices: Vec<u64>) {
        let records: Vec<_> = indices
            .into_iter()
            .enumerate()
            .map(|(position, index)| {
                (key(index), format!("original {position}"))
            })
            .collect();
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::DuplicateChunk))
        );
    }

    #[test]
    fn duplicate_index_spellings_preserve_the_exact_keys() {
        let records = vec![
            (key(0), "first".into()),
            (with_field(&key(0), 10, "00"), "second".into()),
        ];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::DuplicateChunk))
        );
    }

    #[test]
    fn failed_groups_preserve_physical_positions_among_other_entries() {
        let records = vec![
            (key(2), "first seen".into()),
            ("unrelated".into(), "unchanged".into()),
            (key(0), "zero".into()),
            (with_field(&key(0), 4, "valid"), "good".into()),
            (key(2), "duplicate".into()),
        ];
        let entries = decode_entries(records.clone());
        assert_eq!(entries.len(), records.len());
        for position in [0, 2, 4] {
            let expected = raw_entries(
                &records[position..=position],
                Some(DecodeError::DuplicateChunk),
            );
            assert_eq!(entries[position], expected[0]);
        }
        assert_eq!(entries[1], raw_entries(&records[1..2], None)[0]);
        assert!(
            matches!(&entries[3], Entry::Diagnostic(d) if d.key().name == "valid")
        );
    }

    #[rstest]
    #[case(1, "other-agent")]
    #[case(2, "00000000-0000-0000-0000-000000000000")]
    #[case(3, "start")]
    #[case(4, "other-name")]
    #[case(5, "00000000-0000-0000-0000-000000000000")]
    #[case(6, "2026-08-31T12:34:56.790Z")]
    #[case(7, "gz+b64")]
    #[case(8, "success")]
    #[case(9, "52")]
    fn grouping_does_not_combine_different_metadata(
        #[case] field: usize,
        #[case] value: &str,
    ) {
        let detached = (with_field(&key(1), field, value), "detached".into());
        let entries = decode_entries(vec![
            (key(0), "a".into()),
            detached.clone(),
            (key(1), "b".into()),
        ]);
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            &entries[0],
            Entry::Diagnostic(d) if d.payload() == &DiagnosticPayload::Text("ab".into())
        ));
        assert_eq!(
            entries[1],
            raw_entries(&[detached], Some(DecodeError::IncompleteGroup))[0]
        );
    }

    #[rstest]
    #[case(2, VM_ID, "3F2504E0-4F89-41D3-9A0C-0305E82C3301")]
    #[case(5, EVENT_ID, "8f3e9c4a1b2c4d5e9f01234567890abc")]
    #[case(9, "52", "052")]
    fn equivalent_metadata_spellings_remain_separate_groups(
        #[case] field: usize,
        #[case] first: &str,
        #[case] second: &str,
    ) {
        let records = vec![
            (with_field(&key(0), field, first), "a".into()),
            (with_field(&key(0), field, second), "b".into()),
            (with_field(&key(1), field, first), "c".into()),
            (with_field(&key(1), field, second), "d".into()),
        ];
        let entries = decode_entries(records);
        assert!(matches!(
            entries.as_slice(),
            [Entry::Diagnostic(first), Entry::Diagnostic(second)]
                if first.payload() == &DiagnosticPayload::Text("ac".into())
                    && second.payload() == &DiagnosticPayload::Text("bd".into())
        ));
    }

    #[test]
    fn versions_are_never_grouped_together() {
        let future = (with_field(&key(1), 0, "DIAG_V2"), "future chunk".into());
        let entries = decode_entries(vec![
            (key(0), "a".into()),
            future.clone(),
            (key(1), "b".into()),
        ]);
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            &entries[0],
            Entry::Diagnostic(d) if d.payload() == &DiagnosticPayload::Text("ab".into())
        ));
        assert_eq!(
            entries[1],
            raw_entries(&[future], Some(DecodeError::UnsupportedVersion))[0]
        );
    }

    #[rstest]
    #[case("base64")]
    #[case("zstd+b64")]
    #[case("GZ+B64")]
    fn unknown_encodings_preserve_every_chunk(#[case] encoding: &str) {
        let records = vec![
            (with_field(&key(1), 7, encoding), "b".into()),
            (with_field(&key(0), 7, encoding), "a".into()),
        ];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::Undecodable))
        );
    }

    #[test]
    fn compressed_payload_is_joined_before_decoding() {
        let records = vec![
            (with_field(&key(1), 7, "gz+b64"), GZIP_HELLO[5..].into()),
            (with_field(&key(0), 7, "gz+b64"), GZIP_HELLO[..5].into()),
        ];
        let diagnostic = only_diagnostic(decode_entries(records));
        assert_eq!(diagnostic.key().encoding, Some(Encoding::GzB64));
        assert_eq!(
            diagnostic.payload(),
            &DiagnosticPayload::Bytes(b"hello".to_vec())
        );
    }

    #[test]
    fn corrupt_gzip_preserves_every_original_chunk() {
        let mut gzip = STANDARD.decode(GZIP_HELLO).unwrap();
        let checksum_offset = gzip.len() - 8;
        gzip[checksum_offset] ^= 1;
        let value = STANDARD.encode(gzip);
        let records = vec![
            (with_field(&key(1), 7, "gz+b64"), value[5..].into()),
            (with_field(&key(0), 7, "gz+b64"), value[..5].into()),
        ];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::Undecodable))
        );
    }

    #[test]
    fn visible_gaps_are_reported_before_payload_errors() {
        let records =
            vec![(with_field(&key(1), 7, "gz+b64"), "not base64".into())];
        assert_eq!(
            decode_entries(records.clone()),
            raw_entries(&records, Some(DecodeError::IncompleteGroup))
        );
    }

    #[test]
    fn empty_text_payload_decodes() {
        let diagnostic =
            only_diagnostic(decode_entries(vec![(key(0), String::new())]));
        assert_eq!(
            diagnostic.payload(),
            &DiagnosticPayload::Text(String::new())
        );
    }

    #[rstest]
    #[case::agent(1, "a".repeat(64))]
    #[case::name(4, "n".repeat(160))]
    fn reader_accepts_fields_above_writer_budgets(
        #[case] field: usize,
        #[case] value: String,
    ) {
        let dir = TempDir::new().unwrap();
        let pool =
            KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Unsafe)
                .unwrap();
        pool.append(&with_field(&key(0), field, &value), "payload")
            .unwrap();
        let diagnostic = only_diagnostic(
            DiagnosticReader::new(store(&dir)).entries().unwrap(),
        );
        let actual = match field {
            1 => &diagnostic.key().agent,
            _ => &diagnostic.key().name,
        };
        assert_eq!(actual, &value);
    }

    #[rstest]
    #[case("0", Duration::ZERO)]
    #[case("-0.0", Duration::ZERO)]
    #[case("+1", Duration::from_secs(1))]
    #[case("1.", Duration::from_secs(1))]
    #[case(".5", Duration::from_millis(500))]
    #[case("1.5", Duration::from_millis(1500))]
    #[case("0.312000", Duration::from_millis(312))]
    #[case("3.12e-1", Duration::from_millis(312))]
    #[case("1.000000001", Duration::new(1, 1))]
    #[case("0.9999999996", Duration::from_secs(1))]
    #[case("1e-12", Duration::ZERO)]
    #[case(
        "9007199254740993.000000001",
        Duration::new(9_007_199_254_740_993, 1)
    )]
    #[case("18446744073709551615.999999999", Duration::MAX)]
    fn reader_accepts_duration_seconds(
        #[case] seconds: &str,
        #[case] expected: Duration,
    ) {
        let key = with_field(&key(0), 9, seconds);
        let diagnostic =
            only_diagnostic(decode_entries(vec![(key, "payload".into())]));
        assert!(matches!(&diagnostic, Diagnostic::Event(event)
                if event.duration == Some(expected)));
    }

    #[test]
    fn reader_accepts_values_above_safe_write_limit() {
        let dir = TempDir::new().unwrap();
        let pool =
            KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Unsafe)
                .unwrap();
        let value = "v".repeat(1500);
        pool.append(&key(0), &value).unwrap();
        let diagnostic = only_diagnostic(
            DiagnosticReader::new(store(&dir)).entries().unwrap(),
        );
        assert_eq!(diagnostic.payload(), &DiagnosticPayload::Text(value));
    }

    #[test]
    fn reader_does_not_limit_contiguous_group_size() {
        let records = (0..1024)
            .rev()
            .map(|index| (key(index), "x".into()))
            .collect();
        let diagnostic = only_diagnostic(decode_entries(records));
        assert_eq!(
            diagnostic.payload(),
            &DiagnosticPayload::Text("x".repeat(1024))
        );
    }
}
