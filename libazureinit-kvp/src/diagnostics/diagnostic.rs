// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::ProvisioningReport;

/// Prefix used for diagnostics emitted by
/// [`DiagnosticWriter`](crate::DiagnosticWriter).
pub const DIAGNOSTIC_VERSION_ID: &str = "DIAG";

/// Whether a diagnostic starts or finishes an operation, or records an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An operation began.
    Start,
    /// An operation ended.
    Finish,
    /// A standalone observation.
    Event,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Start => "start",
            Self::Finish => "finish",
            Self::Event => "event",
        })
    }
}

/// Encoding used to compress a diagnostic payload for storage.
///
/// Pass `None` to [`DiagnosticWriter`](crate::DiagnosticWriter) for plain text.
/// Prefer [`ZlibB64`](Self::ZlibB64) for telemetry consumed by Kusto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Zlib followed by standard base64 (`zlib+b64`).
    ZlibB64,
    /// Gzip followed by standard base64 (`gz+b64`). The reader also accepts
    /// cloud-init's zlib data under this label.
    GzB64,
    /// An unsupported encoding token; writers reject it.
    Other(String),
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZlibB64 => "zlib+b64",
            Self::GzB64 => "gz+b64",
            Self::Other(token) => token,
        })
    }
}

impl Serialize for Encoding {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

/// Reported result of an operation or observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The operation succeeded.
    Success,
    /// The operation failed, represented as `fail` in serialized output.
    #[serde(rename = "fail")]
    Failure,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Success => "success",
            Self::Failure => "fail",
        })
    }
}

/// The text or bytes carried by a diagnostic.
///
/// Strings convert to [`Text`](Self::Text); byte slices and vectors convert to
/// [`Bytes`](Self::Bytes). Reading a compressed payload always returns bytes,
/// even when its contents are valid text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticPayload {
    /// A UTF-8 message.
    Text(String),
    /// Binary content, serialized to JSON as a base64-encoded object.
    Bytes(Vec<u8>),
}

impl From<&str> for DiagnosticPayload {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for DiagnosticPayload {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&[u8]> for DiagnosticPayload {
    fn from(value: &[u8]) -> Self {
        Self::Bytes(value.to_vec())
    }
}

impl From<Vec<u8>> for DiagnosticPayload {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl Serialize for DiagnosticPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Text(text) => serializer.serialize_str(text),
            Self::Bytes(bytes) => {
                let mut payload =
                    serializer.serialize_struct("DiagnosticPayload", 3)?;
                payload.serialize_field("type", "bytes")?;
                payload.serialize_field("encoding", "base64")?;
                payload.serialize_field("data", &STANDARD.encode(bytes))?;
                payload.end()
            }
        }
    }
}

/// Why a recognized diagnostic or report could not be decoded.
///
/// The reader preserves the original record and attaches this error to
/// [`RawKeyValue::error`]. Failures to read the pool instead return
/// [`KvpError`](crate::KvpError).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeError {
    /// The record belongs to an unsupported diagnostic schema.
    UnsupportedVersion,
    /// Chunk indices are missing or do not start at zero.
    IncompleteGroup,
    /// More than one record uses the same chunk index.
    DuplicateChunk,
    /// The payload encoding is unsupported or its contents are invalid.
    Undecodable,
    /// Diagnostic or report metadata does not match its format.
    Malformed,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedVersion => "unsupported diagnostic schema version",
            Self::IncompleteGroup => {
                "diagnostic chunks are not contiguous from index 0"
            }
            Self::DuplicateChunk => "duplicate diagnostic chunk index",
            Self::Undecodable => "diagnostic payload could not be decoded",
            Self::Malformed => "malformed diagnostic or provisioning report",
        })
    }
}

impl std::error::Error for DecodeError {}

/// Producer, identity and timestamp shared by all diagnostic kinds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticKey {
    /// Reporting agent, conventionally `name/VERSION`.
    pub agent: String,
    /// VM UUID; `None` for older cloud-init records.
    pub vm_id: Option<String>,
    /// Operation or observation name, such as `provision:run` or `dmesg`.
    pub name: String,
    /// UTF-8 identifier, typically UUID, shared by an operation's start and finish;
    /// unique for a standalone event.
    pub event_id: String,
    /// When this diagnostic was emitted, in UTC.
    #[serde(serialize_with = "serialize_timestamp")]
    pub timestamp: DateTime<Utc>,
    /// Stored payload encoding; `None` means UTF-8-encoded plain text.
    #[serde(serialize_with = "serialize_encoding")]
    pub encoding: Option<Encoding>,
}

/// The beginning of an operation whose finish uses the same event ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticStart {
    /// Producer, identity and time of the start.
    #[serde(flatten)]
    pub key: DiagnosticKey,
    /// Message or artifact associated with the start.
    pub payload: DiagnosticPayload,
}

/// The end of an operation, including its outcome and elapsed time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticFinish {
    /// Producer, identity and time of the finish.
    #[serde(flatten)]
    pub key: DiagnosticKey,
    /// Message or artifact associated with the finish.
    pub payload: DiagnosticPayload,
    /// Reported outcome of the operation.
    pub result: Outcome,
    /// Elapsed time, serialized to JSON as a number of seconds.
    #[serde(
        rename = "duration",
        serialize_with = "serialize_duration_seconds"
    )]
    pub duration: Duration,
}

/// A standalone observation, optionally with an outcome or elapsed time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticEvent {
    /// Producer, identity and time of the observation.
    #[serde(flatten)]
    pub key: DiagnosticKey,
    /// Message or artifact captured by the event.
    pub payload: DiagnosticPayload,
    /// Reported outcome, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Outcome>,
    /// Elapsed time if measured, serialized to JSON as a number of seconds.
    #[serde(
        rename = "duration",
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_opt_duration_seconds"
    )]
    pub duration: Option<Duration>,
}

/// A diagnostic record with its metadata and decoded payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Diagnostic {
    /// An operation began.
    Start(DiagnosticStart),
    /// An operation ended with a reported outcome and duration.
    Finish(DiagnosticFinish),
    /// A standalone observation.
    Event(DiagnosticEvent),
}

impl Diagnostic {
    /// Returns the metadata shared by all diagnostic kinds.
    pub fn key(&self) -> &DiagnosticKey {
        match self {
            Self::Start(start) => &start.key,
            Self::Finish(finish) => &finish.key,
            Self::Event(event) => &event.key,
        }
    }

    /// Returns whether this is a start, finish or standalone event.
    pub fn kind(&self) -> Kind {
        match self {
            Self::Start(_) => Kind::Start,
            Self::Finish(_) => Kind::Finish,
            Self::Event(_) => Kind::Event,
        }
    }

    /// Returns the decoded payload; encoded data has already been decompressed.
    pub fn payload(&self) -> &DiagnosticPayload {
        match self {
            Self::Start(start) => &start.payload,
            Self::Finish(finish) => &finish.payload,
            Self::Event(event) => &event.payload,
        }
    }
}

/// A stored record that was not decoded as a diagnostic or report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RawKeyValue {
    /// Original key.
    pub key: String,
    /// Original value, without payload decoding.
    pub value: String,
    /// Why a recognized record could not be decoded; `None` for unknown or
    /// non-diagnostic key/value pairs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DecodeError>,
}

/// One item returned by [`DiagnosticReader::entries`](crate::DiagnosticReader::entries).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type")]
pub enum Entry {
    /// A decoded diagnostic.
    #[serde(rename = "diagnostic")]
    Diagnostic(Diagnostic),
    /// A provisioning health report.
    #[serde(rename = "PROVISIONING_REPORT")]
    Report(ProvisioningReport),
    /// An unrelated or invalid record, preserved without interpretation.
    #[serde(rename = "raw")]
    Raw(RawKeyValue),
}

fn serialize_timestamp<S>(
    timestamp: &DateTime<Utc>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer
        .serialize_str(&timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

fn serialize_encoding<S>(
    encoding: &Option<Encoding>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match encoding {
        Some(encoding) => encoding.serialize(serializer),
        None => serializer.serialize_str("none"),
    }
}

fn serialize_duration_seconds<S>(
    duration: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_f64(duration.as_secs_f64())
}

fn serialize_opt_duration_seconds<S>(
    duration: &Option<Duration>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    duration
        .as_ref()
        .map(Duration::as_secs_f64)
        .serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReportPpsType;
    use rstest::rstest;
    use serde_json::{json, Value};

    const AGENT: &str = "azure-init/0.1.1";
    const VM_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    const EVENT_ID: &str = "8f3e9c4a-1b2c-4d5e-9f01-234567890abc";
    const TIMESTAMP: &str = "2026-08-31T12:34:56.789Z";

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

    fn expected_diagnostic(kind: &str, payload: Value) -> Value {
        json!({
            "type": "diagnostic",
            "kind": kind,
            "agent": AGENT,
            "vm_id": VM_ID,
            "name": "provision:run",
            "event_id": EVENT_ID,
            "timestamp": TIMESTAMP,
            "encoding": "none",
            "payload": payload,
        })
    }

    #[rstest]
    #[case(Kind::Start, "start")]
    #[case(Kind::Finish, "finish")]
    #[case(Kind::Event, "event")]
    fn kind_uses_wire_token(#[case] kind: Kind, #[case] token: &str) {
        assert_eq!(kind.to_string(), token);
        assert_eq!(serde_json::to_value(kind).unwrap(), token);
    }

    #[rstest]
    #[case(Encoding::ZlibB64, "zlib+b64")]
    #[case(Encoding::GzB64, "gz+b64")]
    #[case(Encoding::Other("zstd+b64".into()), "zstd+b64")]
    #[case(Encoding::Other("".into()), "")]
    fn encoding_preserves_token(
        #[case] encoding: Encoding,
        #[case] token: &str,
    ) {
        assert_eq!(encoding.to_string(), token);
        assert_eq!(serde_json::to_value(encoding).unwrap(), token);
    }

    #[rstest]
    #[case(Outcome::Success, "success")]
    #[case(Outcome::Failure, "fail")]
    fn outcome_uses_wire_token(#[case] result: Outcome, #[case] token: &str) {
        assert_eq!(result.to_string(), token);
        assert_eq!(serde_json::to_value(result).unwrap(), token);
    }

    #[rstest]
    #[case(DecodeError::UnsupportedVersion, "unsupported_version")]
    #[case(DecodeError::IncompleteGroup, "incomplete_group")]
    #[case(DecodeError::DuplicateChunk, "duplicate_chunk")]
    #[case(DecodeError::Undecodable, "undecodable")]
    #[case(DecodeError::Malformed, "malformed")]
    fn decode_error_has_serializable_reason(
        #[case] reason: DecodeError,
        #[case] token: &str,
    ) {
        assert_eq!(serde_json::to_value(reason).unwrap(), token);
        let error: &dyn std::error::Error = &reason;
        assert!(!error.to_string().is_empty());
        assert!(error.source().is_none());
    }

    #[test]
    fn string_conversions_preserve_text() {
        let text = "héllo\n\"world\"\u{0}";
        let expected = DiagnosticPayload::Text(text.to_owned());
        assert_eq!(DiagnosticPayload::from(text), expected);
        assert_eq!(DiagnosticPayload::from(text.to_owned()), expected);
    }

    #[rstest]
    #[case(b"")]
    #[case(b"hello")]
    #[case(&[0, 255, 128, 0])]
    fn byte_inputs_remain_bytes(#[case] bytes: &[u8]) {
        let expected = DiagnosticPayload::Bytes(bytes.to_vec());
        assert_eq!(DiagnosticPayload::from(bytes), expected);
        assert_eq!(DiagnosticPayload::from(bytes.to_vec()), expected);
    }

    #[rstest]
    #[case("")]
    #[case("héllo\n\"world\"\u{0}")]
    fn text_serializes_as_a_string(#[case] text: &str) {
        let payload = DiagnosticPayload::Text(text.to_owned());
        assert_eq!(serde_json::to_value(payload).unwrap(), json!(text));
    }

    #[rstest]
    #[case(b"", "")]
    #[case(b"hello", "aGVsbG8=")]
    #[case(&[0], "AA==")]
    #[case(&[251, 255], "+/8=")]
    fn bytes_serialize_as_standard_base64(
        #[case] bytes: &[u8],
        #[case] encoded: &str,
    ) {
        assert_eq!(
            serde_json::to_value(DiagnosticPayload::Bytes(bytes.to_vec()))
                .unwrap(),
            json!({"type": "bytes", "encoding": "base64", "data": encoded})
        );
    }

    #[test]
    fn start_serializes_without_result_or_duration() {
        let entry = Entry::Diagnostic(Diagnostic::Start(DiagnosticStart {
            key: key(),
            payload: "starting".into(),
        }));
        assert_eq!(
            serde_json::to_value(entry).unwrap(),
            expected_diagnostic("start", json!("starting"))
        );
    }

    #[rstest]
    #[case(Outcome::Success, "success", Duration::from_micros(312), 0.000312)]
    #[case(Outcome::Failure, "fail", Duration::ZERO, 0.0)]
    #[case(Outcome::Success, "success", Duration::MAX, u64::MAX as f64)]
    fn finish_serializes_result_and_seconds(
        #[case] result: Outcome,
        #[case] token: &str,
        #[case] duration: Duration,
        #[case] seconds: f64,
    ) {
        let entry = Entry::Diagnostic(Diagnostic::Finish(DiagnosticFinish {
            key: key(),
            payload: "finished".into(),
            result,
            duration,
        }));
        let mut expected = expected_diagnostic("finish", json!("finished"));
        expected["result"] = json!(token);
        expected["duration"] = json!(seconds);
        assert_eq!(serde_json::to_value(entry).unwrap(), expected);
    }

    #[rstest]
    #[case::neither(None, None)]
    #[case::result_only(Some(Outcome::Success), None)]
    #[case::zero_duration(None, Some(0))]
    #[case::both(Some(Outcome::Failure), Some(52))]
    fn event_serializes_only_measured_fields(
        #[case] result: Option<Outcome>,
        #[case] duration_us: Option<u64>,
    ) {
        let entry = Entry::Diagnostic(Diagnostic::Event(DiagnosticEvent {
            key: key(),
            payload: "observed".into(),
            result,
            duration: duration_us.map(Duration::from_micros),
        }));
        let mut expected = expected_diagnostic("event", json!("observed"));
        if let Some(result) = result {
            expected["result"] = json!(result.to_string());
        }
        if let Some(duration_us) = duration_us {
            expected["duration"] = json!(duration_us as f64 / 1_000_000.0);
        }
        assert_eq!(serde_json::to_value(entry).unwrap(), expected);
    }

    #[test]
    fn wire_encoding_is_distinct_from_payload_presentation() {
        let entry = Entry::Diagnostic(Diagnostic::Event(DiagnosticEvent {
            key: DiagnosticKey {
                encoding: Some(Encoding::GzB64),
                ..key()
            },
            payload: b"hello".as_slice().into(),
            result: None,
            duration: None,
        }));
        let mut expected = expected_diagnostic(
            "event",
            json!({
                "type": "bytes",
                "encoding": "base64",
                "data": "aGVsbG8=",
            }),
        );
        expected["encoding"] = json!("gz+b64");
        assert_eq!(serde_json::to_value(entry).unwrap(), expected);
    }

    #[rstest]
    #[case(None, "none")]
    #[case(Some(Encoding::ZlibB64), "zlib+b64")]
    #[case(Some(Encoding::GzB64), "gz+b64")]
    #[case(Some(Encoding::Other("zstd+b64".into())), "zstd+b64")]
    fn key_always_serializes_encoding(
        #[case] encoding: Option<Encoding>,
        #[case] token: &str,
    ) {
        let key = DiagnosticKey { encoding, ..key() };
        assert_eq!(serde_json::to_value(key).unwrap()["encoding"], token);
    }

    #[test]
    fn missing_cloud_init_vm_id_serializes_as_null() {
        let entry = Entry::Diagnostic(Diagnostic::Start(DiagnosticStart {
            key: DiagnosticKey {
                agent: "CLOUD_INIT".into(),
                vm_id: None,
                ..key()
            },
            payload: "starting".into(),
        }));
        let mut expected = expected_diagnostic("start", json!("starting"));
        expected["agent"] = json!("CLOUD_INIT");
        expected["vm_id"] = Value::Null;
        assert_eq!(serde_json::to_value(entry).unwrap(), expected);
    }

    #[rstest]
    #[case("2026-08-31T12:34:56Z", "2026-08-31T12:34:56Z")]
    #[case("2026-08-31T12:34:56.3Z", "2026-08-31T12:34:56.300Z")]
    #[case("2026-08-31T12:34:56.789999Z", "2026-08-31T12:34:56.789999Z")]
    #[case("2026-08-31T12:34:56.789123456Z", "2026-08-31T12:34:56.789123456Z")]
    #[case("2026-08-31T14:34:56.789+02:00", TIMESTAMP)]
    fn timestamp_serializes_in_utc_without_padding(
        #[case] timestamp: &str,
        #[case] expected: &str,
    ) {
        let key = DiagnosticKey {
            timestamp: DateTime::parse_from_rfc3339(timestamp)
                .unwrap()
                .with_timezone(&Utc),
            ..key()
        };
        assert_eq!(serde_json::to_value(key).unwrap()["timestamp"], expected);
    }

    #[rstest]
    fn diagnostic_accessors_cover_all_kinds(
        #[values(Kind::Start, Kind::Finish, Kind::Event)] kind: Kind,
    ) {
        let key = key();
        let payload = DiagnosticPayload::from("message");
        let diagnostic = match kind {
            Kind::Start => Diagnostic::Start(DiagnosticStart {
                key: key.clone(),
                payload: payload.clone(),
            }),
            Kind::Finish => Diagnostic::Finish(DiagnosticFinish {
                key: key.clone(),
                payload: payload.clone(),
                result: Outcome::Success,
                duration: Duration::ZERO,
            }),
            Kind::Event => Diagnostic::Event(DiagnosticEvent {
                key: key.clone(),
                payload: payload.clone(),
                result: None,
                duration: None,
            }),
        };
        assert_eq!(diagnostic.key(), &key);
        assert_eq!(diagnostic.kind(), kind);
        assert_eq!(diagnostic.payload(), &payload);
    }

    #[rstest]
    #[case(None, None)]
    #[case(Some(DecodeError::Malformed), Some("malformed"))]
    fn raw_entry_preserves_key_value_and_optional_error(
        #[case] error: Option<DecodeError>,
        #[case] token: Option<&str>,
    ) {
        let entry = Entry::Raw(RawKeyValue {
            key: "original|key|0".into(),
            value: "original \"value\"\nwith Unicode: é".into(),
            error,
        });
        let mut expected = json!({
            "type": "raw",
            "key": "original|key|0",
            "value": "original \"value\"\nwith Unicode: é",
        });
        if let Some(token) = token {
            expected["error"] = json!(token);
        }
        assert_eq!(serde_json::to_value(entry).unwrap(), expected);
    }

    #[test]
    fn report_entry_adds_type_without_nesting_report_fields() {
        let report =
            ProvisioningReport::success(AGENT, VM_ID, ReportPpsType::None);
        let mut expected = serde_json::to_value(&report).unwrap();
        expected["type"] = json!("PROVISIONING_REPORT");
        assert_eq!(
            serde_json::to_value(Entry::Report(report)).unwrap(),
            expected
        );
    }
}
