// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Structured provisioning report abstraction layered over the raw
//! [`KvpPoolStore`](crate::KvpPoolStore) key/value API.
//!
//! [`ProvisioningReport`] is a strongly-typed representation of a
//! provisioning health report. Instead of building ad-hoc key/value
//! strings at the call site, callers construct a report and persist it
//! with [`write_report`], which serializes it into the single
//! pipe-delimited `PROVISIONING_REPORT` KVP record that the Azure/Hyper-V
//! host parses.

use chrono::Utc;

use crate::{KvpError, KvpPoolStore};

/// KVP key under which the encoded provisioning health report is stored.
///
/// The Azure/Hyper-V host parses this single key; its value is the
/// pipe-delimited `key=value|key=value|...` report produced by
/// [`write_report`].
pub const PROVISIONING_REPORT_KEY: &str = "PROVISIONING_REPORT";

/// The current time formatted as an RFC 3339 string.
fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// Outcome of a provisioning attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Pre-provisioning (PPS) type reported in the `pps_type` field.
///
/// Mirrors the values cloud-init reports for the platform's
/// `PreprovisionedVMType` / IMDS `ppsType`. Marked `#[non_exhaustive]`
/// so new platform PPS types can be added without breaking downstream
/// `match` statements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportPpsType {
    /// Not pre-provisioned (`None`).
    None,
    /// Pre-provisioned OS disk (`PreprovisionedOSDisk`).
    OsDisk,
    /// Running pre-provisioning (`Running`).
    Running,
    /// Savable pre-provisioning (`Savable`).
    Savable,
    /// Unknown pre-provisioning type (`Unknown`).
    Unknown,
}

impl ReportPpsType {
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

/// A strongly-typed provisioning health report.
///
/// Construct one with [`ProvisioningReport::success`] or
/// [`ProvisioningReport::failure`], optionally attach extra context with
/// the builder methods, then persist it with [`write_report`].
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisioningReport {
    /// Provisioning outcome (`result` field).
    result: ReportResult,
    /// Reporting agent identifier (`agent` field).
    agent: String,
    /// Virtual machine identifier (`vm_id` field).
    vm_id: String,
    /// Report timestamp (`timestamp` field), set to the current time
    /// (RFC 3339) when the report is constructed.
    timestamp: String,
    /// Pre-provisioning type (`pps_type` field).
    pps_type: ReportPpsType,
    /// Failure reason (`reason` field). Present for error reports.
    reason: Option<String>,
    /// Documentation URL (`documentation_url` field), if applicable.
    documentation_url: Option<String>,
    /// Additional ordered key/value context (e.g. supporting data).
    extra: Vec<(String, String)>,
}

impl ProvisioningReport {
    /// Create a successful provisioning report.
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

    /// Create a failed provisioning report with a failure reason.
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

    /// Attach a documentation URL.
    pub fn with_documentation_url(mut self, url: impl Into<String>) -> Self {
        self.documentation_url = Some(url.into());
        self
    }

    /// Append an additional key/value pair. Extras are emitted in the
    /// order they were added.
    pub fn with_extra(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.extra.push((key.into(), value.into()));
        self
    }
}

impl ProvisioningReport {
    /// Encode the report as a single pipe-delimited `key=value` string.
    ///
    /// - Success: `result`, `agent`, `pps_type`, `vm_id`, `timestamp`,
    ///   then any extras in insertion order.
    /// - Failure: `result`, `reason`, `agent`, extras in insertion
    ///   order, `pps_type`, `vm_id`, `timestamp`, then
    ///   `documentation_url` (if any).
    fn encode(&self) -> String {
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

/// Persist a report to the KVP store under [`PROVISIONING_REPORT_KEY`].
///
/// The report is encoded into a single pipe-delimited value and written
/// with [`KvpPoolStore::insert`] (upsert / last-write-wins), so it
/// overrides any existing `PROVISIONING_REPORT` record rather than
/// accumulating duplicates.
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
    fn encode_emits_expected_pipe_string(
        #[case] report: ProvisioningReport,
        #[case] expected: &str,
    ) {
        assert_eq!(report.encode(), expected);
    }

    /// Pins each [`ReportPpsType`] variant to its exact wire string.
    #[rstest]
    #[case(ReportPpsType::None, "None")]
    #[case(ReportPpsType::OsDisk, "PreprovisionedOSDisk")]
    #[case(ReportPpsType::Running, "Running")]
    #[case(ReportPpsType::Savable, "Savable")]
    #[case(ReportPpsType::Unknown, "Unknown")]
    fn pps_type_display_matches_wire_string(
        #[case] pps_type: ReportPpsType,
        #[case] expected: &str,
    ) {
        assert_eq!(pps_type.to_string(), expected);
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
}
