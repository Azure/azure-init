// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `libazureinit-kvp` provides a storage trait and Hyper-V-backed
//! implementation for KVP pool files.
//!
//! - [`KvpStore`]: storage interface used by higher layers.
//! - [`HyperVKvpStore`]: Hyper-V pool file implementation.
//! - [`AzureKvpStore`]: Azure-specific wrapper with stricter value limits.

use std::collections::HashMap;
use std::fmt;
use std::io;

pub mod azure;
pub mod hyperv;

/// Errors returned by [`KvpStore`] operations.
#[derive(Debug)]
pub enum KvpError {
    /// The key was empty.
    EmptyKey,
    /// The key exceeds the store's maximum key size.
    KeyTooLarge { max: usize, actual: usize },
    /// The value exceeds the store's maximum value size.
    ValueTooLarge { max: usize, actual: usize },
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

pub use azure::AzureKvpStore;
pub use hyperv::HyperVKvpStore;

/// Key-value store that supports Hyper-V KVP semantics while being
/// generic enough for non-file-backed in-memory implementations
/// (e.g. tests).
pub trait KvpStore: Send + Sync {
    /// Maximum key size in bytes for this store.
    const MAX_KEY_SIZE: usize;
    /// Maximum value size in bytes for this store.
    const MAX_VALUE_SIZE: usize;

    // -- Backend callouts for read/write (required) ---------------------

    /// Backend-specific read implementation.
    fn backend_read(&self, key: &str) -> Result<Option<String>, KvpError>;

    /// Backend-specific write implementation.
    fn backend_write(&self, key: &str, value: &str) -> Result<(), KvpError>;

    // -- Required methods (no shared validation wrapper) ---------------

    /// Return all key-value pairs, deduplicated with last-write-wins.
    fn entries(&self) -> Result<HashMap<String, String>, KvpError>;

    /// Return all raw key-value records without deduplication.
    ///
    /// Useful for testing or diagnostic dump commands where the full
    /// record history is needed.
    fn entries_raw(&self) -> Result<Vec<(String, String)>, KvpError>;

    /// Remove all records matching `key`.
    ///
    /// Returns `true` if at least one record was removed, `false` if
    /// the key was not found.
    fn delete(&self, key: &str) -> Result<bool, KvpError>;

    /// Backend-specific clear implementation (empty the store).
    fn backend_clear(&self) -> Result<(), KvpError>;

    // -- Public API with shared validation ----------------------------

    /// Empty the store, removing all records.
    fn clear(&self) -> Result<(), KvpError> {
        self.backend_clear()
    }

    /// Write a key-value pair into the store.
    ///
    /// # Errors
    ///
    /// Returns [`KvpError::EmptyKey`] if the key is empty,
    /// [`KvpError::KeyContainsNull`] if the key contains a null byte,
    /// [`KvpError::KeyTooLarge`] if the key exceeds [`Self::MAX_KEY_SIZE`],
    /// [`KvpError::ValueTooLarge`] if the value exceeds
    /// [`Self::MAX_VALUE_SIZE`], or [`KvpError::Io`] on I/O failure.
    fn write(&self, key: &str, value: &str) -> Result<(), KvpError> {
        Self::validate_key(key)?;
        Self::validate_value(value)?;
        self.backend_write(key, value)
    }

    /// Read the value for a given key, returning `None` if absent.
    ///
    /// When multiple records share the same key, the most recent value
    /// is returned (last-write-wins).
    fn read(&self, key: &str) -> Result<Option<String>, KvpError> {
        Self::validate_key(key)?;
        self.backend_read(key)
    }

    /// Whether the store's data is stale (e.g. predates the current
    /// boot). Defaults to `false`; file-backed stores can override.
    fn is_stale(&self) -> Result<bool, KvpError> {
        Ok(false)
    }

    // -- Validation helpers -------------------------------------------

    /// Validate a key against common constraints.
    ///
    /// Keys must be non-empty, must not contain null bytes (the on-disk
    /// format uses null-padding), and must not exceed
    /// [`Self::MAX_KEY_SIZE`] bytes.
    fn validate_key(key: &str) -> Result<(), KvpError> {
        if key.is_empty() {
            return Err(KvpError::EmptyKey);
        }
        if key.as_bytes().contains(&0) {
            return Err(KvpError::KeyContainsNull);
        }
        let actual = key.len();
        let max = Self::MAX_KEY_SIZE;
        if actual > max {
            return Err(KvpError::KeyTooLarge { max, actual });
        }
        Ok(())
    }

    /// Validate a value against the store's size limit.
    fn validate_value(value: &str) -> Result<(), KvpError> {
        let actual = value.len();
        let max = Self::MAX_VALUE_SIZE;
        if actual > max {
            return Err(KvpError::ValueTooLarge { max, actual });
        }
        Ok(())
    }
}
