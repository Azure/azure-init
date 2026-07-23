// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `libazureinit-kvp` provides a unified KVP pool file store for
//! Hyper-V/Azure guests.
//!
//! - [`KvpPoolStore`]: KVP pool file store with
//!   [`PoolMode`]-based policy.
//! - [`ProvisioningReport`]: structured provisioning health report that
//!   is persisted as the single `PROVISIONING_REPORT` record with
//!   [`write_report`].
//! - [`DiagnosticsKvp`]: typed view over [`KvpPoolStore`] that formats,
//!   chunks, and reassembles azure-init diagnostic events.

mod cli;
mod diagnostics;
mod error;
mod report;
mod store;
mod vm_id;

pub use cli::run;
pub use diagnostics::{
    DiagnosticEvent, DiagnosticRecord, DiagnosticsKvp, MAX_CHUNK_BYTES,
};
pub use error::KvpError;
pub use report::{
    write_report, ProvisioningReport, ReportPpsType, PROVISIONING_REPORT_KEY,
};
pub use store::{KvpPool, KvpPoolStore, PoolMode};
