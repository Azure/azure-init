// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::ProvisioningReport;

pub const DIAGNOSTIC_VERSION_ID: &str = "DIAG_V1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Start,
    Finish,
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

/// Unencoded text uses `None` in `DiagnosticKey::encoding`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Encoding {
    GzB64,
    Other(String),
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,
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

/// Decoded bytes retain their type even when they contain valid UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticPayload {
    Text(String),
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

/// Describes uninterpretable stored data, not a failed I/O operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeError {
    UnsupportedVersion,
    IncompleteGroup,
    DuplicateChunk,
    Undecodable,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticKey {
    pub agent: String,
    /// Older cloud-init records do not include a VM identity.
    pub vm_id: Option<String>,
    pub name: String,
    /// Span endpoints share this ID; standalone events have their own.
    pub event_id: String,
    #[serde(serialize_with = "serialize_timestamp")]
    pub timestamp: DateTime<Utc>,
    #[serde(serialize_with = "serialize_encoding")]
    pub encoding: Option<Encoding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticStart {
    #[serde(flatten)]
    pub key: DiagnosticKey,
    pub payload: DiagnosticPayload,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticFinish {
    #[serde(flatten)]
    pub key: DiagnosticKey,
    pub payload: DiagnosticPayload,
    pub result: Outcome,
    #[serde(rename = "duration", serialize_with = "serialize_duration_us")]
    pub duration: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticEvent {
    #[serde(flatten)]
    pub key: DiagnosticKey,
    pub payload: DiagnosticPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Outcome>,
    #[serde(
        rename = "duration",
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_opt_duration_us"
    )]
    pub duration: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Diagnostic {
    Start(DiagnosticStart),
    Finish(DiagnosticFinish),
    Event(DiagnosticEvent),
}

impl Diagnostic {
    pub fn key(&self) -> &DiagnosticKey {
        match self {
            Self::Start(start) => &start.key,
            Self::Finish(finish) => &finish.key,
            Self::Event(event) => &event.key,
        }
    }

    pub fn kind(&self) -> Kind {
        match self {
            Self::Start(_) => Kind::Start,
            Self::Finish(_) => Kind::Finish,
            Self::Event(_) => Kind::Event,
        }
    }

    pub fn payload(&self) -> &DiagnosticPayload {
        match self {
            Self::Start(start) => &start.payload,
            Self::Finish(finish) => &finish.payload,
            Self::Event(event) => &event.payload,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RawKeyValue {
    pub key: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DecodeError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type")]
pub enum Entry {
    #[serde(rename = "diagnostic")]
    Diagnostic(Diagnostic),
    #[serde(rename = "PROVISIONING_REPORT")]
    Report(ProvisioningReport),
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

/// DIAG_V1 stores elapsed time as integer microseconds.
fn duration_micros(duration: &Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn serialize_duration_us<S>(
    duration: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u64(duration_micros(duration))
}

fn serialize_opt_duration_us<S>(
    duration: &Option<Duration>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    duration.as_ref().map(duration_micros).serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReportPpsType;
    use rstest::rstest;
    use serde_json::{json, Value};

    const AGENT: &str = "azure-init-0.1.1";
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
    #[case(Outcome::Success, "success", 312)]
    #[case(Outcome::Failure, "fail", 0)]
    #[case(Outcome::Success, "success", u64::MAX)]
    fn finish_serializes_result_and_microseconds(
        #[case] result: Outcome,
        #[case] token: &str,
        #[case] duration_us: u64,
    ) {
        let entry = Entry::Diagnostic(Diagnostic::Finish(DiagnosticFinish {
            key: key(),
            payload: "finished".into(),
            result,
            duration: Duration::from_micros(duration_us),
        }));
        let mut expected = expected_diagnostic("finish", json!("finished"));
        expected["result"] = json!(token);
        expected["duration"] = json!(duration_us);
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
            expected["duration"] = json!(duration_us);
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
