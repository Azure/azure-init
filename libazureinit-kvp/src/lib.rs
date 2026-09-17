// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Read and write Hyper-V KVP metadata on Linux guests.
//!
//! KVP (Key-Value Pair) data exchange lets a VM share small text records with
//! its host without network connectivity. This library operates on the local
//! pool files, normally in `/var/lib/hyperv`. The Hyper-V daemon and kernel
//! handle transport; writing a file does not itself notify the host.
//!
//! # Choose an API
//! - [`DiagnosticWriter`] emits operations and observations, handling
//!   timestamps, compression and splitting long payloads into records.
//! - [`DiagnosticReader`] reads native and cloud-init diagnostics, provisioning
//!   reports, and unrecognized records as [`Entry`] values.
//! - [`ProvisioningReport`] and [`write_report`] publish one replaceable
//!   provisioning result, rather than a history of diagnostic events.
//! - [`KvpPoolStore`] reads and writes ordinary key/value records. Use
//!   [`insert`](KvpPoolStore::insert) to update a key, or
//!   [`append`](KvpPoolStore::append) to preserve duplicates.
//!
//! Use [`KvpPool::Guest`] for guest-produced telemetry and [`PoolMode::Safe`]
//! for host-readable writes. [`KvpPool::AutoExternal`] contains host-provided
//! data. The selected directory must exist and permit the requested file I/O;
//! [`KvpPoolStore::new_in`] selects an alternate directory.
//!
//! # Emit Telemetry
//!
//! ```no_run
//! use libazureinit_kvp::{
//!     DiagnosticWriter, Encoding, KvpPool, KvpPoolStore, Outcome, PoolMode,
//! };
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let store = KvpPoolStore::new(KvpPool::Guest, PoolMode::Safe)?;
//! let writer = DiagnosticWriter::new(
//!     store,
//!     "azure-init/0.1.1",
//!     "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
//! )?;
//! writer.emit_event(
//!     "imds", "metadata retrieved", None, Some(Outcome::Success), None,
//! )?;
//! writer.emit_event(
//!     "os:release", std::fs::read("/etc/os-release")?,
//!     Some(Encoding::ZlibB64), None, None,
//! )?;
//! # Ok(())
//! # }
//! ```
//!
//! See [`DiagnosticWriter`] for recording operations, [`DiagnosticReader`] for
//! consuming telemetry, and [`ProvisioningReport`] for reporting provisioning
//! success or failure.
//!
//! # Format References
//! The [KVP contract] describes pool files and Hyper-V interfaces. The
//! [diagnostics contract] describes record fields and encodings for consumers
//! that read telemetry independently of this library.
//!
//! [KVP contract]: https://github.com/Azure/azure-init/blob/main/doc/kvp.md
//! [diagnostics contract]: https://github.com/Azure/azure-init/blob/main/doc/diagnostics.md

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
    DurationPrecision, Encoding, Entry, Kind, Outcome, RawKeyValue,
    TimestampPrecision, DIAGNOSTIC_VERSION_ID, MAX_CHUNK_BYTES,
};
pub use error::KvpError;
pub use report::{
    write_report, ProvisioningReport, ReportPpsType, PROVISIONING_REPORT_KEY,
};
pub use store::{KvpPool, KvpPoolStore, PoolMode};
