// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `libazureinit-kvp` provides a storage trait and Hyper-V-backed
//! implementation for KVP pool files.
//!
//! - [`KvpStore`]: storage interface used by higher layers.
//! - [`HyperVKvpStore`]: production implementation.
//! - [`KvpLimits`]: exported Hyper-V and Azure byte limits.

use std::collections::HashMap;
use std::io;

pub mod hyperv;

pub use hyperv::HyperVKvpStore;

/// Hyper-V key limit in bytes (policy/default preset).
pub const HYPERV_MAX_KEY_BYTES: usize = 512;
/// Hyper-V value limit in bytes (policy/default preset).
pub const HYPERV_MAX_VALUE_BYTES: usize = 2048;
/// Azure key limit in bytes (policy/default preset).
pub const AZURE_MAX_KEY_BYTES: usize = 512;
/// Azure value limit in bytes (UTF-16: 511 characters + null terminator).
pub const AZURE_MAX_VALUE_BYTES: usize = 1022;

/// Storage abstraction for KVP backends.
///
/// Semantics:
/// - `write`: stores one key/value or returns validation/I/O error.
/// - `read`: returns the most recent value for a key (last-write-wins).
/// - `entries`: returns deduplicated key/value pairs as `HashMap`.
/// - `delete`: removes all records for a key and reports whether any were removed.
/// - `limits`: returns the [`KvpLimits`] that govern maximum key/value
///   sizes for this store, allowing consumers to chunk or validate
///   data generically.
pub trait KvpStore: Send + Sync {
    /// The key and value byte-size limits for this store.
    ///
    /// Consumers (e.g. diagnostics, tracing layers) should call this
    /// instead of hardcoding size constants, so the limits stay correct
    /// regardless of the underlying implementation.
    fn limits(&self) -> KvpLimits;

    /// Write a key-value pair into the store.
    ///
    /// Returns an error if:
    /// - The key is empty.
    /// - The key exceeds the configured maximum key size.
    /// - The value exceeds the configured maximum value size.
    /// - An I/O error occurs during the write.
    fn write(&self, key: &str, value: &str) -> io::Result<()>;

    /// Read the value for a given key, returning `None` if absent.
    ///
    /// When multiple records exist for the same key (append-only
    /// storage), the value from the most recent record is returned
    /// (last-write-wins).
    fn read(&self, key: &str) -> io::Result<Option<String>>;

    /// Return all key-value pairs currently in the store.
    ///
    /// Keys are deduplicated using last-write-wins semantics, matching
    /// the behavior of [`read`](KvpStore::read).
    fn entries(&self) -> io::Result<HashMap<String, String>>;

    /// Remove all records matching `key`.
    ///
    /// Returns `true` if at least one record was removed, `false` if
    /// the key was not found.
    fn delete(&self, key: &str) -> io::Result<bool>;
}

/// Configurable key/value byte limits for writes.
///
/// Presets:
/// - [`KvpLimits::hyperv`]: [`HYPERV_MAX_KEY_BYTES`] /
///   [`HYPERV_MAX_VALUE_BYTES`].
/// - [`KvpLimits::azure`]: [`AZURE_MAX_KEY_BYTES`] /
///   [`AZURE_MAX_VALUE_BYTES`].
///
/// Use `azure()` for Azure guests, where host-side consumers are stricter
/// on value byte length than raw Hyper-V format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvpLimits {
    pub max_key_size: usize,
    pub max_value_size: usize,
}

impl KvpLimits {
    /// Raw Hyper-V wire format limits.
    ///
    /// - Max key size: 512 bytes
    /// - Max value size: 2,048 bytes
    ///
    /// Use this when writing to a Hyper-V KVP pool file that will only
    /// be consumed by Hyper-V tooling (not the Azure host agent).
    pub const fn hyperv() -> Self {
        Self {
            max_key_size: HYPERV_MAX_KEY_BYTES,
            max_value_size: HYPERV_MAX_VALUE_BYTES,
        }
    }

    /// Azure platform limits.
    ///
    /// - Max key size: 512 bytes
    /// - Max value size: 1,022 bytes (UTF-16: 511 characters + null
    ///   terminator)
    ///
    /// The Azure host agent reads KVP records from the guest but is
    /// stricter than the underlying Hyper-V format. Values beyond
    /// 1,022 bytes are silently truncated by the host. Use this preset
    /// for any code running on Azure VMs.
    pub const fn azure() -> Self {
        Self {
            max_key_size: AZURE_MAX_KEY_BYTES,
            max_value_size: AZURE_MAX_VALUE_BYTES,
        }
    }
}
