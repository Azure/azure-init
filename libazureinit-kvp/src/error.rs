// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::io;

/// Errors returned by KVP storage and diagnostic writing.
#[derive(Debug)]
pub enum KvpError {
    /// The key was empty.
    EmptyKey,
    EmptyEventField {
        field: &'static str,
    },
    /// An underlying I/O error.
    Io(io::Error),
    /// An event key field (`agent`, `vm_id`, `kind`, `name`, or `event_id`)
    /// contained the `|` delimiter, which would make the formatted event
    /// key ambiguous to parse back.
    EventFieldContainsDelimiter {
        field: &'static str,
    },
    EventFieldTooLong {
        field: &'static str,
        max: usize,
        actual: usize,
    },
    InvalidUuid {
        field: &'static str,
    },
    DurationTooLarge {
        max_ms: u64,
        actual_ms: u64,
    },
    TooManyChunks {
        max: usize,
    },
    /// The key contains a null byte, which is incompatible with the
    /// on-disk format (null-padded fixed-width fields).
    KeyContainsNull,
    /// The key exceeds the store's maximum key size.
    KeyTooLarge {
        max: usize,
        actual: usize,
    },
    /// The store already has the maximum allowed number of unique keys.
    MaxUniqueKeysExceeded {
        max: usize,
    },
    PayloadNotUtf8,
    UnsupportedEncoding {
        token: String,
    },
    /// The value exceeds the store's maximum value size.
    ValueTooLarge {
        max: usize,
        actual: usize,
    },
    /// The value contains a null byte, which is incompatible with the
    /// null-padded KVP wire format.
    ValueContainsNull,
}

impl fmt::Display for KvpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyKey => write!(f, "KVP key must not be empty"),
            Self::EmptyEventField { field } => {
                write!(f, "event key field '{field}' must not be empty")
            }
            Self::EventFieldContainsDelimiter { field } => {
                write!(f, "event key field '{field}' must not contain '|'")
            }
            Self::EventFieldTooLong { field, max, actual } => {
                write!(f, "event key field '{field}' length ({actual}) exceeds maximum ({max})")
            }
            Self::InvalidUuid { field } => {
                write!(f, "event key field '{field}' must be a UUID")
            }
            Self::DurationTooLarge { max_ms, actual_ms } => {
                write!(f, "diagnostic duration ({actual_ms}ms) exceeds maximum ({max_ms}ms)")
            }
            Self::TooManyChunks { max } => {
                write!(f, "diagnostic chunk count exceeds maximum ({max})")
            }
            Self::Io(e) => write!(f, "{e}"),
            Self::KeyContainsNull => {
                write!(f, "KVP key must not contain null bytes")
            }
            Self::KeyTooLarge { max, actual } => {
                write!(f, "KVP key length ({actual}) exceeds maximum ({max})")
            }
            Self::MaxUniqueKeysExceeded { max } => {
                write!(f, "KVP unique key count exceeded maximum ({max})")
            }
            Self::PayloadNotUtf8 => {
                write!(f, "diagnostic payload must be valid UTF-8 for encoding 'none'")
            }
            Self::UnsupportedEncoding { token } => {
                write!(f, "diagnostic encoding '{token}' is not supported")
            }
            Self::ValueTooLarge { max, actual } => {
                write!(f, "KVP value length ({actual}) exceeds maximum ({max})")
            }
            Self::ValueContainsNull => {
                write!(f, "KVP value must not contain null bytes")
            }
        }
    }
}

impl std::error::Error for KvpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for KvpError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(
        KvpError::EmptyEventField { field: "name" },
        "event key field 'name' must not be empty"
    )]
    #[case(
        KvpError::EventFieldContainsDelimiter { field: "name" },
        "event key field 'name' must not contain '|'"
    )]
    #[case(
        KvpError::EventFieldTooLong { field: "name", max: 48, actual: 49 },
        "event key field 'name' length (49) exceeds maximum (48)"
    )]
    #[case(
        KvpError::InvalidUuid { field: "vm_id" },
        "event key field 'vm_id' must be a UUID"
    )]
    #[case(
        KvpError::DurationTooLarge { max_ms: 9_999_999_999, actual_ms: 10_000_000_000 },
        "diagnostic duration (10000000000ms) exceeds maximum (9999999999ms)"
    )]
    #[case(
        KvpError::TooManyChunks { max: 1023 },
        "diagnostic chunk count exceeds maximum (1023)"
    )]
    #[case(
        KvpError::PayloadNotUtf8,
        "diagnostic payload must be valid UTF-8 for encoding 'none'"
    )]
    #[case(
        KvpError::UnsupportedEncoding { token: "zstd+b64".into() },
        "diagnostic encoding 'zstd+b64' is not supported"
    )]
    fn diagnostic_validation_errors_explain_the_failure(
        #[case] error: KvpError,
        #[case] expected: &str,
    ) {
        assert_eq!(error.to_string(), expected);
        let error: &dyn std::error::Error = &error;
        assert!(error.source().is_none());
    }
}
