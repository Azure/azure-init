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

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Acquire an exclusive (write) lock on the entire file.
///
/// Uses `fcntl(F_OFD_SETLKW)` — open file description locks that are
/// per-FD (safe for multi-threaded use) yet conflict with traditional
/// `fcntl` record locks used by `hv_kvp_daemon.c` and cloud-init.
fn fcntl_lock_exclusive(file: &File) -> io::Result<()> {
    let fl = libc::flock {
        l_type: libc::F_WRLCK as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLKW, &fl) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Acquire a shared (read) lock on the entire file.
fn fcntl_lock_shared(file: &File) -> io::Result<()> {
    let fl = libc::flock {
        l_type: libc::F_RDLCK as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLKW, &fl) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Release a lock on the entire file.
fn fcntl_unlock(file: &File) -> io::Result<()> {
    let fl = libc::flock {
        l_type: libc::F_UNLCK as libc::c_short,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: 0,
        l_len: 0,
        l_pid: 0,
    };
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &fl) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Policy mode controlling key/value size limits for writes.
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

    fn validate_key(&self, key: &str) -> Result<(), KvpError> {
        if key.is_empty() {
            return Err(KvpError::EmptyKey);
        }
        if key.as_bytes().contains(&0) {
            return Err(KvpError::KeyContainsNull);
        }
        let actual = key.len();
        let max = self.mode.max_key_size();
        if actual > max {
            return Err(KvpError::KeyTooLarge { max, actual });
        }
        Ok(())
    }

    /// Looser validation for reads: accepts keys up to the full
    /// wire-format maximum regardless of [`PoolMode`].
    fn validate_key_for_read(key: &str) -> Result<(), KvpError> {
        if key.is_empty() {
            return Err(KvpError::EmptyKey);
        }
        if key.as_bytes().contains(&0) {
            return Err(KvpError::KeyContainsNull);
        }
        let actual = key.len();
        if actual > WIRE_MAX_KEY_BYTES {
            return Err(KvpError::KeyTooLarge {
                max: WIRE_MAX_KEY_BYTES,
                actual,
            });
        }
        Ok(())
    }

    fn validate_value(&self, value: &str) -> Result<(), KvpError> {
        let actual = value.len();
        let max = self.mode.max_value_size();
        if actual > max {
            return Err(KvpError::ValueTooLarge { max, actual });
        }
        Ok(())
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

        fcntl_lock_exclusive(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;
        let result = file.set_len(0).map_err(KvpError::from);
        let _ = fcntl_unlock(&file);
        result
    }

    fn open_for_read(&self) -> io::Result<File> {
        OpenOptions::new().read(true).open(&self.path)
    }

    fn open_for_read_write(&self) -> io::Result<File> {
        OpenOptions::new().read(true).write(true).open(&self.path)
    }

    fn open_for_read_write_create(&self) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
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

    if !len.is_multiple_of(RECORD_SIZE) {
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

fn rewrite_file(
    file: &mut File,
    map: &HashMap<String, String>,
) -> Result<(), KvpError> {
    file.set_len(0)?;
    file.seek(io::SeekFrom::Start(0))?;
    for (k, v) in map {
        file.write_all(&encode_record(k, v))?;
    }
    file.flush()?;
    Ok(())
}

impl KvpStore for KvpPoolStore {
    fn max_key_size(&self) -> usize {
        self.mode.max_key_size()
    }

    fn max_value_size(&self) -> usize {
        self.mode.max_value_size()
    }

    fn upsert(&self, key: &str, value: &str) -> Result<(), KvpError> {
        self.validate_key(key)?;
        self.validate_value(value)?;

        let mut file = self.open_for_read_write_create()?;
        fcntl_lock_exclusive(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;

        let result = (|| -> Result<(), KvpError> {
            let records = read_all_records(&mut file)?;
            let mut map: HashMap<String, String> =
                records.into_iter().collect();

            if !map.contains_key(key) && map.len() >= MAX_UNIQUE_KEYS {
                return Err(KvpError::MaxUniqueKeysExceeded {
                    max: MAX_UNIQUE_KEYS,
                });
            }

            map.insert(key.to_string(), value.to_string());
            rewrite_file(&mut file, &map)?;
            Ok(())
        })();

        let _ = fcntl_unlock(&file);
        result
    }

    fn read(&self, key: &str) -> Result<Option<String>, KvpError> {
        Self::validate_key_for_read(key)?;

        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        fcntl_lock_shared(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;
        let records = read_all_records(&mut file);
        let _ = fcntl_unlock(&file);
        let records = records?;

        Ok(records.into_iter().find(|(k, _)| k == key).map(|(_, v)| v))
    }

    fn entries(&self) -> Result<HashMap<String, String>, KvpError> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(HashMap::new())
            }
            Err(e) => return Err(e.into()),
        };

        fcntl_lock_shared(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;
        let records = read_all_records(&mut file);
        let _ = fcntl_unlock(&file);
        let records = records?;

        Ok(records.into_iter().collect())
    }

    fn delete(&self, key: &str) -> Result<bool, KvpError> {
        let mut file = match self.open_for_read_write() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };

        fcntl_lock_exclusive(&file).map_err(|e| {
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

        let _ = fcntl_unlock(&file);
        result
    }

    fn clear(&self) -> Result<(), KvpError> {
        self.truncate_pool()
    }

    fn len(&self) -> Result<usize, KvpError> {
        match std::fs::metadata(&self.path) {
            Ok(m) => Ok(m.len() as usize / RECORD_SIZE),
            Err(ref e) if e.kind() == ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e.into()),
        }
    }

    fn is_empty(&self) -> Result<bool, KvpError> {
        match std::fs::metadata(&self.path) {
            Ok(m) => Ok(m.len() == 0),
            Err(ref e) if e.kind() == ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e.into()),
        }
    }

    fn is_stale(&self) -> Result<bool, KvpError> {
        self.pool_is_stale()
    }

    fn dump(&self, path: &Path) -> Result<(), KvpError> {
        let entries = self.entries()?;
        let json = serde_json::to_string_pretty(&entries).map_err(|e| {
            io::Error::other(format!("JSON serialization failed: {e}"))
        })?;
        std::fs::write(path, json)?;
        Ok(())
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
    fn test_upsert_rejects_empty_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        let err = store.upsert("", "value").unwrap_err();
        assert!(matches!(err, KvpError::EmptyKey));
    }

    #[test]
    fn test_upsert_rejects_null_in_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        let err = store.upsert("bad\0key", "value").unwrap_err();
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
        store.upsert(&ok_key, "v").unwrap();

        let bad_key = "k".repeat(255);
        let err = store.upsert(&bad_key, "v").unwrap_err();
        assert!(matches!(err, KvpError::KeyTooLarge { .. }));
    }

    #[test]
    fn test_restricted_limits_value_1022_pass_1023_fail() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        let ok_val = "v".repeat(1022);
        store.upsert("k", &ok_val).unwrap();

        let bad_val = "v".repeat(1023);
        let err = store.upsert("k2", &bad_val).unwrap_err();
        assert!(matches!(err, KvpError::ValueTooLarge { .. }));
    }

    #[test]
    fn test_full_limits_key_512_pass_513_fail() {
        let tmp = NamedTempFile::new().unwrap();
        let store = full_store(tmp.path());

        let ok_key = "k".repeat(512);
        store.upsert(&ok_key, "v").unwrap();

        let bad_key = "k".repeat(513);
        let err = store.upsert(&bad_key, "v").unwrap_err();
        assert!(matches!(err, KvpError::KeyTooLarge { .. }));
    }

    #[test]
    fn test_full_limits_value_2048_pass_2049_fail() {
        let tmp = NamedTempFile::new().unwrap();
        let store = full_store(tmp.path());

        let ok_val = "v".repeat(2048);
        store.upsert("k", &ok_val).unwrap();

        let bad_val = "v".repeat(2049);
        let err = store.upsert("k2", &bad_val).unwrap_err();
        assert!(matches!(err, KvpError::ValueTooLarge { .. }));
    }

    #[test]
    fn test_upsert_overwrites_existing_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        store.upsert("key", "v1").unwrap();
        store.upsert("key", "v2").unwrap();
        store.upsert("other", "v3").unwrap();

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
            store.upsert(&format!("k{i}"), "v").unwrap();
        }
        let err = store.upsert("overflow", "v").unwrap_err();
        assert!(matches!(err, KvpError::MaxUniqueKeysExceeded { .. }));
    }

    #[test]
    fn test_unique_key_cap_allows_overwrite_at_limit() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        for i in 0..MAX_UNIQUE_KEYS {
            store.upsert(&format!("k{i}"), "v").unwrap();
        }
        store.upsert("k0", "updated").unwrap();
        assert_eq!(store.read("k0").unwrap(), Some("updated".to_string()));
    }

    #[test]
    fn test_unique_key_cap_allows_new_key_after_delete() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        for i in 0..MAX_UNIQUE_KEYS {
            store.upsert(&format!("k{i}"), "v").unwrap();
        }
        assert!(store.delete("k0").unwrap());
        store.upsert("new-key", "v").unwrap();
    }

    #[test]
    fn test_clear_empties_store() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        store.upsert("key", "value").unwrap();
        store.clear().unwrap();
        assert_eq!(store.read("key").unwrap(), None);
    }

    #[test]
    fn test_delete_removes_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        store.upsert("key", "v1").unwrap();
        store.upsert("other", "v2").unwrap();

        assert!(store.delete("key").unwrap());
        assert_eq!(store.read("key").unwrap(), None);
        assert_eq!(store.read("other").unwrap(), Some("v2".to_string()));
    }

    #[test]
    fn test_is_stale_and_pool_is_stale_at_boot_helpers() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());
        store.upsert("key", "value").unwrap();

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

    #[test]
    fn test_len_and_is_empty() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        assert!(store.is_empty().unwrap());
        assert_eq!(store.len().unwrap(), 0);

        store.upsert("key", "value").unwrap();
        assert!(!store.is_empty().unwrap());
        assert_eq!(store.len().unwrap(), 1);

        store.upsert("key2", "value2").unwrap();
        assert_eq!(store.len().unwrap(), 2);

        store.upsert("key", "updated").unwrap();
        assert_eq!(store.len().unwrap(), 2);
    }

    #[test]
    fn test_read_accepts_wire_max_key_in_restricted_mode() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        // Write a record using Full mode so a 300-byte key is accepted
        let full = full_store(tmp.path());
        let long_key = "k".repeat(300);
        full.upsert(&long_key, "val").unwrap();

        // Restricted store can still read the 300-byte key (> 254 safe limit)
        assert_eq!(store.read(&long_key).unwrap(), Some("val".to_string()));

        // But a key beyond the wire max (512) is rejected even for reads
        let too_long = "k".repeat(513);
        let err = store.read(&too_long).unwrap_err();
        assert!(matches!(err, KvpError::KeyTooLarge { .. }));
    }

    #[test]
    fn test_dump_writes_json() {
        let tmp = NamedTempFile::new().unwrap();
        let store = restricted_store(tmp.path());

        store.upsert("key1", "value1").unwrap();
        store.upsert("key2", "value2").unwrap();

        let dump_file = NamedTempFile::new().unwrap();
        store.dump(dump_file.path()).unwrap();

        let contents = std::fs::read_to_string(dump_file.path()).unwrap();
        let parsed: HashMap<String, String> =
            serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.get("key1"), Some(&"value1".to_string()));
        assert_eq!(parsed.get("key2"), Some(&"value2".to_string()));
    }

    #[test]
    fn test_concurrent_upserts_to_different_keys() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let p = path.clone();
                std::thread::spawn(move || {
                    let store =
                        KvpPoolStore::new(Some(p), PoolMode::Restricted, false)
                            .unwrap();
                    for i in 0..10 {
                        let key = format!("t{t}_k{i}");
                        store.upsert(&key, &format!("val_{t}_{i}")).unwrap();
                    }
                })
            })
            .collect();

        for t in threads {
            t.join().unwrap();
        }

        let store = restricted_store(tmp.path());
        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 80);
        for t in 0..8 {
            for i in 0..10 {
                let key = format!("t{t}_k{i}");
                assert_eq!(
                    entries.get(&key),
                    Some(&format!("val_{t}_{i}")),
                    "missing or wrong value for {key}"
                );
            }
        }
    }

    #[test]
    fn test_concurrent_upserts_to_same_key() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let p = path.clone();
                std::thread::spawn(move || {
                    let store =
                        KvpPoolStore::new(Some(p), PoolMode::Restricted, false)
                            .unwrap();
                    for i in 0..10 {
                        store
                            .upsert("shared_key", &format!("t{t}_v{i}"))
                            .unwrap();
                    }
                })
            })
            .collect();

        for t in threads {
            t.join().unwrap();
        }

        let store = restricted_store(tmp.path());
        assert_eq!(store.len().unwrap(), 1);
        let val = store.read("shared_key").unwrap().unwrap();
        assert!(
            val.starts_with('t') && val.contains("_v"),
            "unexpected value format: {val}"
        );

        // File must be exactly one record (no duplicates)
        let file_len = std::fs::metadata(tmp.path()).unwrap().len() as usize;
        assert_eq!(file_len, RECORD_SIZE);
    }

    #[test]
    fn test_concurrent_readers_and_writers() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        // Seed some initial data
        let store = restricted_store(tmp.path());
        for i in 0..10 {
            store.upsert(&format!("k{i}"), &format!("v{i}")).unwrap();
        }

        let writer_threads: Vec<_> = (0..4)
            .map(|t| {
                let p = path.clone();
                std::thread::spawn(move || {
                    let store =
                        KvpPoolStore::new(Some(p), PoolMode::Restricted, false)
                            .unwrap();
                    for round in 0..5 {
                        let key = format!("k{t}");
                        store.upsert(&key, &format!("w{t}_r{round}")).unwrap();
                    }
                })
            })
            .collect();

        let reader_threads: Vec<_> = (0..4)
            .map(|_| {
                let p = path.clone();
                std::thread::spawn(move || {
                    let store =
                        KvpPoolStore::new(Some(p), PoolMode::Restricted, false)
                            .unwrap();
                    for _ in 0..20 {
                        let entries = store.entries().unwrap();
                        // Should always have 10 keys, never more
                        assert!(entries.len() <= 10);
                        // File should be well-formed
                        for (k, v) in &entries {
                            assert!(!k.is_empty());
                            assert!(!v.is_empty());
                        }
                    }
                })
            })
            .collect();

        for t in writer_threads {
            t.join().unwrap();
        }
        for t in reader_threads {
            t.join().unwrap();
        }

        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 10);
    }

    #[test]
    fn test_concurrent_writers_at_key_cap() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        // Fill to just under the cap
        let store = restricted_store(tmp.path());
        for i in 0..MAX_UNIQUE_KEYS - 4 {
            store.upsert(&format!("pre{i}"), "v").unwrap();
        }

        // 4 threads each try to add one new unique key concurrently
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let p = path.clone();
                std::thread::spawn(move || {
                    let store =
                        KvpPoolStore::new(Some(p), PoolMode::Restricted, false)
                            .unwrap();
                    store.upsert(&format!("new{t}"), "v")
                })
            })
            .collect();

        let results: Vec<_> =
            threads.into_iter().map(|t| t.join().unwrap()).collect();

        let successes = results.iter().filter(|r| r.is_ok()).count();
        let cap_errors = results
            .iter()
            .filter(|r| {
                matches!(r, Err(KvpError::MaxUniqueKeysExceeded { .. }))
            })
            .count();

        // Exactly 4 should succeed (filling the last 4 slots)
        assert_eq!(successes, 4);
        assert_eq!(cap_errors, 0);
        assert_eq!(store.len().unwrap(), MAX_UNIQUE_KEYS);

        // One more should be rejected
        let err = store.upsert("one_too_many", "v").unwrap_err();
        assert!(matches!(err, KvpError::MaxUniqueKeysExceeded { .. }));
    }
}
