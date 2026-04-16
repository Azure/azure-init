// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::io;

/// Errors returned by [`KvpStore`](crate::KvpStore) operations.
#[derive(Debug)]
pub enum KvpError {
    /// The key was empty.
    EmptyKey,
    /// The key exceeds the store's maximum key size.
    KeyTooLarge { max: usize, actual: usize },
    /// The value exceeds the store's maximum value size.
    ValueTooLarge { max: usize, actual: usize },
    /// The store already has the maximum allowed number of unique keys.
    MaxUniqueKeysExceeded { max: usize },
    /// The key contains a null byte, which is incompatible with the
    /// on-disk format (null-padded fixed-width fields).
    KeyContainsNull,
    /// An underlying I/O error.
    Io(io::Error),
}

impl fmt::Display for KvpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyKey => write!(f, "KVP key must not be empty"),
            Self::KeyTooLarge { max, actual } => {
                write!(f, "KVP key length ({actual}) exceeds maximum ({max})")
            }
            Self::ValueTooLarge { max, actual } => {
                write!(f, "KVP value length ({actual}) exceeds maximum ({max})")
            }
            Self::MaxUniqueKeysExceeded { max } => {
                write!(f, "KVP unique key count exceeded maximum ({max})")
            }
            Self::KeyContainsNull => {
                write!(f, "KVP key must not contain null bytes")
            }
            Self::Io(e) => write!(f, "{e}"),
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
