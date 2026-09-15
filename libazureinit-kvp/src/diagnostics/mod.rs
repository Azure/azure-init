// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed diagnostics over the raw [`crate::KvpPoolStore`].
//!
//! [`DiagnosticWriter`] emits versioned `DIAG_V1` records. [`DiagnosticReader`]
//! reads diagnostics, provisioning reports, and raw records from a successful
//! string snapshot, including cloud-init diagnostics through a read-only bridge.

mod cloud_init;
mod diagnostic;
mod encoding;
mod reader;
mod writer;

pub use diagnostic::{
    DecodeError, Diagnostic, DiagnosticEvent, DiagnosticFinish, DiagnosticKey,
    DiagnosticPayload, DiagnosticStart, Encoding, Entry, Kind, Outcome,
    RawKeyValue, DIAGNOSTIC_VERSION_ID,
};
pub use reader::DiagnosticReader;
pub use writer::{DiagnosticWriter, TimestampPrecision};

/// Maximum number of UTF-8 value bytes stored in one diagnostic record.
///
/// This conservative limit keeps records readable through the Hyper-V host
/// path. Longer messages are split at UTF-8 character boundaries.
pub const MAX_CHUNK_BYTES: usize = 1022;

/// Parse a non-empty run of ASCII digits (a chunk index or duration) as `u64`.
fn parse_unsigned(value: &str) -> Result<u64, DecodeError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DecodeError::Malformed);
    }
    value.parse().map_err(|_| DecodeError::Malformed)
}
