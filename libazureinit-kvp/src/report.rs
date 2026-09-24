// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Provisioning report creation and storage.

use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::{DecodeError, KvpError, KvpPoolStore};

/// Key used by [`write_report`] to store the provisioning result.
pub const PROVISIONING_REPORT_KEY: &str = "PROVISIONING_REPORT";

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// Outcome of a provisioning attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum ReportResult {
    /// Provisioning completed successfully.
    Success,
    /// Provisioning failed.
    Error,
}

impl ReportResult {
    /// The wire string used in the `result` KVP field.
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for ReportResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Pre-provisioning state included in a provisioning report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum ReportPpsType {
    /// Not pre-provisioned.
    None,
    /// A pre-provisioned OS disk.
    #[serde(rename = "PreprovisionedOSDisk")]
    OsDisk,
    /// Pre-provisioning is running.
    Running,
    /// Pre-provisioning can be saved.
    Savable,
    /// The pre-provisioning state is unknown.
    Unknown,
}

impl ReportPpsType {
    fn from_wire(value: &str) -> Result<Self, DecodeError> {
        match value {
            "None" => Ok(Self::None),
            "PreprovisionedOSDisk" => Ok(Self::OsDisk),
            "Running" => Ok(Self::Running),
            "Savable" => Ok(Self::Savable),
            "Unknown" => Ok(Self::Unknown),
            _ => Err(DecodeError::Malformed),
        }
    }

    /// The wire string used in the `pps_type` KVP field.
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::OsDisk => "PreprovisionedOSDisk",
            Self::Running => "Running",
            Self::Savable => "Savable",
            Self::Unknown => "Unknown",
        }
    }
}

impl std::fmt::Display for ReportPpsType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A provisioning result for host telemetry.
///
/// [`success`](Self::success) and [`failure`](Self::failure) capture the current
/// time. Add optional context with the builder methods, then call
/// [`write_report`] to persist it. Existing stored reports can be parsed with
/// [`str::parse`]; parsing preserves their timestamps.
///
/// See the [provisioning report contract] for the stored format.
///
/// [provisioning report contract]: https://github.com/Azure/azure-init/blob/main/doc/diagnostics.md#provisioning-reports
///
/// # Example
/// ```no_run
/// use libazureinit_kvp::{
///     write_report, KvpPool, KvpPoolStore, PoolMode, ProvisioningReport,
///     ReportPpsType,
/// };
///
/// # fn main() -> Result<(), libazureinit_kvp::KvpError> {
/// let store = KvpPoolStore::new(KvpPool::Guest, PoolMode::Safe)?;
///
/// let report = ProvisioningReport::success(
///     format!("Azure-Init/{}", env!("CARGO_PKG_VERSION")),
///     "00000000-0000-0000-0000-000000000abc",
///     ReportPpsType::None,
/// )
/// .with_extra("build", "test-123");
///
/// write_report(&store, &report)?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ProvisioningReport {
    /// Provisioning outcome.
    result: ReportResult,
    /// Reporting agent identifier.
    agent: String,
    /// Virtual machine identifier.
    vm_id: String,
    /// RFC 3339 timestamp captured when the report is constructed.
    timestamp: String,
    /// Pre-provisioning state.
    pps_type: ReportPpsType,
    /// Failure reason, present only for error reports.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    /// Help URL, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    documentation_url: Option<String>,
    /// Additional ordered key/value context.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    extra: Vec<(String, String)>,
}

impl ProvisioningReport {
    /// Creates a successful provisioning report.
    pub fn success(
        agent: impl Into<String>,
        vm_id: impl Into<String>,
        pps_type: ReportPpsType,
    ) -> Self {
        Self {
            result: ReportResult::Success,
            agent: agent.into(),
            vm_id: vm_id.into(),
            timestamp: now_rfc3339(),
            pps_type,
            reason: None,
            documentation_url: None,
            extra: Vec::new(),
        }
    }

    /// Creates a failed provisioning report with a reason.
    pub fn failure(
        agent: impl Into<String>,
        vm_id: impl Into<String>,
        reason: impl Into<String>,
        pps_type: ReportPpsType,
    ) -> Self {
        Self {
            result: ReportResult::Error,
            agent: agent.into(),
            vm_id: vm_id.into(),
            timestamp: now_rfc3339(),
            pps_type,
            reason: Some(reason.into()),
            documentation_url: None,
            extra: Vec::new(),
        }
    }

    /// Sets a help URL, included in the stored value for failure reports only.
    pub fn with_documentation_url(mut self, url: impl Into<String>) -> Self {
        self.documentation_url = Some(url.into());
        self
    }

    /// Adds context to the report.
    ///
    /// Entries retain insertion order and duplicate keys. Do not use standard
    /// report field names such as `result` or `timestamp`.
    pub fn with_extra(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.extra.push((key.into(), value.into()));
        self
    }
}

impl FromStr for ProvisioningReport {
    type Err = DecodeError;

    /// Parses a stored report without changing its timestamp.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value
            .strip_suffix("\r\n")
            .or_else(|| value.strip_suffix('\n'))
            .unwrap_or(value);
        validate_report_quoting(value)?;
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(b'|')
            .has_headers(false)
            .from_reader(value.as_bytes());
        let record = reader
            .records()
            .next()
            .ok_or(DecodeError::Malformed)?
            .map_err(|_| DecodeError::Malformed)?;
        let mut fields = record
            .iter()
            .map(|field| {
                let (key, value) =
                    field.split_once('=').ok_or(DecodeError::Malformed)?;
                if key.is_empty() {
                    return Err(DecodeError::Malformed);
                }
                Ok((key.to_owned(), value.to_owned()))
            })
            .collect::<Result<Vec<_>, DecodeError>>()?;

        let result = match take_field(&mut fields, "result")?
            .ok_or(DecodeError::Malformed)?
            .as_str()
        {
            "success" => ReportResult::Success,
            "error" => ReportResult::Error,
            _ => return Err(DecodeError::Malformed),
        };
        let agent =
            take_field(&mut fields, "agent")?.ok_or(DecodeError::Malformed)?;
        let vm_id =
            take_field(&mut fields, "vm_id")?.ok_or(DecodeError::Malformed)?;
        let timestamp = take_field(&mut fields, "timestamp")?
            .ok_or(DecodeError::Malformed)?;
        DateTime::parse_from_rfc3339(&timestamp)
            .map_err(|_| DecodeError::Malformed)?;
        let pps_type = ReportPpsType::from_wire(
            &take_field(&mut fields, "pps_type")?
                .ok_or(DecodeError::Malformed)?,
        )?;
        let (reason, documentation_url) = match result {
            ReportResult::Success => (None, None),
            ReportResult::Error => (
                Some(
                    take_field(&mut fields, "reason")?
                        .ok_or(DecodeError::Malformed)?,
                ),
                take_field(&mut fields, "documentation_url")?,
            ),
        };
        Ok(Self {
            result,
            agent,
            vm_id,
            timestamp,
            pps_type,
            reason,
            documentation_url,
            extra: fields,
        })
    }
}

fn take_field(
    fields: &mut Vec<(String, String)>,
    name: &str,
) -> Result<Option<String>, DecodeError> {
    let mut matches = fields
        .iter()
        .enumerate()
        .filter(|(_, (key, _))| key == name);
    let position = matches.next().map(|(index, _)| index);
    if matches.next().is_some() {
        return Err(DecodeError::Malformed);
    }
    Ok(position.map(|index| fields.remove(index).1))
}

fn validate_report_quoting(value: &str) -> Result<(), DecodeError> {
    enum State {
        Start,
        Unquoted,
        Quoted,
        Closed,
    }
    use State::*;

    // The CSV reader tolerates broken quoting, but reports must be unambiguous.
    let mut state = Start;
    for byte in value.bytes() {
        if byte == 0 {
            return Err(DecodeError::Malformed);
        }
        state = match (state, byte) {
            (Start | Closed, b'"') => Quoted,
            (Quoted, b'"') => Closed,
            (Quoted, _) => Quoted,
            (Start | Unquoted | Closed, b'|') => Start,
            (Closed, _) | (_, b'"' | b'\r' | b'\n') => {
                return Err(DecodeError::Malformed);
            }
            _ => Unquoted,
        };
    }
    if matches!(state, Quoted) {
        return Err(DecodeError::Malformed);
    }
    Ok(())
}

impl ProvisioningReport {
    /// Returns the report's pipe-delimited CSV representation.
    ///
    /// Preserves the report's timestamp and performs no I/O. This method does
    /// not check pool size limits; [`write_report`] applies the store's policy.
    pub fn encode(&self) -> String {
        let mut data = Vec::with_capacity(7 + self.extra.len());

        data.push(format!("result={}", self.result));
        match self.result {
            ReportResult::Success => {
                data.push(format!("agent={}", self.agent));
                data.push(format!("pps_type={}", self.pps_type));
                data.push(format!("vm_id={}", self.vm_id));
                data.push(format!("timestamp={}", self.timestamp));
                for (key, value) in &self.extra {
                    data.push(format!("{key}={value}"));
                }
            }
            ReportResult::Error => {
                if let Some(reason) = &self.reason {
                    data.push(format!("reason={reason}"));
                }
                data.push(format!("agent={}", self.agent));
                for (key, value) in &self.extra {
                    data.push(format!("{key}={value}"));
                }
                data.push(format!("pps_type={}", self.pps_type));
                data.push(format!("vm_id={}", self.vm_id));
                data.push(format!("timestamp={}", self.timestamp));
                if let Some(url) = &self.documentation_url {
                    data.push(format!("documentation_url={url}"));
                }
            }
        }

        let mut writer = csv::WriterBuilder::new()
            .delimiter(b'|')
            .quote_style(csv::QuoteStyle::Necessary)
            .from_writer(vec![]);
        writer
            .write_record(&data)
            .expect("writing to an in-memory buffer cannot fail");
        let mut bytes = writer
            .into_inner()
            .expect("flushing an in-memory buffer cannot fail");
        if let Some(b'\n') = bytes.last() {
            bytes.pop();
        }
        String::from_utf8(bytes).expect("encoded report is valid UTF-8")
    }
}

/// Stores a provisioning result, replacing any existing report in the pool.
///
/// A report must fit in one value under the store's configured size policy.
///
/// # Errors
/// Returns validation and I/O errors from the store.
pub fn write_report(
    store: &KvpPoolStore,
    report: &ProvisioningReport,
) -> Result<(), KvpError> {
    store.insert(PROVISIONING_REPORT_KEY, &report.encode())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KvpPool, PoolMode};
    use rstest::rstest;
    use tempfile::TempDir;

    const VM_ID: &str = "00000000-0000-0000-0000-000000000abc";
    const AGENT: &str = "Azure-Init/0.0.0";
    const TS: &str = "2026-06-17T00:00:00+00:00";

    fn safe_store(dir: &TempDir) -> KvpPoolStore {
        KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe)
            .unwrap()
    }

    /// Pin a report's timestamp so generated entries are deterministic.
    fn with_ts(mut report: ProvisioningReport) -> ProvisioningReport {
        report.timestamp = TS.to_string();
        report
    }

    fn success_wire() -> String {
        with_ts(ProvisioningReport::success(
            AGENT,
            VM_ID,
            ReportPpsType::None,
        ))
        .encode()
    }

    #[rstest]
    #[case::success(
        with_ts(ProvisioningReport::success(AGENT, VM_ID, ReportPpsType::None)),
        "result=success|agent=Azure-Init/0.0.0|pps_type=None|vm_id=00000000-0000-0000-0000-000000000abc|timestamp=2026-06-17T00:00:00+00:00",
    )]
    #[case::success_with_extras(
        with_ts(
            ProvisioningReport::success(AGENT, VM_ID, ReportPpsType::None)
                .with_extra("endpoint", "http://example.com")
                .with_extra("status", "404"),
        ),
        "result=success|agent=Azure-Init/0.0.0|pps_type=None|vm_id=00000000-0000-0000-0000-000000000abc|timestamp=2026-06-17T00:00:00+00:00|endpoint=http://example.com|status=404",
    )]
    #[case::custom_pps_type(
        with_ts(ProvisioningReport::success(
            AGENT,
            VM_ID,
            ReportPpsType::Savable,
        )),
        "result=success|agent=Azure-Init/0.0.0|pps_type=Savable|vm_id=00000000-0000-0000-0000-000000000abc|timestamp=2026-06-17T00:00:00+00:00",
    )]
    #[case::error_with_documentation_url(
        with_ts(
            ProvisioningReport::failure(
                AGENT,
                VM_ID,
                "failed to load sshd config",
                ReportPpsType::None,
            )
            .with_documentation_url("https://aka.ms/linuxprovisioningerror"),
        ),
        "result=error|reason=failed to load sshd config|agent=Azure-Init/0.0.0|pps_type=None|vm_id=00000000-0000-0000-0000-000000000abc|timestamp=2026-06-17T00:00:00+00:00|documentation_url=https://aka.ms/linuxprovisioningerror",
    )]
    #[case::error_without_documentation_url(
        with_ts(ProvisioningReport::failure(
            AGENT,
            VM_ID,
            "boom",
            ReportPpsType::None,
        )),
        "result=error|reason=boom|agent=Azure-Init/0.0.0|pps_type=None|vm_id=00000000-0000-0000-0000-000000000abc|timestamp=2026-06-17T00:00:00+00:00",
    )]
    fn report_wire_format_round_trips(
        #[case] report: ProvisioningReport,
        #[case] expected: &str,
    ) {
        assert_eq!(report.encode(), expected);
        assert_eq!(expected.parse::<ProvisioningReport>().unwrap(), report);
    }

    /// Pins each [`ReportPpsType`] variant to its exact wire string.
    #[rstest]
    #[case(ReportPpsType::None, "None")]
    #[case(ReportPpsType::OsDisk, "PreprovisionedOSDisk")]
    #[case(ReportPpsType::Running, "Running")]
    #[case(ReportPpsType::Savable, "Savable")]
    #[case(ReportPpsType::Unknown, "Unknown")]
    fn pps_type_wire_tokens_match_the_model(
        #[case] pps_type: ReportPpsType,
        #[case] expected: &str,
    ) {
        assert_eq!(pps_type.to_string(), expected);
        assert_eq!(serde_json::to_value(pps_type).unwrap(), expected);
        assert_eq!(ReportPpsType::from_wire(expected).unwrap(), pps_type);
    }

    #[test]
    fn success_serializes_without_absent_fields() {
        let report = with_ts(ProvisioningReport::success(
            AGENT,
            VM_ID,
            ReportPpsType::None,
        ));
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::json!({
                "result": "success",
                "agent": AGENT,
                "vm_id": VM_ID,
                "timestamp": TS,
                "pps_type": "None",
            })
        );
    }

    #[test]
    fn failure_serialization_preserves_ordered_extras() {
        let report = with_ts(
            ProvisioningReport::failure(
                AGENT,
                VM_ID,
                "boom",
                ReportPpsType::OsDisk,
            )
            .with_extra("detail", "first")
            .with_extra("detail", "second")
            .with_extra("result", "extra context")
            .with_documentation_url("https://aka.ms/linuxprovisioningerror"),
        );
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::json!({
                "result": "error",
                "agent": AGENT,
                "vm_id": VM_ID,
                "timestamp": TS,
                "pps_type": "PreprovisionedOSDisk",
                "reason": "boom",
                "documentation_url": "https://aka.ms/linuxprovisioningerror",
                "extra": [
                    ["detail", "first"],
                    ["detail", "second"],
                    ["result", "extra context"],
                ],
            })
        );
    }

    /// The success layout lists the standard fields first, then any
    /// extras appended at the very end.
    #[test]
    fn success_layout_appends_extras_last() {
        let report = with_ts(
            ProvisioningReport::success(AGENT, VM_ID, ReportPpsType::None)
                .with_extra("build", "test-123"),
        );
        assert_eq!(
            report.encode(),
            "result=success|agent=Azure-Init/0.0.0|pps_type=None\
|vm_id=00000000-0000-0000-0000-000000000abc\
|timestamp=2026-06-17T00:00:00+00:00|build=test-123"
        );
    }

    /// The failure layout lists reason and agent first, extras
    /// (supporting data) before the standard fields, and
    /// `documentation_url` last.
    #[test]
    fn failure_layout_places_extras_before_pps_type() {
        let report = with_ts(
            ProvisioningReport::failure(
                AGENT,
                VM_ID,
                "boom",
                ReportPpsType::None,
            )
            .with_extra("details", "bad config")
            .with_documentation_url("https://aka.ms/linuxprovisioningerror"),
        );
        assert_eq!(
            report.encode(),
            "result=error|reason=boom|agent=Azure-Init/0.0.0\
|details=bad config|pps_type=None\
|vm_id=00000000-0000-0000-0000-000000000abc\
|timestamp=2026-06-17T00:00:00+00:00\
|documentation_url=https://aka.ms/linuxprovisioningerror"
        );
    }

    #[test]
    fn default_timestamp_is_populated() {
        let report =
            ProvisioningReport::success(AGENT, VM_ID, ReportPpsType::None);
        assert!(!report.timestamp.is_empty());
    }

    #[test]
    fn write_report_round_trips_through_store() {
        let dir = TempDir::new().unwrap();
        let store = safe_store(&dir);

        let report = with_ts(
            ProvisioningReport::failure(
                AGENT,
                VM_ID,
                "boom",
                ReportPpsType::None,
            )
            .with_extra("details", "bad config")
            .with_documentation_url("https://aka.ms/linuxprovisioningerror"),
        );

        write_report(&store, &report).unwrap();

        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries.get(PROVISIONING_REPORT_KEY).map(String::as_str),
            Some(
                "result=error|reason=boom|agent=Azure-Init/0.0.0|details=bad config|pps_type=None|vm_id=00000000-0000-0000-0000-000000000abc|timestamp=2026-06-17T00:00:00+00:00|documentation_url=https://aka.ms/linuxprovisioningerror"
            )
        );
    }

    #[test]
    fn write_report_is_idempotent_upsert() {
        let dir = TempDir::new().unwrap();
        let store = safe_store(&dir);

        let report = with_ts(ProvisioningReport::success(
            AGENT,
            VM_ID,
            ReportPpsType::None,
        ));
        write_report(&store, &report).unwrap();
        write_report(&store, &report).unwrap();

        assert_eq!(store.len().unwrap(), 1);
    }

    #[test]
    fn write_report_propagates_store_error() {
        let dir = TempDir::new().unwrap();
        let store = safe_store(&dir);

        let oversized = "x".repeat(store.max_value_size() + 1);
        let report =
            ProvisioningReport::success(AGENT, VM_ID, ReportPpsType::None)
                .with_extra("big", oversized);

        let result = write_report(&store, &report);
        assert!(result.is_err());
    }

    #[test]
    fn parses_reordered_fields_without_normalizing_identity_or_timestamp() {
        let timestamp = "2026-06-17T02:00:00.123456+02:00";
        let value = format!(
            "timestamp={timestamp}|vm_id=vm-abc|pps_type=None|agent={AGENT}|result=success"
        );
        let mut expected =
            ProvisioningReport::success(AGENT, "vm-abc", ReportPpsType::None);
        expected.timestamp = timestamp.into();
        assert_eq!(value.parse::<ProvisioningReport>().unwrap(), expected);
    }

    #[test]
    fn failure_round_trips_quoted_fields_and_documentation_url() {
        let report = with_ts(
            ProvisioningReport::failure(
                AGENT,
                VM_ID,
                "failed | \"quoted\"\r\nnext line",
                ReportPpsType::Running,
            )
            .with_extra("context", "key=value|details\nmore")
            .with_documentation_url("https://example.invalid/?key=a=b"),
        );
        assert_eq!(
            report.encode().parse::<ProvisioningReport>().unwrap(),
            report
        );
    }

    #[test]
    fn supporting_data_preserves_order_duplicates_and_empty_values() {
        let value =
            format!("{}|detail=first|detail=second|empty=", success_wire());
        let report = value.parse::<ProvisioningReport>().unwrap();
        assert_eq!(
            report.extra,
            vec![
                ("detail".into(), "first".into()),
                ("detail".into(), "second".into()),
                ("empty".into(), String::new()),
            ]
        );
    }

    #[test]
    fn success_keeps_failure_only_fields_as_supporting_data() {
        let report = with_ts(
            ProvisioningReport::success(AGENT, VM_ID, ReportPpsType::None)
                .with_extra("reason", "additional context")
                .with_extra("documentation_url", "https://example.invalid/"),
        );
        assert_eq!(
            report.encode().parse::<ProvisioningReport>().unwrap(),
            report
        );
    }

    #[rstest]
    #[case::result("result")]
    #[case::agent("agent")]
    #[case::vm_id("vm_id")]
    #[case::timestamp("timestamp")]
    #[case::pps_type("pps_type")]
    fn report_requires_each_standard_field(#[case] missing: &str) {
        let value = success_wire()
            .split('|')
            .filter(|field| !field.starts_with(&format!("{missing}=")))
            .collect::<Vec<_>>()
            .join("|");
        assert_eq!(
            value.parse::<ProvisioningReport>(),
            Err(DecodeError::Malformed)
        );
    }

    #[test]
    fn failure_requires_a_reason() {
        let value = success_wire().replace("result=success", "result=error");
        assert_eq!(
            value.parse::<ProvisioningReport>(),
            Err(DecodeError::Malformed)
        );
    }

    #[rstest]
    #[case::success(with_ts(ProvisioningReport::success(
        "",
        "",
        ReportPpsType::None
    )))]
    #[case::failure(with_ts(ProvisioningReport::failure(
        AGENT,
        VM_ID,
        "",
        ReportPpsType::None
    )))]
    fn empty_values_supported_by_the_writer_remain_readable(
        #[case] report: ProvisioningReport,
    ) {
        assert_eq!(
            report.encode().parse::<ProvisioningReport>().unwrap(),
            report
        );
    }

    #[rstest]
    #[case::result("result=success", "result=fail")]
    #[case::pps_type("pps_type=None", "pps_type=FutureType")]
    #[case::timestamp(TS, "not-a-timestamp")]
    fn invalid_standard_values_are_malformed(
        #[case] from: &str,
        #[case] to: &str,
    ) {
        let value = success_wire().replace(from, to);
        assert_eq!(
            value.parse::<ProvisioningReport>(),
            Err(DecodeError::Malformed)
        );
    }

    #[rstest]
    #[case::unclosed_quote("\"extra=value")]
    #[case::characters_after_quote("\"extra=value\"suffix")]
    #[case::unquoted_quote("extra=va\"lue")]
    #[case::missing_equals("extra")]
    #[case::empty_key("=value")]
    #[case::null("extra=va\0lue")]
    fn malformed_supporting_fields_are_not_silently_repaired(
        #[case] extra: &str,
    ) {
        let value = format!("{}|{extra}", success_wire());
        assert_eq!(
            value.parse::<ProvisioningReport>(),
            Err(DecodeError::Malformed)
        );
    }

    #[test]
    fn multiple_csv_records_are_rejected() {
        let value = format!("{}\n{}", success_wire(), success_wire());
        assert_eq!(
            value.parse::<ProvisioningReport>(),
            Err(DecodeError::Malformed)
        );
    }

    #[test]
    fn optional_csv_quotes_and_record_terminators_are_accepted() {
        let quoted = success_wire()
            .split('|')
            .map(|field| format!("\"{field}\""))
            .collect::<Vec<_>>()
            .join("|");
        let expected = success_wire().parse::<ProvisioningReport>().unwrap();
        for ending in ["", "\n", "\r\n"] {
            assert_eq!(
                format!("{quoted}{ending}")
                    .parse::<ProvisioningReport>()
                    .unwrap(),
                expected
            );
        }
    }

    #[rstest]
    #[case::conflicting_result("result=error")]
    #[case::repeated_agent("agent=Azure-Init/0.0.0")]
    fn duplicate_standard_fields_are_ambiguous(#[case] duplicate: &str) {
        let value = format!("{}|{duplicate}", success_wire());
        assert_eq!(
            value.parse::<ProvisioningReport>(),
            Err(DecodeError::Malformed)
        );
    }
}
