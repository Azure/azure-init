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
//! - [`DiagnosticWriter`]: typed writer for versioned diagnostics.
//! - [`DiagnosticReader`]: reader for diagnostics, provisioning reports, and
//!   raw records, including a read-only cloud-init compatibility bridge.
//!
//! # Diagnostics
//!
//! The reader preserves first-seen pool order. Unknown or invalid records
//! remain [`Entry::Raw`] within a successful snapshot; a failed snapshot,
//! including invalid physical UTF-8, returns an error without entries.
//! The CLI's `dump --parse` renders those entries in the same pool order.
//!
//! ```no_run
//! use libazureinit_kvp::{
//!     DiagnosticReader, DiagnosticWriter, KvpPool, KvpPoolStore, Outcome,
//!     PoolMode,
//! };
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let store = KvpPoolStore::new(KvpPool::Guest, PoolMode::Safe)?;
//! let writer = DiagnosticWriter::new(
//!     store.clone(),
//!     "azure-init",
//!     "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
//! )?;
//! writer.emit_event(
//!     "imds", "metadata retrieved", None, Some(Outcome::Success), None,
//! )?;
//!
//! let entries = DiagnosticReader::new(store).entries()?;
//! println!("{}", serde_json::to_string(&entries)?);
//! # Ok(())
//! # }
//! ```

mod cli;
mod diagnostics;
mod error;
mod report;
mod store;
mod vm_id;

pub use cli::run;
pub use diagnostics::{
    DecodeError, Diagnostic, DiagnosticEvent, DiagnosticFinish, DiagnosticKey,
    DiagnosticPayload, DiagnosticReader, DiagnosticStart, DiagnosticWriter,
    Encoding, Entry, Kind, Outcome, RawKeyValue, DIAGNOSTIC_VERSION_ID,
    MAX_CHUNK_BYTES,
};
pub use error::KvpError;
pub use report::{
    write_report, ProvisioningReport, ReportPpsType, PROVISIONING_REPORT_KEY,
};
pub use store::{KvpPool, KvpPoolStore, PoolMode};
