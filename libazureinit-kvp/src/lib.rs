// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `libazureinit-kvp` provides a storage trait and unified KVP pool
//! implementation for Hyper-V/Azure guests.
//!
//! - [`KvpStore`]: storage interface used by higher layers.
//! - [`KvpPoolStore`]: KVP pool file implementation with
//!   [`PoolMode`](kvp_pool::PoolMode)-based policy.

use std::collections::HashMap;
use std::fmt;
use std::io;

pub mod kvp_pool;

/// Errors returned by [`KvpStore`] operations.
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

pub use kvp_pool::KvpPoolStore;

/// Key-value store with Hyper-V KVP semantics.
///
/// The trait splits each operation into a `backend_*` method (raw I/O,
/// provided by the implementor) and a public method (`write`, `read`,
/// `clear`) that validates inputs then delegates to the backend.
pub trait KvpStore: Send + Sync {
    /// Maximum key size in bytes for this store.
    fn max_key_size(&self) -> usize;

    /// Maximum value size in bytes for this store.
    fn max_value_size(&self) -> usize;

    /// Raw write — persist a key-value pair without validation.
    fn backend_write(&self, key: &str, value: &str) -> Result<(), KvpError>;

    /// Raw read — look up a key without validation.
    fn backend_read(&self, key: &str) -> Result<Option<String>, KvpError>;

    /// Return all key-value pairs, deduplicated with last-write-wins.
    fn entries(&self) -> Result<HashMap<String, String>, KvpError>;

    /// Return all raw records in file order, without deduplication.
    fn entries_raw(&self) -> Result<Vec<(String, String)>, KvpError>;

    /// Remove all records matching `key`. Returns `true` if any were removed.
    fn delete(&self, key: &str) -> Result<bool, KvpError>;

    /// Raw clear — remove all records without additional checks.
    fn backend_clear(&self) -> Result<(), KvpError>;

    /// Write a key-value pair after validation.
    fn write(&self, key: &str, value: &str) -> Result<(), KvpError> {
        self.validate_key(key)?;
        self.validate_value(value)?;
        self.backend_write(key, value)
    }

    /// Read the most recent value for a key (last-write-wins).
    ///
    /// Returns `Ok(None)` when the key is absent.
    fn read(&self, key: &str) -> Result<Option<String>, KvpError> {
        self.validate_key(key)?;
        self.backend_read(key)
    }

    /// Remove all records from the store.
    fn clear(&self) -> Result<(), KvpError> {
        self.backend_clear()
    }

    /// Whether the store's data is stale (e.g. predates current boot).
    fn is_stale(&self) -> Result<bool, KvpError> {
        Ok(false)
    }

    /// Validate a key: must be non-empty, no null bytes, within
    /// [`max_key_size`](Self::max_key_size).
    fn validate_key(&self, key: &str) -> Result<(), KvpError> {
        if key.is_empty() {
            return Err(KvpError::EmptyKey);
        }
        if key.as_bytes().contains(&0) {
            return Err(KvpError::KeyContainsNull);
        }
        let actual = key.len();
        let max = self.max_key_size();
        if actual > max {
            return Err(KvpError::KeyTooLarge { max, actual });
        }
        Ok(())
    }

    /// Validate a value: must be within [`max_value_size`](Self::max_value_size).
    fn validate_value(&self, value: &str) -> Result<(), KvpError> {
        let actual = value.len();
        let max = self.max_value_size();
        if actual > max {
            return Err(KvpError::ValueTooLarge { max, actual });
        }
        Ok(())
    }
}
