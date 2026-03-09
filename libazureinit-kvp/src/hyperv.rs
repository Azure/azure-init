// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hyper-V-backed [`KvpStore`] implementation.
//!
//! ## Record format
//! - Fixed-size records of [`RECORD_SIZE`] bytes (512-byte key +
//!   2,048-byte value), zero-padded on disk.
//! - No record-count header or explicit record cap in this layer.
//!
//! ## Behavior summary
//! - **`write`**: append-only; one record appended per call.
//! - **`read`**: last-write-wins for duplicate keys.
//! - **`entries`**: returns a deduplicated `HashMap` with last-write-wins.
//! - **`delete`**: rewrites file and removes all records for the key.
//! - **`truncate_if_stale`**: truncates if file predates boot; on lock
//!   contention (`WouldBlock`) it returns `Ok(())` and skips.
//!
//! ## Limits
//! Writes validate key/value byte lengths using [`KvpLimits`] and return
//! errors for empty/oversized keys or oversized values. The on-disk
//! format is always 512 + 2,048 bytes; limits only constrain what may
//! be written (for example, Azure's 1,022-byte value limit).
//!
//! Higher layers are responsible for splitting/chunking oversized
//! diagnostics payloads before calling this store.
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

use crate::{
    KvpLimits, KvpStore, HYPERV_MAX_KEY_BYTES, HYPERV_MAX_VALUE_BYTES,
};

/// DMI chassis asset tag used to identify Azure VMs.
const AZURE_CHASSIS_ASSET_TAG: &str = "7783-7084-3265-9085-8269-3286-77";
const AZURE_CHASSIS_ASSET_TAG_PATH: &str =
    "/sys/class/dmi/id/chassis_asset_tag";

fn is_azure_vm(tag_path: Option<&str>) -> bool {
    let path = tag_path.unwrap_or(AZURE_CHASSIS_ASSET_TAG_PATH);
    std::fs::read_to_string(path)
        .map(|s| s.trim() == AZURE_CHASSIS_ASSET_TAG)
        .unwrap_or(false)
}

/// Key field width in the on-disk record format (bytes).
const HV_KVP_EXCHANGE_MAX_KEY_SIZE: usize = HYPERV_MAX_KEY_BYTES;

/// Value field width in the on-disk record format (bytes).
const HV_KVP_EXCHANGE_MAX_VALUE_SIZE: usize = HYPERV_MAX_VALUE_BYTES;

/// Total size of one on-disk record (key + value).
const RECORD_SIZE: usize =
    HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE;

/// Hyper-V KVP pool file store.
///
/// Reads and writes the binary Hyper-V KVP format: fixed-size records
/// of [`RECORD_SIZE`] bytes (512-byte key + 2,048-byte value) with
/// flock-based concurrency control.
///
/// Constructed via [`HyperVKvpStore::new`] with a file path and a
/// [`KvpLimits`] that determines the maximum allowed key and value
/// byte lengths for writes.
#[derive(Clone, Debug)]
pub struct HyperVKvpStore {
    path: PathBuf,
    limits: KvpLimits,
}

impl HyperVKvpStore {
    /// Create a store with explicit limits.
    ///
    /// The file is created on first write if it does not already exist.
    /// Use [`HyperVKvpStore::new_autodetect`] to choose limits automatically.
    pub fn new(path: impl Into<PathBuf>, limits: KvpLimits) -> Self {
        Self {
            path: path.into(),
            limits,
        }
    }

    /// Create a store with limits chosen from host platform detection.
    ///
    /// If the Azure DMI asset tag is present, uses [`KvpLimits::azure`].
    /// Otherwise, uses [`KvpLimits::hyperv`].
    pub fn new_autodetect(path: impl Into<PathBuf>) -> Self {
        let limits = if is_azure_vm(None) {
            KvpLimits::azure()
        } else {
            KvpLimits::hyperv()
        };
        Self {
            path: path.into(),
            limits,
        }
    }

    #[cfg(test)]
    fn new_autodetect_with_tag_path(
        path: impl Into<PathBuf>,
        tag_path: &str,
    ) -> Self {
        let limits = if is_azure_vm(Some(tag_path)) {
            KvpLimits::azure()
        } else {
            KvpLimits::hyperv()
        };
        Self {
            path: path.into(),
            limits,
        }
    }

    /// Return a reference to the pool file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Truncate the file when its mtime predates the current boot
    /// (stale-data guard).
    ///
    /// An exclusive `flock` is held while checking metadata and
    /// truncating so that concurrent processes don't race on the same
    /// check-then-truncate sequence. If the lock cannot be acquired
    /// immediately (another client holds it), the call returns `Ok(())`
    /// without blocking.
    pub fn truncate_if_stale(&self) -> io::Result<()> {
        let boot_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| io::Error::other(format!("clock error: {e}")))?
            .as_secs()
            .saturating_sub(get_uptime().as_secs())
            as i64;

        let file =
            match OpenOptions::new().read(true).write(true).open(&self.path) {
                Ok(f) => f,
                Err(ref e) if e.kind() == ErrorKind::NotFound => {
                    return Ok(());
                }
                Err(e) => return Err(e),
            };

        if FileExt::try_lock_exclusive(&file).is_err() {
            return Ok(());
        }

        let result = (|| -> io::Result<()> {
            let metadata = file.metadata()?;
            if metadata.mtime() < boot_time {
                file.set_len(0)?;
            }
            Ok(())
        })();

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

    fn validate_key_value(&self, key: &str, value: &str) -> io::Result<()> {
        if key.is_empty() {
            return Err(io::Error::other("KVP key must not be empty"));
        }
        if key.len() > self.limits.max_key_size {
            return Err(io::Error::other(format!(
                "KVP key length ({}) exceeds maximum ({})",
                key.len(),
                self.limits.max_key_size
            )));
        }
        if value.len() > self.limits.max_value_size {
            return Err(io::Error::other(format!(
                "KVP value length ({}) exceeds maximum ({})",
                value.len(),
                self.limits.max_value_size
            )));
        }
        Ok(())
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
/// if `data` is not exactly [`RECORD_SIZE`] bytes.
pub(crate) fn decode_record(data: &[u8]) -> io::Result<(String, String)> {
    if data.len() != RECORD_SIZE {
        return Err(io::Error::other(format!(
            "record size mismatch: expected {RECORD_SIZE}, got {}",
            data.len()
        )));
    }

    let key = String::from_utf8(data[..HV_KVP_EXCHANGE_MAX_KEY_SIZE].to_vec())
        .unwrap_or_default()
        .trim_end_matches('\0')
        .to_string();

    let value =
        String::from_utf8(data[HV_KVP_EXCHANGE_MAX_KEY_SIZE..].to_vec())
            .unwrap_or_default()
            .trim_end_matches('\0')
            .to_string();

    Ok((key, value))
}

/// Read all records from a file that is already open and locked.
fn read_all_records(file: &mut File) -> io::Result<Vec<(String, String)>> {
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;

    if contents.is_empty() {
        return Ok(Vec::new());
    }

    if contents.len() % RECORD_SIZE != 0 {
        return Err(io::Error::other(format!(
            "file size ({}) is not a multiple of record size ({RECORD_SIZE})",
            contents.len()
        )));
    }

    contents
        .chunks_exact(RECORD_SIZE)
        .map(decode_record)
        .collect()
}

impl KvpStore for HyperVKvpStore {
    fn limits(&self) -> KvpLimits {
        self.limits
    }

    /// Append one fixed-size record to the pool file.
    ///
    /// Validates key and value against the configured [`KvpLimits`],
    /// acquires an exclusive flock, writes the record, flushes, and
    /// releases the lock.
    fn write(&self, key: &str, value: &str) -> io::Result<()> {
        self.validate_key_value(key, value)?;

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
            return Err(err);
        }
        unlock_result
    }

    /// Scan all records and return the value of the last record
    /// matching `key` (last-write-wins).
    ///
    /// Acquires a shared flock during the scan. Returns `Ok(None)` if
    /// the pool file does not exist or no record matches.
    fn read(&self, key: &str) -> io::Result<Option<String>> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(e) => return Err(e),
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
    /// Duplicate keys are resolved by last-write-wins, matching
    /// [`read`](KvpStore::read) semantics. Acquires a shared flock
    /// during the scan. Returns an empty map if the pool file does
    /// not exist.
    fn entries(&self) -> io::Result<HashMap<String, String>> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(HashMap::new());
            }
            Err(e) => return Err(e),
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

    /// Rewrite the pool file without the record(s) matching `key`.
    ///
    /// Acquires an exclusive flock for the duration. Returns `true` if
    /// at least one record was removed, `false` if the key was not
    /// found. Returns `Ok(false)` if the pool file does not exist.
    fn delete(&self, key: &str) -> io::Result<bool> {
        let mut file = match self.open_for_read_write() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(e) => return Err(e),
        };

        FileExt::lock_exclusive(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;

        let result = (|| -> io::Result<bool> {
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

    fn truncate_with_boot_time(path: &Path, boot_time: i64) -> io::Result<()> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let metadata = file.metadata()?;
        if metadata.mtime() < boot_time {
            file.set_len(0)?;
        }
        Ok(())
    }

    fn hyperv_store(path: &Path) -> HyperVKvpStore {
        HyperVKvpStore::new(path, KvpLimits::hyperv())
    }

    fn azure_store(path: &Path) -> HyperVKvpStore {
        HyperVKvpStore::new(path, KvpLimits::azure())
    }

    #[test]
    fn test_autodetect_uses_azure_limits_on_azure_host() {
        let tag_file = NamedTempFile::new().unwrap();
        let pool_file = NamedTempFile::new().unwrap();
        // DMI files on Linux have a trailing newline.
        std::fs::write(tag_file.path(), format!("{AZURE_CHASSIS_ASSET_TAG}\n"))
            .unwrap();

        let store = HyperVKvpStore::new_autodetect_with_tag_path(
            pool_file.path(),
            tag_file.path().to_str().unwrap(),
        );
        assert_eq!(store.limits(), KvpLimits::azure());
    }

    #[test]
    fn test_autodetect_uses_hyperv_limits_on_bare_hyperv() {
        let tag_file = NamedTempFile::new().unwrap();
        let pool_file = NamedTempFile::new().unwrap();
        std::fs::write(tag_file.path(), "bare-hyperv-tag").unwrap();

        let store = HyperVKvpStore::new_autodetect_with_tag_path(
            pool_file.path(),
            tag_file.path().to_str().unwrap(),
        );
        assert_eq!(store.limits(), KvpLimits::hyperv());
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
            err.to_string().contains("empty"),
            "expected empty-key error, got: {err}"
        );
    }

    #[test]
    fn test_write_rejects_oversized_key() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        let key = "K".repeat(HV_KVP_EXCHANGE_MAX_KEY_SIZE + 1);
        let err = store.write(&key, "v").unwrap_err();
        assert!(
            err.to_string().contains("key length"),
            "expected key-length error, got: {err}"
        );
    }

    #[test]
    fn test_write_rejects_oversized_value_hyperv() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        let value = "V".repeat(HV_KVP_EXCHANGE_MAX_VALUE_SIZE + 1);
        let err = store.write("k", &value).unwrap_err();
        assert!(
            err.to_string().contains("value length"),
            "expected value-length error, got: {err}"
        );
    }

    #[test]
    fn test_azure_limits_reject_long_value() {
        let tmp = NamedTempFile::new().unwrap();
        let store = azure_store(tmp.path());

        let value = "V".repeat(1023);
        let err = store.write("k", &value).unwrap_err();
        assert!(
            err.to_string().contains("value length"),
            "expected value-length error with azure limits, got: {err}"
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
    fn test_truncate_if_stale_truncates_when_file_is_older_than_boot() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key", "value").unwrap();
        assert!(tmp.path().metadata().unwrap().len() > 0);

        truncate_with_boot_time(tmp.path(), i64::MAX).unwrap();
        assert_eq!(tmp.path().metadata().unwrap().len(), 0);
    }

    #[test]
    fn test_truncate_if_stale_keeps_file_when_newer_than_boot() {
        let tmp = NamedTempFile::new().unwrap();
        let store = hyperv_store(tmp.path());

        store.write("key", "value").unwrap();
        let len_before = tmp.path().metadata().unwrap().len();

        // Epoch boot time ensures any current file mtime is considered fresh.
        truncate_with_boot_time(tmp.path(), 0).unwrap();
        assert_eq!(tmp.path().metadata().unwrap().len(), len_before);
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
                    let store = HyperVKvpStore::new(&p, KvpLimits::hyperv());
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

        let store = HyperVKvpStore::new(&path, KvpLimits::hyperv());
        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), num_threads * iterations);
    }
}
