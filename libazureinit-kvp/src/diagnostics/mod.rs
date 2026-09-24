// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Diagnostic types and KVP pool access.

mod cloud_init;
mod diagnostic;
mod encoding;
mod reader;
#[cfg(feature = "tracing")]
mod tracing_layer;
mod writer;

pub use diagnostic::{
    DecodeError, Diagnostic, DiagnosticEvent, DiagnosticFinish, DiagnosticKey,
    DiagnosticPayload, DiagnosticStart, Encoding, Entry, Kind, Outcome,
    RawKeyValue, DIAGNOSTIC_VERSION_ID,
};
pub use reader::DiagnosticReader;
#[cfg(feature = "tracing")]
pub use tracing_layer::DiagnosticsKvp;
pub use writer::{DiagnosticWriter, DurationPrecision, TimestampPrecision};

/// Maximum encoded payload bytes stored in one diagnostic record.
///
/// [`DiagnosticWriter`] splits larger payloads automatically.
pub const MAX_CHUNK_BYTES: usize = 1022;

/// Parse a non-empty run of ASCII digits (a chunk index or duration) as `u64`.
fn parse_unsigned(value: &str) -> Result<u64, DecodeError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DecodeError::Malformed);
    }
    value.parse().map_err(|_| DecodeError::Malformed)
}
