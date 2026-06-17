// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Structured provisioning report abstraction layered over the raw
//! [`KvpPoolStore`](crate::KvpPoolStore) key/value API.
//!
//! [`ProvisioningReport`] is a strongly-typed representation of a
//! provisioning health report. Instead of building ad-hoc key/value
//! strings at the call site, callers construct a report and convert it
//! into ordered KVP entries via [`ToKvp::to_kvp`], then persist it with
//! [`write_report`].
//!
//! The [`ToKvp`] trait is the shared seam intended for future layering:
//! a diagnostics report can implement the same trait and reuse
//! [`write_report`] without changing this module.

use chrono::Utc;

use crate::{KvpError, KvpPoolStore};

/// Default value for the `pps_type` field when none is specified.
const DEFAULT_PPS_TYPE: &str = "None";

/// The current time formatted as an RFC 3339 string.
fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}
/// Conversion into ordered KVP entries.
///
/// Implementors return key/value pairs in a deterministic order so the
/// resulting KVP records are stable and easy to assert against. This is
/// the shared seam that future report types (e.g. diagnostics) can
/// implement to reuse [`write_report`].
pub trait ToKvp {
    /// Return the report as ordered `(key, value)` pairs.
    fn to_kvp(&self) -> Vec<(String, String)>;
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

/// A strongly-typed provisioning health report.
///
/// Construct one with [`ProvisioningReport::success`] or
/// [`ProvisioningReport::error`], optionally attach extra context with
/// the builder methods, then convert to KVP entries with
/// [`ToKvp::to_kvp`] or persist with [`write_report`].
///
/// # Example
/// ```no_run
/// use libazureinit_kvp::{
///     write_report, KvpPool, KvpPoolStore, PoolMode, ProvisioningReport,
/// };
///
/// # fn main() -> Result<(), libazureinit_kvp::KvpError> {
/// let store = KvpPoolStore::new(KvpPool::Guest, PoolMode::Safe)?;
///
/// let report = ProvisioningReport::success(
///     format!("Azure-Init/{}", env!("CARGO_PKG_VERSION")),
///     "00000000-0000-0000-0000-000000000abc",
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
    /// Pre-provisioning type (`pps_type` field). Defaults to `None`.
    pps_type: String,
    /// Failure reason (`reason` field). Present for error reports.
    reason: Option<String>,
    /// Documentation URL (`documentation_url` field), if applicable.
    documentation_url: Option<String>,
    /// Additional ordered key/value context (e.g. supporting data).
    extra: Vec<(String, String)>,
}

impl ProvisioningReport {
    /// Create a successful provisioning report.
    pub fn success(agent: impl Into<String>, vm_id: impl Into<String>) -> Self {
        Self {
            result: ReportResult::Success,
            agent: agent.into(),
            vm_id: vm_id.into(),
            timestamp: now_rfc3339(),
            pps_type: DEFAULT_PPS_TYPE.to_string(),
            reason: None,
            documentation_url: None,
            extra: Vec::new(),
        }
    }

    /// Create a failed provisioning report with a failure reason.
    pub fn error(
        agent: impl Into<String>,
        vm_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            result: ReportResult::Error,
            agent: agent.into(),
            vm_id: vm_id.into(),
            timestamp: now_rfc3339(),
            pps_type: DEFAULT_PPS_TYPE.to_string(),
            reason: Some(reason.into()),
            documentation_url: None,
            extra: Vec::new(),
        }
    }

    /// Override the `pps_type` field (defaults to `None`).
    pub fn with_pps_type(mut self, pps_type: impl Into<String>) -> Self {
        self.pps_type = pps_type.into();
        self
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

impl ToKvp for ProvisioningReport {
    /// Emit entries in a deterministic order:
    /// `result`, `reason` (if any), `agent`, extras (in order),
    /// `pps_type`, `vm_id`, `timestamp`, `documentation_url` (if any).
    fn to_kvp(&self) -> Vec<(String, String)> {
        let mut entries = Vec::with_capacity(6 + self.extra.len());

        entries.push(("result".to_string(), self.result.to_string()));
        if let Some(reason) = &self.reason {
            entries.push(("reason".to_string(), reason.clone()));
        }
        entries.push(("agent".to_string(), self.agent.clone()));
        for (key, value) in &self.extra {
            entries.push((key.clone(), value.clone()));
        }
        entries.push(("pps_type".to_string(), self.pps_type.clone()));
        entries.push(("vm_id".to_string(), self.vm_id.clone()));
        entries.push(("timestamp".to_string(), self.timestamp.clone()));
        if let Some(url) = &self.documentation_url {
            entries.push(("documentation_url".to_string(), url.clone()));
        }

        entries
    }
}

/// Persist a report to the KVP store.
///
/// Each entry from [`ToKvp::to_kvp`] is written with
/// [`KvpPoolStore::insert`] (upsert / last-write-wins), so re-emitting a
/// report collapses to a single record per key rather than accumulating
/// duplicates. The inserts are not transactional: an I/O error partway
/// through can leave entries written before the failure in the store.
pub fn write_report(
    store: &KvpPoolStore,
    report: &impl ToKvp,
) -> Result<(), KvpError> {
    for (key, value) in report.to_kvp() {
        store.insert(&key, &value)?;
    }
    Ok(())
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
        with_ts(ProvisioningReport::success(AGENT, VM_ID)),
        vec![
            ("result", "success"),
            ("agent", AGENT),
            ("pps_type", "None"),
            ("vm_id", VM_ID),
            ("timestamp", TS),
        ],
    )]
    #[case::success_with_extras(
        with_ts(
            ProvisioningReport::success(AGENT, VM_ID)
                .with_extra("endpoint", "http://example.com")
                .with_extra("status", "404"),
        ),
        vec![
            ("result", "success"),
            ("agent", AGENT),
            ("endpoint", "http://example.com"),
            ("status", "404"),
            ("pps_type", "None"),
            ("vm_id", VM_ID),
            ("timestamp", TS),
        ],
    )]
    #[case::custom_pps_type(
        with_ts(
            ProvisioningReport::success(AGENT, VM_ID)
                .with_pps_type("Savable"),
        ),
        vec![
            ("result", "success"),
            ("agent", AGENT),
            ("pps_type", "Savable"),
            ("vm_id", VM_ID),
            ("timestamp", TS),
        ],
    )]
    #[case::error_with_documentation_url(
        with_ts(
            ProvisioningReport::error(AGENT, VM_ID, "failed to load sshd config")
                .with_documentation_url("https://aka.ms/linuxprovisioningerror"),
        ),
        vec![
            ("result", "error"),
            ("reason", "failed to load sshd config"),
            ("agent", AGENT),
            ("pps_type", "None"),
            ("vm_id", VM_ID),
            ("timestamp", TS),
            ("documentation_url", "https://aka.ms/linuxprovisioningerror"),
        ],
    )]
    #[case::error_without_documentation_url(
        with_ts(ProvisioningReport::error(AGENT, VM_ID, "boom")),
        vec![
            ("result", "error"),
            ("reason", "boom"),
            ("agent", AGENT),
            ("pps_type", "None"),
            ("vm_id", VM_ID),
            ("timestamp", TS),
        ],
    )]
    fn to_kvp_emits_expected_entries(
        #[case] report: ProvisioningReport,
        #[case] expected: Vec<(&str, &str)>,
    ) {
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(report.to_kvp(), expected);
    }

    #[test]
    fn default_timestamp_is_populated() {
        let report = ProvisioningReport::success(AGENT, VM_ID);
        assert!(!report.timestamp.is_empty());
    }

    #[test]
    fn write_report_round_trips_through_store() {
        let dir = TempDir::new().unwrap();
        let store = safe_store(&dir);

        let report = with_ts(
            ProvisioningReport::error(AGENT, VM_ID, "boom")
                .with_extra("details", "bad config")
                .with_documentation_url(
                    "https://aka.ms/linuxprovisioningerror",
                ),
        );

        write_report(&store, &report).unwrap();

        let entries = store.entries().unwrap();
        assert_eq!(entries.get("result").map(String::as_str), Some("error"));
        assert_eq!(entries.get("reason").map(String::as_str), Some("boom"));
        assert_eq!(
            entries.get("details").map(String::as_str),
            Some("bad config")
        );
        assert_eq!(entries.get("vm_id").map(String::as_str), Some(VM_ID));
        assert_eq!(entries.get("timestamp").map(String::as_str), Some(TS));
        assert_eq!(
            entries.get("documentation_url").map(String::as_str),
            Some("https://aka.ms/linuxprovisioningerror")
        );
    }

    #[test]
    fn write_report_is_idempotent_upsert() {
        let dir = TempDir::new().unwrap();
        let store = safe_store(&dir);

        let report = with_ts(ProvisioningReport::success(AGENT, VM_ID));
        write_report(&store, &report).unwrap();
        write_report(&store, &report).unwrap();

        assert_eq!(store.len().unwrap(), report.to_kvp().len());
    }

    #[test]
    fn write_report_propagates_store_error() {
        let dir = TempDir::new().unwrap();
        let store = safe_store(&dir);

        let oversized = "x".repeat(store.max_value_size() + 1);
        let report = ProvisioningReport::success(AGENT, VM_ID)
            .with_extra("big", oversized);

        let result = write_report(&store, &report);
        assert!(result.is_err());
    }
}
