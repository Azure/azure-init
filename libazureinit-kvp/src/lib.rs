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
use std::path::Path;

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
pub trait KvpStore: Send + Sync {
    /// Maximum key size in bytes for writes.
    fn max_key_size(&self) -> usize;

    /// Maximum value size in bytes for writes.
    fn max_value_size(&self) -> usize;

    /// Insert a new key-value pair or update an existing key's value.
    fn upsert(&self, key: &str, value: &str) -> Result<(), KvpError>;

    /// Read the value for a key. Returns `Ok(None)` when absent.
    fn read(&self, key: &str) -> Result<Option<String>, KvpError>;

    /// Remove a key. Returns `true` if the key was present.
    fn delete(&self, key: &str) -> Result<bool, KvpError>;

    /// Remove all entries from the store.
    fn clear(&self) -> Result<(), KvpError>;

    /// Return all key-value pairs.
    fn entries(&self) -> Result<HashMap<String, String>, KvpError>;

    /// Return the number of entries in the store.
    fn len(&self) -> Result<usize, KvpError>;

    /// Return whether the store is empty.
    fn is_empty(&self) -> Result<bool, KvpError>;

    /// Whether the store's data is stale (e.g. predates current boot).
    fn is_stale(&self) -> Result<bool, KvpError>;

    /// Dump all entries to a JSON file at the given path.
    fn dump(&self, path: &Path) -> Result<(), KvpError>;
}
