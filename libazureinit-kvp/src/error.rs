// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::io;

/// Errors returned by KVP storage and telemetry writers.
#[derive(Debug)]
pub enum KvpError {
    /// The key was empty.
    EmptyKey,
    /// A required diagnostic field was empty.
    EmptyEventField {
        /// Name of the rejected field.
        field: &'static str,
    },
    /// An I/O operation failed, or stored pool data was invalid.
    Io(io::Error),
    /// A diagnostic field contained the reserved `|` delimiter.
    EventFieldContainsDelimiter {
        /// Name of the rejected field.
        field: &'static str,
    },
    /// A diagnostic field exceeded its UTF-8 byte limit.
    EventFieldTooLong {
        /// Name of the rejected field.
        field: &'static str,
        /// Maximum allowed bytes.
        max: usize,
        /// Supplied bytes.
        actual: usize,
    },
    /// A VM or event identifier was not a valid UUID.
    InvalidUuid {
        /// Name of the rejected field.
        field: &'static str,
    },
    /// An encoded payload needed too many records.
    TooManyChunks {
        /// Maximum allowed records for one payload.
        max: usize,
    },
    /// A key or diagnostic key field contained a NUL byte.
    KeyContainsNull,
    /// The key exceeds the store's maximum key size.
    KeyTooLarge {
        /// Maximum allowed UTF-8 bytes.
        max: usize,
        /// Supplied UTF-8 bytes.
        actual: usize,
    },
    /// An insert or replacement would exceed the unique-key limit.
    MaxUniqueKeysExceeded {
        /// Maximum allowed distinct keys.
        max: usize,
    },
    /// A byte payload could not be written as unencoded UTF-8 text.
    PayloadNotUtf8,
    /// The requested payload encoding is not supported.
    UnsupportedEncoding {
        /// Requested encoding name.
        token: String,
    },
    /// The value exceeds the store's maximum value size.
    ValueTooLarge {
        /// Maximum allowed UTF-8 bytes.
        max: usize,
        /// Supplied UTF-8 bytes.
        actual: usize,
    },
    /// A stored value contained a NUL byte.
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
