// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Read;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use flate2::bufread::{GzDecoder, ZlibDecoder};
use serde_json::{Number, Value};
use uuid::Uuid;

use super::diagnostic::{
    DecodeError, Diagnostic, DiagnosticEvent, DiagnosticFinish, DiagnosticKey,
    DiagnosticPayload, DiagnosticStart, Encoding, Outcome,
};

pub(super) const PREFIX: &str = "CLOUD_INIT";

struct CloudInitKey<'a> {
    kind: &'a str,
    name: &'a str,
    vm_id: Option<&'a str>,
    event_id: &'a str,
}

pub(super) fn split_key(key: &str) -> Result<(&str, Option<u64>), DecodeError> {
    let fields: Vec<_> = key.split('|').collect();
    let indexed = match fields.len() {
        5 => false,
        6 => Uuid::parse_str(fields[5]).is_err(),
        7 => true,
        _ => return Err(DecodeError::Malformed),
    };
    let (base, index) = if indexed {
        let (base, index) =
            key.rsplit_once('|').ok_or(DecodeError::Malformed)?;
        (base, Some(super::parse_unsigned(index)?))
    } else {
        (key, None)
    };
    parse_key(base)?;
    Ok((base, index))
}

fn parse_key(base: &str) -> Result<CloudInitKey<'_>, DecodeError> {
    let fields: Vec<_> = base.split('|').collect();
    let (prefix, incarnation, kind, name, vm_id, event_id) =
        match fields.as_slice() {
            [prefix, incarnation, kind, name, event_id] => {
                (*prefix, *incarnation, *kind, *name, None, *event_id)
            }
            [prefix, incarnation, kind, name, vm_id, event_id] => {
                (*prefix, *incarnation, *kind, *name, Some(*vm_id), *event_id)
            }
            _ => return Err(DecodeError::Malformed),
        };
    if prefix != PREFIX
        || base.contains('\0')
        || fields.iter().any(|field| field.is_empty())
        || !incarnation.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(DecodeError::Malformed);
    }
    if let Some(vm_id) = vm_id {
        Uuid::parse_str(vm_id).map_err(|_| DecodeError::Malformed)?;
    }
    Uuid::parse_str(event_id).map_err(|_| DecodeError::Malformed)?;
    Ok(CloudInitKey {
        kind,
        name,
        vm_id,
        event_id,
    })
}

pub(super) fn decode_single(
    base: &str,
    value: &str,
) -> Result<Diagnostic, DecodeError> {
    let key = parse_key(base)?;
    let metadata: Value =
        serde_json::from_str(value).map_err(|_| DecodeError::Malformed)?;
    if metadata.get("msg_i").is_some() {
        return Err(DecodeError::Malformed);
    }
    let message = metadata
        .get("msg")
        .and_then(Value::as_str)
        .ok_or(DecodeError::Malformed)?;
    diagnostic(key, &metadata, message.to_owned())
}

/// Chunk indices are already ordered and checked by the reader.
pub(super) fn decode_chunks<'a>(
    base: &str,
    chunks: impl Iterator<Item = (u64, &'a str)>,
) -> Result<Diagnostic, DecodeError> {
    let key = parse_key(base)?;
    let mut metadata = None;
    let mut escaped_message = String::new();
    for (index, value) in chunks {
        let (mut current, fragment) = chunk_parts(value)?;
        if current.get("msg_i").and_then(Value::as_u64) != Some(index) {
            return Err(DecodeError::Malformed);
        }
        current
            .as_object_mut()
            .ok_or(DecodeError::Malformed)?
            .remove("msg_i");
        if metadata.as_ref().is_some_and(|first| first != &current) {
            return Err(DecodeError::Malformed);
        }
        metadata.get_or_insert(current);
        escaped_message.push_str(fragment);
    }
    let message = serde_json::from_str(&format!("\"{escaped_message}\""))
        .map_err(|_| DecodeError::Malformed)?;
    diagnostic(key, &metadata.ok_or(DecodeError::Malformed)?, message)
}

fn chunk_parts(value: &str) -> Result<(Value, &str), DecodeError> {
    // cloud-init puts `msg` last and slices its escaped JSON string verbatim.
    let (prefix, message) = value
        .rsplit_once(",\"msg\":")
        .ok_or(DecodeError::Malformed)?;
    let fragment = message
        .strip_prefix('"')
        .and_then(|message| message.strip_suffix("\"}"))
        .ok_or(DecodeError::Malformed)?;
    let metadata = serde_json::from_str(&format!("{prefix}}}"))
        .map_err(|_| DecodeError::Malformed)?;
    Ok((metadata, fragment))
}

fn diagnostic(
    source: CloudInitKey<'_>,
    metadata: &Value,
    message: String,
) -> Result<Diagnostic, DecodeError> {
    if metadata.get("name").and_then(Value::as_str) != Some(source.name)
        || metadata.get("type").and_then(Value::as_str) != Some(source.kind)
    {
        return Err(DecodeError::Malformed);
    }
    let timestamp = metadata
        .get("ts")
        .and_then(Value::as_str)
        .ok_or(DecodeError::Malformed)?;
    let timestamp = DateTime::parse_from_rfc3339(timestamp)
        .map_err(|_| DecodeError::Malformed)?
        .with_timezone(&Utc);
    let (payload, encoding) =
        decode_message(message, source.kind == "compressed")?;
    let key = DiagnosticKey {
        agent: PREFIX.to_owned(),
        vm_id: source.vm_id.map(str::to_owned),
        name: source.name.to_owned(),
        event_id: source.event_id.to_owned(),
        timestamp,
        encoding,
    };
    match source.kind {
        "start" => {
            if metadata.get("result").is_some()
                || metadata.get("duration").is_some()
            {
                return Err(DecodeError::Malformed);
            }
            Ok(Diagnostic::Start(DiagnosticStart { key, payload }))
        }
        "finish" => {
            let result = match metadata.get("result").and_then(Value::as_str) {
                Some("SUCCESS") => Outcome::Success,
                Some("FAIL") => Outcome::Failure,
                _ => return Err(DecodeError::Malformed),
            };
            let duration = metadata
                .get("duration")
                .and_then(Value::as_number)
                .ok_or(DecodeError::Malformed)?;
            Ok(Diagnostic::Finish(DiagnosticFinish {
                key,
                payload,
                result,
                duration_ms: duration_ms(duration)?,
            }))
        }
        _ => Ok(Diagnostic::Event(DiagnosticEvent {
            key,
            payload,
            result: None,
            duration_ms: None,
        })),
    }
}

fn duration_ms(seconds: &Number) -> Result<u64, DecodeError> {
    if let Some(seconds) = seconds.as_u64() {
        return seconds.checked_mul(1000).ok_or(DecodeError::Malformed);
    }
    let millis = seconds.as_f64().ok_or(DecodeError::Malformed)? * 1000.0;
    // The exclusive upper bound avoids a saturating float-to-integer cast.
    if !(0.0..u64::MAX as f64).contains(&millis) {
        return Err(DecodeError::Malformed);
    }
    Ok(millis as u64)
}

fn decode_message(
    message: String,
    compressed: bool,
) -> Result<(DiagnosticPayload, Option<Encoding>), DecodeError> {
    let envelope = serde_json::from_str::<Value>(&message).ok();
    let is_envelope = envelope.as_ref().is_some_and(|value| {
        value.get("encoding").is_some() && value.get("data").is_some()
    });
    if !compressed && !is_envelope {
        return Ok((DiagnosticPayload::Text(message), None));
    }
    let envelope = envelope.ok_or(DecodeError::Malformed)?;
    let encoding = envelope
        .get("encoding")
        .and_then(Value::as_str)
        .ok_or(DecodeError::Malformed)?;
    if encoding != "gz+b64" {
        return Err(DecodeError::Undecodable);
    }
    let data = envelope
        .get("data")
        .and_then(Value::as_str)
        .ok_or(DecodeError::Malformed)?;
    Ok((decode_compressed(data)?, Some(Encoding::GzB64)))
}

fn decode_compressed(data: &str) -> Result<DiagnosticPayload, DecodeError> {
    let compact: Vec<_> = data
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let compressed = STANDARD
        .decode(compact)
        .map_err(|_| DecodeError::Undecodable)?;
    let mut bytes = Vec::new();
    // cloud-init labels zlib streams and line-wrapped base64 as `gz+b64`.
    let remaining = if compressed.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = GzDecoder::new(compressed.as_slice());
        decoder
            .read_to_end(&mut bytes)
            .map_err(|_| DecodeError::Undecodable)?;
        decoder.into_inner()
    } else {
        let mut decoder = ZlibDecoder::new(compressed.as_slice());
        decoder
            .read_to_end(&mut bytes)
            .map_err(|_| DecodeError::Undecodable)?;
        decoder.into_inner()
    };
    if !remaining.is_empty() {
        return Err(DecodeError::Undecodable);
    }
    Ok(DiagnosticPayload::Bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use rstest::rstest;
    use serde_json::json;
    use tempfile::TempDir;

    use super::super::diagnostic::{Entry, Kind, RawKeyValue};
    use super::super::encoding::decode_payload;
    use super::super::reader::DiagnosticReader;
    use crate::{KvpPool, KvpPoolStore, PoolMode};

    const VM_ID: &str = "0e5e179d-5341-478b-8456-fbb90621bdf8";
    const EVENT_ID: &str = "b7a822ba-4eea-46c0-b559-e84396101132";
    const TIMESTAMP: &str = "2026-07-27T23:33:24.339006+02:00";
    const ZLIB_DATA: &str = "eJxLzskvTdHNzMss4WL4DwAi1AUC";
    const GZIP_DATA: &str = "H4sIAAAAAAAC/0vOyS9N0c3MyyzhYvgPACZ1n10NAAAA";

    fn key(kind: &str, current: bool, index: Option<u64>) -> String {
        let identity = if current {
            format!("{VM_ID}|{EVENT_ID}")
        } else {
            EVENT_ID.to_owned()
        };
        let base = format!("CLOUD_INIT|100|{kind}|test|{identity}");
        match index {
            Some(index) => format!("{base}|{index}"),
            None => base,
        }
    }

    fn value(kind: &str, message: &str) -> Value {
        json!({"name": "test", "type": kind, "ts": TIMESTAMP, "msg": message})
    }

    fn chunk(kind: &str, index: u64, fragment: &str) -> String {
        format!(
            r#"{{"name":"test","type":"{kind}","ts":"{TIMESTAMP}","msg_i":{index},"msg":"{fragment}"}}"#
        )
    }

    fn entries(records: &[(String, String)]) -> Vec<Entry> {
        let dir = TempDir::new().unwrap();
        let store =
            KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe)
                .unwrap();
        store.append_multiple(records.iter().cloned()).unwrap();
        let before = fs::read(store.path()).unwrap();
        let entries = DiagnosticReader::new(store.clone()).entries().unwrap();
        assert_eq!(fs::read(store.path()).unwrap(), before);
        entries
    }

    fn only_diagnostic(mut entries: Vec<Entry>) -> Diagnostic {
        assert_eq!(entries.len(), 1);
        let mut diagnostic = None;
        if let Some(Entry::Diagnostic(value)) = entries.pop() {
            diagnostic = Some(value);
        }
        diagnostic.expect("expected a diagnostic")
    }

    fn assert_raw(records: &[(String, String)], error: DecodeError) {
        let expected: Vec<_> = records
            .iter()
            .map(|(key, value)| {
                Entry::Raw(RawKeyValue {
                    key: key.clone(),
                    value: value.clone(),
                    error: Some(error),
                })
            })
            .collect();
        assert_eq!(entries(records), expected);
    }

    #[rstest]
    #[case::old_single(false, None)]
    #[case::current_single(true, None)]
    #[case::old_chunk(false, Some(0))]
    #[case::current_chunk(true, Some(17))]
    fn key_layouts_identify_chunk_suffixes(
        #[case] current: bool,
        #[case] index: Option<u64>,
    ) {
        let key = key("event", current, index);
        let (base, parsed_index) = split_key(&key).unwrap();
        let parsed = parse_key(base).unwrap();
        assert_eq!(parsed_index, index);
        assert_eq!(parsed.vm_id, current.then_some(VM_ID));
        assert_eq!(parsed.event_id, EVENT_ID);
    }

    #[test]
    fn malformed_base_key_layout_is_rejected() {
        assert!(matches!(
            parse_key("CLOUD_INIT|100|event|test"),
            Err(DecodeError::Malformed)
        ));
    }

    #[rstest]
    #[case::layout("CLOUD_INIT|100|event".into())]
    #[case::vm(key("event", true, None).replace(VM_ID, "invalid"))]
    #[case::incarnation(key("event", false, None).replace("|100|", "|bad|"))]
    #[case::index(format!("{}|-1", key("event", true, None)))]
    fn malformed_cloud_keys_remain_raw(#[case] key: String) {
        assert_raw(
            &[(key, value("event", "message").to_string())],
            DecodeError::Malformed,
        );
    }

    #[rstest]
    #[case::older(false)]
    #[case::current(true)]
    fn source_identity_and_timestamp_are_mapped(#[case] current: bool) {
        let diagnostic = only_diagnostic(entries(&[(
            key("event", current, None),
            value("event", "message").to_string(),
        )]));
        assert_eq!(diagnostic.key().agent, "CLOUD_INIT");
        assert_eq!(diagnostic.key().vm_id.as_deref(), current.then_some(VM_ID));
        assert_eq!(diagnostic.key().name, "test");
        assert_eq!(diagnostic.key().event_id, EVENT_ID);
        let rendered = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(rendered["timestamp"], "2026-07-27T21:33:24.339Z");
        assert!(rendered.get("boot_epoch").is_none());
        assert!(rendered.get("diagnostic_version_id").is_none());
    }

    #[rstest]
    #[case::start("start", Kind::Start)]
    #[case::event("event", Kind::Event)]
    #[case::system_info("system-info", Kind::Event)]
    #[case::warning("warning", Kind::Event)]
    fn source_type_determines_timeline_kind(
        #[case] source: &str,
        #[case] kind: Kind,
    ) {
        let diagnostic = only_diagnostic(entries(&[(
            key(source, true, None),
            value(source, "observed").to_string(),
        )]));
        assert_eq!(diagnostic.kind(), kind);
        assert_eq!(
            diagnostic.payload(),
            &DiagnosticPayload::Text("observed".into())
        );
    }

    #[test]
    fn finish_closes_its_span_with_mapped_outcome_and_milliseconds() {
        let mut finish = value("finish", "finished with failure");
        finish["result"] = json!("FAIL");
        finish["duration"] = json!(0.1234);
        let entries = entries(&[
            (
                key("start", true, None),
                value("start", "starting").to_string(),
            ),
            (key("finish", true, None), finish.to_string()),
        ]);
        assert!(matches!(
            entries.as_slice(),
            [Entry::Diagnostic(Diagnostic::Start(start)), Entry::Diagnostic(Diagnostic::Finish(finish))]
                if start.key.event_id == finish.key.event_id
                    && finish.result == Outcome::Failure
                    && finish.duration_ms == 123
        ));
    }

    #[test]
    fn captured_successful_finish_decodes() {
        let records = [(
            "CLOUD_INIT|1785187982|finish|modules-final/config-scripts_user|0e5e179d-5341-478b-8456-fbb90621bdf8|e5f01809-a7a3-4279-aa64-1f18e21eda6e".into(),
            r#"{"name":"modules-final/config-scripts_user","type":"finish","ts":"2026-07-27T21:33:24.339006+00:00","result":"SUCCESS","duration":0.0006448590000012189,"msg":"config-scripts_user ran successfully and took 0.001 seconds"}"#.into(),
        )];
        let diagnostic = only_diagnostic(entries(&records));
        assert!(matches!(&diagnostic, Diagnostic::Finish(finish)
                if finish.result == Outcome::Success
                    && finish.duration_ms == 0
                    && finish.key.name == "modules-final/config-scripts_user"));
    }

    #[test]
    fn warn_finish_is_preserved_without_reclassifying_it_as_an_event() {
        let mut warning = value("finish", "completed with warnings");
        warning["result"] = json!("WARN");
        warning["duration"] = json!(1);
        let raw = RawKeyValue {
            key: key("finish", true, None),
            value: warning.to_string(),
            error: Some(DecodeError::Malformed),
        };
        let entries = entries(&[
            (
                key("start", true, None),
                value("start", "starting").to_string(),
            ),
            (raw.key.clone(), raw.value.clone()),
        ]);
        assert!(matches!(
            &entries[0],
            Entry::Diagnostic(Diagnostic::Start(_))
        ));
        assert_eq!(entries[1], Entry::Raw(raw));
    }

    #[rstest]
    #[case::result("result")]
    #[case::duration("duration")]
    fn finish_requires_result_and_duration(#[case] missing: &str) {
        let mut metadata = value("finish", "finished");
        metadata["result"] = json!("SUCCESS");
        metadata["duration"] = json!(1);
        metadata.as_object_mut().unwrap().remove(missing);
        assert_raw(
            &[(key("finish", true, None), metadata.to_string())],
            DecodeError::Malformed,
        );
    }

    #[rstest]
    #[case::invalid_json("event", "not json".into())]
    #[case::timestamp("event", value("event", "message").to_string().replace(TIMESTAMP, "bad"))]
    #[case::name("event", value("event", "message").to_string().replace("\"test\"", "\"other\""))]
    #[case::message("event", json!({"name":"test", "type":"event", "ts":TIMESTAMP, "msg":7}).to_string())]
    #[case::start_result("start", {
        let mut metadata = value("start", "message");
        metadata["result"] = json!("SUCCESS");
        metadata.to_string()
    })]
    #[case::start_duration("start", {
        let mut metadata = value("start", "message");
        metadata["duration"] = json!(1);
        metadata.to_string()
    })]
    fn malformed_source_values_remain_raw(
        #[case] kind: &str,
        #[case] value: String,
    ) {
        assert_raw(&[(key(kind, true, None), value)], DecodeError::Malformed);
    }

    #[test]
    fn chunk_without_a_key_index_is_malformed() {
        assert_raw(
            &[(key("event", false, None), chunk("event", 0, "partial"))],
            DecodeError::Malformed,
        );
    }

    #[rstest]
    #[case::newline(r#"line1\"#, "nline2", "line1\nline2")]
    #[case::unicode(r"\ud83", r"d\ude00", "😀")]
    fn escaped_fragments_are_unescaped_only_after_reassembly(
        #[case] first: &str,
        #[case] second: &str,
        #[case] expected: &str,
    ) {
        let first = chunk("event", 0, first);
        assert!(serde_json::from_str::<Value>(&first).is_err());
        let diagnostic = only_diagnostic(entries(&[
            (key("event", true, Some(1)), chunk("event", 1, second)),
            (key("event", true, Some(0)), first),
        ]));
        assert_eq!(
            diagnostic.payload(),
            &DiagnosticPayload::Text(expected.into())
        );
    }

    #[rstest]
    #[case::index(chunk("event", 0, "second"))]
    #[case::metadata(chunk("event", 1, "second").replace(TIMESTAMP, "2026-07-27T21:34:00Z"))]
    #[case::escape(chunk("event", 1, r"\x"))]
    fn invalid_chunk_values_preserve_the_entire_group(#[case] second: String) {
        assert_raw(
            &[
                (key("event", true, Some(1)), second),
                (key("event", true, Some(0)), chunk("event", 0, "first")),
            ],
            DecodeError::Malformed,
        );
    }

    #[rstest]
    #[case::gap(2, DecodeError::IncompleteGroup)]
    #[case::duplicate(0, DecodeError::DuplicateChunk)]
    fn cloud_groups_use_shared_index_validation(
        #[case] second: u64,
        #[case] error: DecodeError,
    ) {
        assert_raw(
            &[
                (key("event", false, Some(0)), chunk("event", 0, "first")),
                (
                    key("event", false, Some(second)),
                    chunk("event", second, "second"),
                ),
            ],
            error,
        );
    }

    #[test]
    fn incarnation_keeps_otherwise_identical_groups_separate() {
        let records = [
            (key("event", false, Some(0)), chunk("event", 0, "a")),
            (
                key("event", false, Some(0)).replace("|100|", "|101|"),
                chunk("event", 0, "x"),
            ),
            (key("event", false, Some(1)), chunk("event", 1, "b")),
            (
                key("event", false, Some(1)).replace("|100|", "|101|"),
                chunk("event", 1, "y"),
            ),
        ];
        let entries = entries(&records);
        assert!(matches!(
            entries.as_slice(),
            [Entry::Diagnostic(first), Entry::Diagnostic(second)]
                if first.key() == second.key()
                    && first.payload() == &DiagnosticPayload::Text("ab".into())
                    && second.payload() == &DiagnosticPayload::Text("xy".into())
        ));
    }

    #[test]
    fn mixed_sources_keep_first_seen_order_and_unrelated_raw_records() {
        let v1 = format!("DIAG_V1|azure-init|{VM_ID}|event|test|{EVENT_ID}|2026-07-27T21:33:00.000Z|none|||0");
        let entries = entries(&[
            (key("event", true, Some(1)), chunk("event", 1, "b")),
            ("unrelated".into(), "value".into()),
            (v1, "v1 message".into()),
            (key("event", true, Some(0)), chunk("event", 0, "a")),
        ]);
        assert_eq!(entries.len(), 3);
        assert!(
            matches!(&entries[0], Entry::Diagnostic(d) if d.key().agent == PREFIX)
        );
        assert!(
            matches!(&entries[1], Entry::Raw(raw) if raw.key == "unrelated" && raw.error.is_none())
        );
        assert!(
            matches!(&entries[2], Entry::Diagnostic(d) if d.key().agent == "azure-init")
        );
    }

    #[test]
    fn cloud_names_are_not_capped_by_writer_budgets() {
        let name = "long-subject".repeat(8);
        let mut metadata = value("system-info", "message");
        metadata["name"] = json!(name);
        let diagnostic = only_diagnostic(entries(&[(
            key("system-info", true, None)
                .replace("|test|", &format!("|{name}|")),
            metadata.to_string(),
        )]));
        assert_eq!(diagnostic.key().name, name);
    }

    #[rstest]
    #[case::whole_seconds(json!(2), 2000)]
    #[case::zero(json!(0), 0)]
    #[case::fraction(json!(0.1234), 123)]
    #[case::sub_millisecond(json!(0.00064), 0)]
    fn duration_conversion_truncates_to_milliseconds(
        #[case] seconds: Value,
        #[case] expected: u64,
    ) {
        assert_eq!(
            duration_ms(seconds.as_number().unwrap()).unwrap(),
            expected
        );
    }

    #[rstest]
    #[case::negative(json!(-1))]
    #[case::integer_overflow(json!(u64::MAX))]
    #[case::float_overflow(json!(1e30))]
    fn duration_conversion_rejects_invalid_ranges(#[case] seconds: Value) {
        assert_eq!(
            duration_ms(seconds.as_number().unwrap()),
            Err(DecodeError::Malformed)
        );
    }

    #[rstest]
    #[case::zlib(ZLIB_DATA)]
    #[case::gzip(GZIP_DATA)]
    fn compressed_envelopes_decode_arbitrary_bytes(#[case] data: &str) {
        let message = json!({"encoding": "gz+b64", "data": data}).to_string();
        let diagnostic = only_diagnostic(entries(&[(
            key("compressed", true, None),
            value("compressed", &message).to_string(),
        )]));
        assert_eq!(diagnostic.kind(), Kind::Event);
        assert_eq!(diagnostic.key().encoding, Some(Encoding::GzB64));
        assert_eq!(
            diagnostic.payload(),
            &DiagnosticPayload::Bytes(b"cloud-init\n\x00\xff".to_vec())
        );
    }

    #[test]
    fn all_truncated_zlib_prefixes_are_undecodable() {
        let compressed = STANDARD.decode(ZLIB_DATA).unwrap();
        for end in 0..compressed.len() {
            assert_eq!(
                (end, decode_compressed(&STANDARD.encode(&compressed[..end]))),
                (end, Err(DecodeError::Undecodable))
            );
        }
    }

    #[test]
    fn zlib_checksum_and_trailing_data_are_validated() {
        let compressed = STANDARD.decode(ZLIB_DATA).unwrap();
        let mut corrupt = compressed.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        let mut trailing = compressed;
        trailing.push(0);
        for bytes in [corrupt, trailing] {
            assert_eq!(
                decode_compressed(&STANDARD.encode(bytes)),
                Err(DecodeError::Undecodable)
            );
        }
    }

    #[test]
    fn compatibility_does_not_relax_v1_gzip_validation() {
        assert_eq!(
            decode_payload(ZLIB_DATA.as_bytes(), Some(&Encoding::GzB64)),
            Err(DecodeError::Undecodable)
        );
    }

    #[rstest]
    #[case::standalone_base64("b64")]
    #[case::unknown("zstd")]
    fn other_encoding_labels_are_not_supported(#[case] encoding: &str) {
        let message =
            json!({"encoding": encoding, "data": GZIP_DATA}).to_string();
        assert_raw(
            &[(
                key("compressed", true, None),
                value("compressed", &message).to_string(),
            )],
            DecodeError::Undecodable,
        );
    }

    #[test]
    fn compressed_envelope_requires_string_data() {
        let message = json!({"encoding": "gz+b64", "data": 7}).to_string();
        assert_raw(
            &[(
                key("compressed", true, None),
                value("compressed", &message).to_string(),
            )],
            DecodeError::Malformed,
        );
    }

    #[test]
    fn ordinary_json_messages_are_not_assumed_to_be_encoded() {
        let message = r#"{"encoding":"utf-8","description":"ordinary JSON"}"#;
        let diagnostic = only_diagnostic(entries(&[(
            key("event", false, None),
            value("event", message).to_string(),
        )]));
        assert_eq!(
            diagnostic.payload(),
            &DiagnosticPayload::Text(message.into())
        );
        assert_eq!(diagnostic.key().encoding, None);
    }
}
