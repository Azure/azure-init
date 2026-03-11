// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hyper-V KVP (Key-Value Pair) pool file backend.
//!
//! Hyper-V exposes a Data Exchange Service that lets a guest and its
//! host share key-value pairs through a set of pool files.  Each pool
//! file is a flat sequence of fixed-size records (512-byte key +
//! 2,048-byte value, zero-padded).  There is no record-count header;
//! the file grows by one record per write.
//!
//! On Azure, the host-side KVP consumer truncates values beyond
//! 1,022 bytes, so Azure guests should use [`AzureKvpStore`](crate::AzureKvpStore).
//!
//! ## Reference
//! - [Hyper-V Data Exchange Service (KVP)](https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/reference/integration-services#hyper-v-data-exchange-service-kvp)

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use sysinfo::System;

use crate::{KvpError, KvpStore};

/// Key field width in the on-disk record format (bytes).
const HV_KVP_EXCHANGE_MAX_KEY_SIZE: usize = 512;

/// Value field width in the on-disk record format (bytes).
const HV_KVP_EXCHANGE_MAX_VALUE_SIZE: usize = 2048;

/// Total size of one on-disk record (key + value).
const RECORD_SIZE: usize =
    HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE;

/// Hyper-V KVP pool file store.
///
/// Reads and writes the binary Hyper-V KVP format: fixed-size records
/// of [`RECORD_SIZE`] bytes (512-byte key + 2,048-byte value) with
/// flock-based concurrency control.
#[derive(Clone, Debug)]
pub struct HyperVKvpStore {
    path: PathBuf,
}

impl HyperVKvpStore {
    /// Create a new store backed by the pool file at `path`.
    ///
    /// When `truncate_on_stale` is `true` the constructor checks
    /// whether the pool file predates the current boot and, if so,
    /// truncates it before returning.
    pub fn new(
        path: impl Into<PathBuf>,
        truncate_on_stale: bool,
    ) -> Result<Self, KvpError> {
        let store = Self { path: path.into() };
        if truncate_on_stale && store.pool_is_stale()? {
            store.truncate_pool()?;
        }
        Ok(store)
    }

    /// Return a reference to the pool file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    // -- Private helpers ------------------------------------------------

    fn boot_time() -> Result<i64, KvpError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| io::Error::other(format!("clock error: {e}")))?
            .as_secs();
        Ok(now.saturating_sub(get_uptime().as_secs()) as i64)
    }

    /// Check whether the pool file's mtime predates the current boot.
    ///
    /// Returns `false` if the file does not exist.
    fn pool_is_stale(&self) -> Result<bool, KvpError> {
        let metadata = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        let boot = Self::boot_time()?;
        Ok(metadata.mtime() < boot)
    }

    #[cfg(test)]
    fn pool_is_stale_at_boot(&self, boot_time: i64) -> Result<bool, KvpError> {
        let metadata = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        Ok(metadata.mtime() < boot_time)
    }

    /// Truncate the pool file to zero length under an exclusive flock.
    fn truncate_pool(&self) -> Result<(), KvpError> {
        let file =
            match OpenOptions::new().read(true).write(true).open(&self.path) {
                Ok(f) => f,
                Err(ref e) if e.kind() == ErrorKind::NotFound => {
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            };

        FileExt::lock_exclusive(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;

        let result = file.set_len(0).map_err(KvpError::from);

        let _ = FileExt::unlock(&file);
        result
    }

    fn open_for_append(&self) -> io::Result<File> {
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
    }

    fn open_for_read(&self) -> io::Result<File> {
        OpenOptions::new().read(true).open(&self.path)
    }

    fn open_for_read_write(&self) -> io::Result<File> {
        OpenOptions::new().read(true).write(true).open(&self.path)
    }
}

/// Encode a key-value pair into a single fixed-size record.
///
/// The key is zero-padded to [`HV_KVP_EXCHANGE_MAX_KEY_SIZE`] bytes
/// and the value is zero-padded to [`HV_KVP_EXCHANGE_MAX_VALUE_SIZE`]
/// bytes. The caller is responsible for ensuring the key and value do
/// not exceed the on-disk field widths; if they do, only the first N
/// bytes are written (no error is raised at this level -- validation
/// happens in [`HyperVKvpStore::write`]).
pub(crate) fn encode_record(key: &str, value: &str) -> Vec<u8> {
    let mut buf = vec![0u8; RECORD_SIZE];

    let key_bytes = key.as_bytes();
    let key_len = key_bytes.len().min(HV_KVP_EXCHANGE_MAX_KEY_SIZE);
    buf[..key_len].copy_from_slice(&key_bytes[..key_len]);

    let val_bytes = value.as_bytes();
    let val_len = val_bytes.len().min(HV_KVP_EXCHANGE_MAX_VALUE_SIZE);
    buf[HV_KVP_EXCHANGE_MAX_KEY_SIZE..HV_KVP_EXCHANGE_MAX_KEY_SIZE + val_len]
        .copy_from_slice(&val_bytes[..val_len]);

    buf
}

/// Decode a fixed-size record into its key and value strings.
///
/// Trailing null bytes are stripped from both fields. Returns an error
/// if `data` is not exactly [`RECORD_SIZE`] bytes or if either field
/// contains invalid UTF-8.
pub(crate) fn decode_record(data: &[u8]) -> io::Result<(String, String)> {
    if data.len() != RECORD_SIZE {
        return Err(io::Error::other(format!(
            "record size mismatch: expected {RECORD_SIZE}, got {}",
            data.len()
        )));
    }

    let (key_bytes, value_bytes) = data.split_at(HV_KVP_EXCHANGE_MAX_KEY_SIZE);

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

/// Read all records from a file that is already open and locked.
fn read_all_records(file: &mut File) -> io::Result<Vec<(String, String)>> {
    let metadata = file.metadata()?;
    let len = metadata.len() as usize;

    if len == 0 {
        return Ok(Vec::new());
    }

    if !len.is_multiple_of(RECORD_SIZE) {
        return Err(io::Error::other(format!(
            "file size ({len}) is not a multiple of record size ({RECORD_SIZE})"
        )));
    }

    // Ensure we start reading from the beginning of the file.
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

impl KvpStore for HyperVKvpStore {
    const MAX_KEY_SIZE: usize = HV_KVP_EXCHANGE_MAX_KEY_SIZE;
    const MAX_VALUE_SIZE: usize = HV_KVP_EXCHANGE_MAX_VALUE_SIZE;

    /// Append one fixed-size record to the pool file.
    ///
    /// Acquires an exclusive flock, writes the record, flushes, and
    /// releases the lock.
    fn backend_write(&self, key: &str, value: &str) -> Result<(), KvpError> {
        let mut file = self.open_for_append()?;
        let record = encode_record(key, value);

        FileExt::lock_exclusive(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;

        let write_result = file.write_all(&record).and_then(|_| file.flush());

        let unlock_result = FileExt::unlock(&file).map_err(|e| {
            io::Error::other(format!("failed to unlock KVP file: {e}"))
        });

        if let Err(err) = write_result {
            let _ = unlock_result;
            return Err(err.into());
        }
        unlock_result.map_err(KvpError::from)
    }

    /// Scan all records and return the value of the last record
    /// matching `key` (last-write-wins).
    ///
    /// Acquires a shared flock during the scan. Returns `Ok(None)` if
    /// the pool file does not exist or no record matches.
    fn backend_read(&self, key: &str) -> Result<Option<String>, KvpError> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(None);
            }
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

    /// Return all key-value pairs as a deduplicated `HashMap`.
    ///
    /// Duplicate keys are resolved by last-write-wins. Acquires a
    /// shared flock during the scan. Returns an empty map if the pool
    /// file does not exist.
    fn entries(&self) -> Result<HashMap<String, String>, KvpError> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(HashMap::new());
            }
            Err(e) => return Err(e.into()),
        };

        FileExt::lock_shared(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;

        let records = read_all_records(&mut file);
        let _ = FileExt::unlock(&file);
        let records = records?;

        let mut map = HashMap::new();
        for (k, v) in records {
            map.insert(k, v);
        }
        Ok(map)
    }

    /// Return all raw records without deduplication.
    ///
    /// Acquires a shared flock during the scan. Returns an empty list
    /// if the pool file does not exist.
    fn entries_raw(&self) -> Result<Vec<(String, String)>, KvpError> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(Vec::new());
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

    /// Rewrite the pool file without the record(s) matching `key`.
    ///
    /// Acquires an exclusive flock for the duration. Returns `true` if
    /// at least one record was removed, `false` if the key was not
    /// found. Returns `Ok(false)` if the pool file does not exist.
    fn delete(&self, key: &str) -> Result<bool, KvpError> {
        let mut file = match self.open_for_read_write() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(false);
            }
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

    /// Truncate the pool file to zero length, removing all records.
    fn backend_clear(&self) -> Result<(), KvpError> {
        self.truncate_pool()
    }

    /// Whether the pool file's mtime predates the current boot.
    fn is_stale(&self) -> Result<bool, KvpError> {
        self.pool_is_stale()
    }
}

fn get_uptime() -> Duration {
    let mut system = System::new();
    system.refresh_memory();
    system.refresh_cpu_usage();
    Duration::from_secs(System::uptime())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn hyperv_store(path: &Path) -> HyperVKvpStore {
        HyperVKvpStore::new(path, false).unwrap()
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let key = "test_key";
        let value = "test_value";
        let record = encode_record(key, value);

        assert_eq!(record.len(), RECORD_SIZE);

        let (decoded_key, decoded_value) =
            decode_record(&record).expect("decode failed");
        assert_eq!(decoded_key, key);
        assert_eq!(decoded_value, value);
    }

    #[test]
    fn test_write_rejects_empty_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        let err = store.write("", "value").unwrap_err();
        assert!(
            matches!(err, KvpError::EmptyKey),
            "expected EmptyKey, got: {err}"
        );
    }

    #[test]
    fn test_write_rejects_null_in_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        let err = store.write("bad\0key", "value").unwrap_err();
        assert!(
            matches!(err, KvpError::KeyContainsNull),
            "expected KeyContainsNull, got: {err}"
        );
    }

    #[test]
    fn test_write_rejects_oversized_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        let key = "K".repeat(HV_KVP_EXCHANGE_MAX_KEY_SIZE + 1);
        let err = store.write(&key, "v").unwrap_err();
        assert!(
            matches!(err, KvpError::KeyTooLarge { .. }),
            "expected KeyTooLarge, got: {err}"
        );
    }

    #[test]
    fn test_write_rejects_oversized_value_hyperv() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        let value = "V".repeat(HV_KVP_EXCHANGE_MAX_VALUE_SIZE + 1);
        let err = store.write("k", &value).unwrap_err();
        assert!(
            matches!(err, KvpError::ValueTooLarge { .. }),
            "expected ValueTooLarge, got: {err}"
        );
    }

    #[test]
    fn test_write_and_read() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key1", "value1").unwrap();
        store.write("key2", "value2").unwrap();

        assert_eq!(store.read("key1").unwrap(), Some("value1".to_string()));
        assert_eq!(store.read("key2").unwrap(), Some("value2".to_string()));
        assert_eq!(store.read("nonexistent").unwrap(), None);
    }

    #[test]
    fn test_read_returns_last_match() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key", "first").unwrap();
        store.write("key", "second").unwrap();
        store.write("key", "third").unwrap();

        assert_eq!(store.read("key").unwrap(), Some("third".to_string()));
    }

    #[test]
    fn test_entries_deduplicates_with_last_write_wins() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key", "v1").unwrap();
        store.write("key", "v2").unwrap();
        store.write("other", "v3").unwrap();

        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.get("key"), Some(&"v2".to_string()));
        assert_eq!(entries.get("other"), Some(&"v3".to_string()));
    }

    #[test]
    fn test_entries_raw_preserves_duplicates() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

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
    fn test_clear_empties_store() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key", "value").unwrap();
        assert!(tmp.path().metadata().unwrap().len() > 0);

        store.clear().unwrap();
        assert_eq!(tmp.path().metadata().unwrap().len(), 0);
        assert_eq!(store.read("key").unwrap(), None);
    }

    #[test]
    fn test_is_stale_false_for_fresh_file() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key", "value").unwrap();
        assert!(!store.is_stale().unwrap());
    }

    #[test]
    fn test_is_stale_false_when_file_missing() {
        let store = hyperv_store(Path::new("/tmp/nonexistent-kvp-pool"));
        assert!(!store.is_stale().unwrap());
    }

    #[test]
    fn test_pool_is_stale_at_boot_detects_old_file() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key", "value").unwrap();
        assert!(store.pool_is_stale_at_boot(i64::MAX).unwrap());
    }

    #[test]
    fn test_pool_is_stale_at_boot_keeps_new_file() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key", "value").unwrap();
        assert!(!store.pool_is_stale_at_boot(0).unwrap());
    }

    #[test]
    fn test_clear_ok_when_file_missing() {
        let store = hyperv_store(Path::new("/tmp/nonexistent-kvp-pool"));
        store.clear().unwrap();
    }

    #[test]
    fn test_delete_removes_all_matches() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key", "v1").unwrap();
        store.write("key", "v2").unwrap();
        store.write("other", "v3").unwrap();

        assert!(store.delete("key").unwrap());
        assert_eq!(store.read("key").unwrap(), None);
        assert_eq!(store.read("other").unwrap(), Some("v3".to_string()));

        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_multi_thread_concurrent_writes() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let num_threads: usize = 20;
        let iterations: usize = 1_000;

        let handles: Vec<_> = (0..num_threads)
            .map(|tid| {
                let p = path.clone();
                std::thread::spawn(move || {
                    let store = HyperVKvpStore::new(&p, false).unwrap();
                    for i in 0..iterations {
                        let key = format!("thread-{tid}-iter-{i}");
                        let value = format!("value-{tid}-{i}");
                        store.write(&key, &value).expect("write failed");
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread panicked");
        }

        let store = HyperVKvpStore::new(&path, false).unwrap();
        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), num_threads * iterations);
    }
}
