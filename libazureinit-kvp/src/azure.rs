// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Azure-specific KVP store.
//!
//! Wraps [`HyperVKvpStore`] with the stricter value-size limit imposed
//! by the Azure host (1,022 bytes).  All other behavior — record
//! format, file locking, append-only writes — is inherited from the
//! underlying Hyper-V pool file implementation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::hyperv::HyperVKvpStore;
use crate::{KvpError, KvpStore};

/// Azure host-side value limit (values beyond this are truncated).
const AZURE_MAX_VALUE_BYTES: usize = 1022;

/// Azure KVP store backed by a Hyper-V pool file.
///
/// Identical to [`HyperVKvpStore`] except that
/// [`MAX_VALUE_SIZE`](KvpStore::MAX_VALUE_SIZE) is set to 1,022 bytes,
/// matching the Azure host's truncation behavior.
#[derive(Clone, Debug)]
pub struct AzureKvpStore {
    inner: HyperVKvpStore,
}

impl AzureKvpStore {
    /// Create a new Azure KVP store backed by the pool file at `path`.
    ///
    /// When `truncate_on_stale` is `true` the constructor checks
    /// whether the pool file predates the current boot and, if so,
    /// truncates it before returning.
    pub fn new(
        path: impl Into<PathBuf>,
        truncate_on_stale: bool,
    ) -> Result<Self, KvpError> {
        Ok(Self {
            inner: HyperVKvpStore::new(path, truncate_on_stale)?,
        })
    }

    /// Return a reference to the pool file path.
    pub fn path(&self) -> &Path {
        self.inner.path()
    }
}

impl KvpStore for AzureKvpStore {
    const MAX_KEY_SIZE: usize = HyperVKvpStore::MAX_KEY_SIZE;
    const MAX_VALUE_SIZE: usize = AZURE_MAX_VALUE_BYTES;

    fn backend_read(&self, key: &str) -> Result<Option<String>, KvpError> {
        self.inner.backend_read(key)
    }

    fn backend_write(&self, key: &str, value: &str) -> Result<(), KvpError> {
        self.inner.backend_write(key, value)
    }

    fn entries(&self) -> Result<HashMap<String, String>, KvpError> {
        self.inner.entries()
    }

    fn entries_raw(&self) -> Result<Vec<(String, String)>, KvpError> {
        self.inner.entries_raw()
    }

    fn delete(&self, key: &str) -> Result<bool, KvpError> {
        self.inner.delete(key)
    }

    fn backend_clear(&self) -> Result<(), KvpError> {
        self.inner.backend_clear()
    }

    fn is_stale(&self) -> Result<bool, KvpError> {
        self.inner.is_stale()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn azure_store(path: &Path) -> AzureKvpStore {
        AzureKvpStore::new(path, false).unwrap()
    }

    #[test]
    fn test_azure_rejects_value_over_1022() {
        let tmp = NamedTempFile::new().unwrap();
        let store = azure_store(tmp.path());

        let value = "V".repeat(AZURE_MAX_VALUE_BYTES + 1);
        let err = store.write("k", &value).unwrap_err();
        assert!(
            matches!(err, KvpError::ValueTooLarge { .. }),
            "expected ValueTooLarge, got: {err}"
        );
    }

    #[test]
    fn test_azure_accepts_value_at_1022() {
        let tmp = NamedTempFile::new().unwrap();
        let store = azure_store(tmp.path());

        let value = "V".repeat(AZURE_MAX_VALUE_BYTES);
        store.write("k", &value).unwrap();
        assert_eq!(store.read("k").unwrap(), Some(value));
    }

    #[test]
    fn test_azure_write_and_read() {
        let tmp = NamedTempFile::new().unwrap();
        let store = azure_store(tmp.path());

        store.write("key", "value").unwrap();
        assert_eq!(store.read("key").unwrap(), Some("value".to_string()));
    }

    #[test]
    fn test_azure_clear() {
        let tmp = NamedTempFile::new().unwrap();
        let store = azure_store(tmp.path());

        store.write("key", "value").unwrap();
        store.clear().unwrap();
        assert_eq!(store.read("key").unwrap(), None);
    }
}
