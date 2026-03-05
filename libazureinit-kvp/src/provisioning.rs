// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed accessor for the `PROVISIONING_REPORT` KVP key.
//!
//! The Azure platform reads this key to determine provisioning
//! outcome. The value is a pipe-delimited string of `key=value`
//! segments produced by the `csv` crate for quoting compatibility.

use std::fmt;
use std::io;

use chrono::{DateTime, Utc};

use crate::KvpStore;

const DEFAULT_AGENT: &str = concat!("Azure-Init/", env!("CARGO_PKG_VERSION"));

/// A structured provisioning report entry.
///
/// - `result`: Outcome string (e.g. "success", "error").
/// - `agent`: Agent identifier (e.g. "Azure-Init/0.1.1").
/// - `pps_type`: PPS type (e.g. "None").
/// - `vm_id`: VM identifier.
/// - `timestamp`: When the report was generated.
/// - `extra`: Optional additional key-value pairs (e.g. origin,
///   reason, documentation_url).
pub struct ProvisioningReport {
    pub result: String,
    pub agent: String,
    pub pps_type: String,
    pub vm_id: String,
    pub timestamp: DateTime<Utc>,
    pub extra: Vec<(String, String)>,
}

impl ProvisioningReport {
    /// Create a success report.
    ///
    /// Sets `result` to `"success"`, `pps_type` to `"None"`, and
    /// `agent` to the crate default. Override any field after
    /// construction if needed (all fields are public).
    pub fn success(vm_id: &str) -> Self {
        Self {
            result: "success".to_string(),
            agent: DEFAULT_AGENT.to_string(),
            pps_type: "None".to_string(),
            vm_id: vm_id.to_string(),
            timestamp: Utc::now(),
            extra: Vec::new(),
        }
    }

    /// Create a failure/error report.
    ///
    /// Sets `result` to `"error"` and stores `reason` in the `extra`
    /// field.
    pub fn error(vm_id: &str, reason: &str) -> Self {
        Self {
            result: "error".to_string(),
            agent: DEFAULT_AGENT.to_string(),
            pps_type: "None".to_string(),
            vm_id: vm_id.to_string(),
            timestamp: Utc::now(),
            extra: vec![("reason".to_string(), reason.to_string())],
        }
    }

    /// Encode as a pipe-delimited string for KVP storage.
    ///
    /// Produces the same wire format as the existing `encode_report()`
    /// in `health.rs`: each segment is a `key=value` string, joined
    /// by `|` via the `csv` crate with `QuoteStyle::Necessary`.
    pub fn encode(&self) -> String {
        let mut fields = vec![
            format!("result={}", self.result),
            format!("agent={}", self.agent),
            format!("pps_type={}", self.pps_type),
            format!("vm_id={}", self.vm_id),
            format!("timestamp={}", self.timestamp.to_rfc3339()),
        ];
        for (k, v) in &self.extra {
            fields.push(format!("{k}={v}"));
        }
        encode_report(&fields)
    }

    /// Parse a pipe-delimited string back into a report.
    pub fn decode(s: &str) -> io::Result<Self> {
        let segments = decode_report(s)?;

        let mut result = None;
        let mut agent = None;
        let mut pps_type = None;
        let mut vm_id = None;
        let mut timestamp = None;
        let mut extra = Vec::new();

        for segment in &segments {
            if let Some((key, value)) = segment.split_once('=') {
                match key {
                    "result" => result = Some(value.to_string()),
                    "agent" => agent = Some(value.to_string()),
                    "pps_type" => pps_type = Some(value.to_string()),
                    "vm_id" => vm_id = Some(value.to_string()),
                    "timestamp" => {
                        timestamp = Some(
                            DateTime::parse_from_rfc3339(value)
                                .map(|dt| dt.with_timezone(&Utc))
                                .map_err(|e| {
                                    io::Error::other(format!(
                                        "invalid timestamp: {e}"
                                    ))
                                })?,
                        );
                    }
                    _ => {
                        extra.push((key.to_string(), value.to_string()));
                    }
                }
            }
        }

        Ok(Self {
            result: result.unwrap_or_default(),
            agent: agent.unwrap_or_default(),
            pps_type: pps_type.unwrap_or_default(),
            vm_id: vm_id.unwrap_or_default(),
            timestamp: timestamp.unwrap_or_else(Utc::now),
            extra,
        })
    }

    /// Write this report to the store (key = `"PROVISIONING_REPORT"`).
    pub fn write_to(&self, store: &impl KvpStore) -> io::Result<()> {
        store.write("PROVISIONING_REPORT", &self.encode())
    }

    /// Read and parse a provisioning report from the store, if present.
    pub fn read_from(store: &impl KvpStore) -> io::Result<Option<Self>> {
        store
            .read("PROVISIONING_REPORT")
            .map(|opt| opt.and_then(|s| Self::decode(&s).ok()))
    }
}

impl fmt::Display for ProvisioningReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.encode())
    }
}

/// Encode a slice of strings as a single pipe-delimited record using
/// the `csv` crate, matching the existing `encode_report()` wire
/// format.
fn encode_report(fields: &[String]) -> String {
    let mut wtr = csv::WriterBuilder::new()
        .delimiter(b'|')
        .quote_style(csv::QuoteStyle::Necessary)
        .from_writer(vec![]);
    wtr.write_record(fields).expect("CSV write failed");
    let mut bytes = wtr.into_inner().unwrap();
    if let Some(b'\n') = bytes.last() {
        bytes.pop();
    }
    if let Some(b'\r') = bytes.last() {
        bytes.pop();
    }
    String::from_utf8(bytes).expect("CSV was not utf-8")
}

/// Decode a pipe-delimited record back into individual segments.
fn decode_report(s: &str) -> io::Result<Vec<String>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(b'|')
        .has_headers(false)
        .from_reader(s.as_bytes());

    let record = rdr.records().next().ok_or_else(|| {
        io::Error::other("empty provisioning report string")
    })??;

    Ok(record.iter().map(|s| s.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryKvpStore;

    #[test]
    fn test_success_report_encode() {
        let report = ProvisioningReport::success("vm-123");
        let encoded = report.encode();

        assert!(encoded.contains("result=success"));
        assert!(encoded.contains("agent=Azure-Init/"));
        assert!(encoded.contains("pps_type=None"));
        assert!(encoded.contains("vm_id=vm-123"));
        assert!(encoded.contains("timestamp="));
        assert!(encoded.contains("|"));
        assert!(!encoded.contains('\n'));
    }

    #[test]
    fn test_error_report_encode() {
        let report = ProvisioningReport::error("vm-456", "disk full");
        let encoded = report.encode();

        assert!(encoded.contains("result=error"));
        assert!(encoded.contains("vm_id=vm-456"));
        assert!(encoded.contains("reason=disk full"));
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let mut report = ProvisioningReport::success("vm-abc");
        report
            .extra
            .push(("origin".to_string(), "test".to_string()));

        let encoded = report.encode();
        let decoded =
            ProvisioningReport::decode(&encoded).expect("decode failed");

        assert_eq!(decoded.result, "success");
        assert_eq!(decoded.vm_id, "vm-abc");
        assert_eq!(decoded.pps_type, "None");
        assert_eq!(
            decoded.extra,
            vec![("origin".to_string(), "test".to_string())]
        );
    }

    #[test]
    fn test_error_roundtrip() {
        let report = ProvisioningReport::error("vm-err", "timeout");
        let encoded = report.encode();
        let decoded =
            ProvisioningReport::decode(&encoded).expect("decode failed");

        assert_eq!(decoded.result, "error");
        assert_eq!(decoded.vm_id, "vm-err");
        assert_eq!(
            decoded.extra,
            vec![("reason".to_string(), "timeout".to_string())]
        );
    }

    #[test]
    fn test_write_to_and_read_from_store() {
        let store = InMemoryKvpStore::default();

        let report = ProvisioningReport::success("vm-store");
        report.write_to(&store).unwrap();

        let read_back = ProvisioningReport::read_from(&store).unwrap().unwrap();
        assert_eq!(read_back.result, "success");
        assert_eq!(read_back.vm_id, "vm-store");
    }

    #[test]
    fn test_read_from_empty_store() {
        let store = InMemoryKvpStore::default();
        let result = ProvisioningReport::read_from(&store).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_display_delegates_to_encode() {
        let report = ProvisioningReport::success("vm-display");
        assert_eq!(format!("{report}"), report.encode());
    }

    #[test]
    fn test_extra_fields_preserved() {
        let mut report = ProvisioningReport::error("vm-x", "reason1");
        report.extra.push((
            "documentation_url".to_string(),
            "https://example.com".to_string(),
        ));

        let encoded = report.encode();
        let decoded =
            ProvisioningReport::decode(&encoded).expect("decode failed");

        assert_eq!(decoded.extra.len(), 2);
        assert!(decoded
            .extra
            .iter()
            .any(|(k, v)| k == "reason" && v == "reason1"));
        assert!(decoded.extra.iter().any(
            |(k, v)| k == "documentation_url" && v == "https://example.com"
        ));
    }

    #[test]
    fn test_wire_format_matches_existing_encode_report() {
        // Verify our encoding matches what health.rs encode_report()
        // produces: pipe-delimited, csv-quoted, no trailing newline.
        let fields = vec![
            "result=success".to_string(),
            "agent=Azure-Init/0.1.0".to_string(),
            "pps_type=None".to_string(),
        ];
        let encoded = encode_report(&fields);
        assert_eq!(
            encoded,
            "result=success|agent=Azure-Init/0.1.0|pps_type=None"
        );
    }
}
