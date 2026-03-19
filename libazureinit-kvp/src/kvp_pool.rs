// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unified KVP pool file backend for Hyper-V and Azure guests.
//!
//! The on-disk pool format is fixed-width and identical across
//! environments:
//! - key field: 512 bytes
//! - value field: 2048 bytes
//! - record size: 2560 bytes
//!
//! [`PoolMode`] selects which size limits are enforced on writes:
//! - [`Restricted`](PoolMode::Restricted) (default): key <= 254 bytes,
//!   value <= 1022 bytes
//! - [`Full`](PoolMode::Full): key <= 512 bytes, value <= 2048 bytes
//!
//! ## Reference
//! - [Hyper-V Data Exchange Service (KVP)](https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/reference/integration-services#hyper-v-data-exchange-service-kvp)

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use sysinfo::System;

use crate::{KvpError, KvpStore};

/// Default KVP pool path when none is provided.
pub const DEFAULT_KVP_POOL_PATH: &str = "/var/lib/hyperv/.kvp_pool_1";

const WIRE_MAX_KEY_BYTES: usize = 512;
const WIRE_MAX_VALUE_BYTES: usize = 2048;
const SAFE_MAX_KEY_BYTES: usize = 254;
const SAFE_MAX_VALUE_BYTES: usize = 1022;

/// Maximum number of unique keys allowed in the pool.
const MAX_UNIQUE_KEYS: usize = 1024;

const RECORD_SIZE: usize = WIRE_MAX_KEY_BYTES + WIRE_MAX_VALUE_BYTES;

/// Policy mode controlling key/value size limits for writes.
///
/// The on-disk record format is always 512 + 2048 bytes regardless of
/// mode; the mode only determines the validation ceiling for incoming
/// writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolMode {
    /// Conservative limits for Azure compatibility
    /// (key <= 254 bytes, value <= 1022 bytes).
    Restricted,
    /// Full Hyper-V wire-format limits
    /// (key <= 512 bytes, value <= 2048 bytes).
    Full,
}

impl PoolMode {
    fn max_key_size(self) -> usize {
        match self {
            Self::Restricted => SAFE_MAX_KEY_BYTES,
            Self::Full => WIRE_MAX_KEY_BYTES,
        }
    }

    fn max_value_size(self) -> usize {
        match self {
            Self::Restricted => SAFE_MAX_VALUE_BYTES,
            Self::Full => WIRE_MAX_VALUE_BYTES,
        }
    }
}

/// Unified KVP pool file store.
#[derive(Clone, Debug)]
pub struct KvpPoolStore {
    path: PathBuf,
    mode: PoolMode,
}

impl KvpPoolStore {
    /// Create a new KVP pool store.
    ///
    /// - `path`: pool file path, defaults to [`DEFAULT_KVP_POOL_PATH`].
    /// - `mode`: size-limit policy (see [`PoolMode`]).
    /// - `truncate_on_stale`: if `true`, clears stale data from a
    ///   previous boot.
    pub fn new(
        path: Option<PathBuf>,
        mode: PoolMode,
        truncate_on_stale: bool,
    ) -> Result<Self, KvpError> {
        let store = Self {
            path: path.unwrap_or_else(|| PathBuf::from(DEFAULT_KVP_POOL_PATH)),
            mode,
        };
        if truncate_on_stale && store.pool_is_stale()? {
            store.truncate_pool()?;
        }
        Ok(store)
    }

    /// Return a reference to the pool path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The policy mode this store was created with.
    pub fn mode(&self) -> PoolMode {
        self.mode
    }

    fn boot_time() -> Result<i64, KvpError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| io::Error::other(format!("clock error: {e}")))?
            .as_secs();
        Ok(now.saturating_sub(get_uptime().as_secs()) as i64)
    }

    fn pool_is_stale(&self) -> Result<bool, KvpError> {
        let metadata = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(ref e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let boot = Self::boot_time()?;
        Ok(metadata.mtime() < boot)
    }

    #[cfg(test)]
    fn pool_is_stale_at_boot(&self, boot_time: i64) -> Result<bool, KvpError> {
        let metadata = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(ref e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        Ok(metadata.mtime() < boot_time)
    }

    fn truncate_pool(&self) -> Result<(), KvpError> {
        let file =
            match OpenOptions::new().read(true).write(true).open(&self.path) {
                Ok(f) => f,
                Err(ref e) if e.kind() == ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.into()),
            };

        FileExt::lock_exclusive(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;
        let result = file.set_len(0).map_err(KvpError::from);
        let _ = FileExt::unlock(&file);
        result
    }

    fn open_for_read(&self) -> io::Result<File> {
        OpenOptions::new().read(true).open(&self.path)
    }

    fn open_for_read_write(&self) -> io::Result<File> {
        OpenOptions::new().read(true).write(true).open(&self.path)
    }

    fn open_for_read_append_create(&self) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&self.path)
    }
}

pub(crate) fn encode_record(key: &str, value: &str) -> Vec<u8> {
    let mut buf = vec![0u8; RECORD_SIZE];

    let key_bytes = key.as_bytes();
    let key_len = key_bytes.len().min(WIRE_MAX_KEY_BYTES);
    buf[..key_len].copy_from_slice(&key_bytes[..key_len]);

    let value_bytes = value.as_bytes();
    let value_len = value_bytes.len().min(WIRE_MAX_VALUE_BYTES);
    buf[WIRE_MAX_KEY_BYTES..WIRE_MAX_KEY_BYTES + value_len]
        .copy_from_slice(&value_bytes[..value_len]);

    buf
}

pub(crate) fn decode_record(data: &[u8]) -> io::Result<(String, String)> {
    if data.len() != RECORD_SIZE {
        return Err(io::Error::other(format!(
            "record size mismatch: expected {RECORD_SIZE}, got {}",
            data.len()
        )));
    }

    let (key_bytes, value_bytes) = data.split_at(WIRE_MAX_KEY_BYTES);

    let key = std::str::from_utf8(key_bytes)
        .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?
        .trim_end_matches('\0')
        .to_string();
    let value = std::str::from_utf8(value_bytes)
        .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?
        .trim_end_matches('\0')
        .to_string();

    Ok((key, value))
}

fn read_all_records(file: &mut File) -> io::Result<Vec<(String, String)>> {
    let len = file.metadata()?.len() as usize;
    if len == 0 {
        return Ok(Vec::new());
    }

    if len % RECORD_SIZE != 0 {
        return Err(io::Error::other(format!(
            "file size ({len}) is not a multiple of record size ({RECORD_SIZE})"
        )));
    }

    file.seek(io::SeekFrom::Start(0))?;
    let record_count = len / RECORD_SIZE;
    let mut records = Vec::with_capacity(record_count);
    let mut buf = [0u8; RECORD_SIZE];

    for _ in 0..record_count {
        file.read_exact(&mut buf)?;
        records.push(decode_record(&buf)?);
    }

    Ok(records)
}

impl KvpStore for KvpPoolStore {
    fn max_key_size(&self) -> usize {
        self.mode.max_key_size()
    }

    fn max_value_size(&self) -> usize {
        self.mode.max_value_size()
    }

    fn backend_write(&self, key: &str, value: &str) -> Result<(), KvpError> {
        let mut file = self.open_for_read_append_create()?;
        FileExt::lock_exclusive(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;

        let result = (|| -> Result<(), KvpError> {
            let records = read_all_records(&mut file)?;
            let mut unique_keys = HashSet::with_capacity(records.len());
            for (record_key, _) in records {
                unique_keys.insert(record_key);
            }

            if !unique_keys.contains(key)
                && unique_keys.len() >= MAX_UNIQUE_KEYS
            {
                return Err(KvpError::MaxUniqueKeysExceeded {
                    max: MAX_UNIQUE_KEYS,
                });
            }

            let record = encode_record(key, value);
            file.write_all(&record)?;
            file.flush()?;
            Ok(())
        })();

        let _ = FileExt::unlock(&file);
        result
    }

    fn backend_read(&self, key: &str) -> Result<Option<String>, KvpError> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        FileExt::lock_shared(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;
        let records = read_all_records(&mut file);
        let _ = FileExt::unlock(&file);
        let records = records?;

        Ok(records
            .into_iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v))
    }

    fn entries(&self) -> Result<HashMap<String, String>, KvpError> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(HashMap::new())
            }
            Err(e) => return Err(e.into()),
        };

        FileExt::lock_shared(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;
        let records = read_all_records(&mut file);
        let _ = FileExt::unlock(&file);
        let records = records?;

        Ok(records.into_iter().collect())
    }

    fn entries_raw(&self) -> Result<Vec<(String, String)>, KvpError> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(Vec::new())
            }
            Err(e) => return Err(e.into()),
        };

        FileExt::lock_shared(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;
        let records = read_all_records(&mut file);
        let _ = FileExt::unlock(&file);
        Ok(records?)
    }

    fn delete(&self, key: &str) -> Result<bool, KvpError> {
        let mut file = match self.open_for_read_write() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };

        FileExt::lock_exclusive(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;

        let result = (|| -> Result<bool, KvpError> {
            let records = read_all_records(&mut file)?;
            let original_count = records.len();
            let kept: Vec<_> =
                records.into_iter().filter(|(k, _)| k != key).collect();

            if kept.len() == original_count {
                return Ok(false);
            }

            file.set_len(0)?;
            file.seek(io::SeekFrom::Start(0))?;
            for (k, v) in &kept {
                file.write_all(&encode_record(k, v))?;
            }
            file.flush()?;
            Ok(true)
        })();

        let _ = FileExt::unlock(&file);
        result
    }

    fn backend_clear(&self) -> Result<(), KvpError> {
        self.truncate_pool()
    }

    fn is_stale(&self) -> Result<bool, KvpError> {
        self.pool_is_stale()
    }
}

fn get_uptime() -> Duration {
    Duration::from_secs(System::uptime())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn restricted_store(path: &Path) -> KvpPoolStore {
        KvpPoolStore::new(Some(path.to_path_buf()), PoolMode::Restricted, false)
            .unwrap()
    }

    fn full_store(path: &Path) -> KvpPoolStore {
        KvpPoolStore::new(Some(path.to_path_buf()), PoolMode::Full, false)
            .unwrap()
    }

    #[test]
    fn test_write_rejects_empty_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        let err = store.write("", "value").unwrap_err();
        assert!(matches!(err, KvpError::EmptyKey));
    }

    #[test]
    fn test_write_rejects_null_in_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        let err = store.write("bad\0key", "value").unwrap_err();
        assert!(matches!(err, KvpError::KeyContainsNull));
    }

    #[test]
    fn test_default_path_is_used() {
        let store =
            KvpPoolStore::new(None, PoolMode::Restricted, false).unwrap();
        assert_eq!(store.path(), Path::new(DEFAULT_KVP_POOL_PATH));
    }

    #[test]
    fn test_explicit_path_is_used() {
        let tmp = NamedTempFile::new().unwrap();
        let store = KvpPoolStore::new(
            Some(tmp.path().to_path_buf()),
            PoolMode::Restricted,
            false,
        )
        .unwrap();
        assert_eq!(store.path(), tmp.path());
    }

    #[test]
    fn test_restricted_limits_key_254_pass_255_fail() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        let ok_key = "k".repeat(254);
        store.write(&ok_key, "v").unwrap();

        let bad_key = "k".repeat(255);
        let err = store.write(&bad_key, "v").unwrap_err();
        assert!(matches!(err, KvpError::KeyTooLarge { .. }));
    }

    #[test]
    fn test_restricted_limits_value_1022_pass_1023_fail() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        let ok_val = "v".repeat(1022);
        store.write("k", &ok_val).unwrap();

        let bad_val = "v".repeat(1023);
        let err = store.write("k2", &bad_val).unwrap_err();
        assert!(matches!(err, KvpError::ValueTooLarge { .. }));
    }

    #[test]
    fn test_full_limits_key_512_pass_513_fail() {
        let tmp = NamedTempFile::new().unwrap();
        let store = full_store(tmp.path());

        let ok_key = "k".repeat(512);
        store.write(&ok_key, "v").unwrap();

        let bad_key = "k".repeat(513);
        let err = store.write(&bad_key, "v").unwrap_err();
        assert!(matches!(err, KvpError::KeyTooLarge { .. }));
    }

    #[test]
    fn test_full_limits_value_2048_pass_2049_fail() {
        let tmp = NamedTempFile::new().unwrap();
        let store = full_store(tmp.path());

        let ok_val = "v".repeat(2048);
        store.write("k", &ok_val).unwrap();

        let bad_val = "v".repeat(2049);
        let err = store.write("k2", &bad_val).unwrap_err();
        assert!(matches!(err, KvpError::ValueTooLarge { .. }));
    }

    #[test]
    fn test_entries_raw_preserves_duplicates() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        store.write("key", "v1").unwrap();
        store.write("key", "v2").unwrap();
        store.write("other", "v3").unwrap();

        let raw = store.entries_raw().unwrap();
        assert_eq!(raw.len(), 3);
        assert_eq!(raw[0], ("key".to_string(), "v1".to_string()));
        assert_eq!(raw[1], ("key".to_string(), "v2".to_string()));
        assert_eq!(raw[2], ("other".to_string(), "v3".to_string()));
    }

    #[test]
    fn test_entries_deduplicates_last_write_wins() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        store.write("key", "v1").unwrap();
        store.write("key", "v2").unwrap();
        store.write("other", "v3").unwrap();

        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.get("key"), Some(&"v2".to_string()));
        assert_eq!(entries.get("other"), Some(&"v3".to_string()));
    }

    #[test]
    fn test_unique_key_cap_allows_1024_then_rejects_1025th() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        for i in 0..MAX_UNIQUE_KEYS {
            store.write(&format!("k{i}"), "v").unwrap();
        }
        let err = store.write("overflow", "v").unwrap_err();
        assert!(matches!(err, KvpError::MaxUniqueKeysExceeded { .. }));
    }

    #[test]
    fn test_unique_key_cap_allows_overwrite_at_limit() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        for i in 0..MAX_UNIQUE_KEYS {
            store.write(&format!("k{i}"), "v").unwrap();
        }
        store.write("k0", "updated").unwrap();
        assert_eq!(store.read("k0").unwrap(), Some("updated".to_string()));
    }

    #[test]
    fn test_unique_key_cap_allows_new_key_after_delete() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        for i in 0..MAX_UNIQUE_KEYS {
            store.write(&format!("k{i}"), "v").unwrap();
        }
        assert!(store.delete("k0").unwrap());
        store.write("new-key", "v").unwrap();
    }

    #[test]
    fn test_clear_empties_store() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        store.write("key", "value").unwrap();
        store.clear().unwrap();
        assert_eq!(store.read("key").unwrap(), None);
    }

    #[test]
    fn test_delete_removes_all_matching_records() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        store.write("key", "v1").unwrap();
        store.write("key", "v2").unwrap();
        store.write("other", "v3").unwrap();

        assert!(store.delete("key").unwrap());
        assert_eq!(store.read("key").unwrap(), None);
        assert_eq!(store.read("other").unwrap(), Some("v3".to_string()));
    }

    #[test]
    fn test_is_stale_and_pool_is_stale_at_boot_helpers() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());
        store.write("key", "value").unwrap();

        assert!(!store.is_stale().unwrap());
        assert!(store.pool_is_stale_at_boot(i64::MAX).unwrap());
        assert!(!store.pool_is_stale_at_boot(0).unwrap());
    }

    #[test]
    fn test_mode_getter() {
        let tmp = NamedTempFile::new().unwrap();
        let restricted = restricted_store(tmp.path());
        assert_eq!(restricted.mode(), PoolMode::Restricted);

        let tmp2 = NamedTempFile::new().unwrap();
        let full = full_store(tmp2.path());
        assert_eq!(full.mode(), PoolMode::Full);
    }
}
