// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Production `KvpStore` implementation backed by the Hyper-V binary
//! pool-file format with flock-based concurrency control.
//!
//! Each record is a fixed-size block of 2,560 bytes: 512 bytes for the
//! key and 2,048 bytes for the value. Unused space is zero-filled.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use sysinfo::System;

use crate::KvpStore;

pub const HV_KVP_EXCHANGE_MAX_KEY_SIZE: usize = 512;
pub const HV_KVP_EXCHANGE_MAX_VALUE_SIZE: usize = 2048;
pub const RECORD_SIZE: usize =
    HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE;

/// Hyper-V KVP pool-file store.
///
/// Reads and writes the binary Hyper-V KVP format: fixed-size records
/// of [`RECORD_SIZE`] bytes (512-byte key + 2,048-byte value) with
/// flock-based concurrency control.
#[derive(Clone)]
pub struct HyperVKvpStore {
    path: PathBuf,
}

impl HyperVKvpStore {
    /// Open (or create) the pool file at the given path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
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
            - get_uptime().as_secs();

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
            if metadata.mtime() < boot_time as i64 {
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
}

/// Encode a key-value pair into a single fixed-size record.
///
/// The key is truncated (if necessary) and zero-padded to 512 bytes.
/// The value is truncated (if necessary) and zero-padded to 2,048
/// bytes.
pub fn encode_record(key: &str, value: &str) -> Vec<u8> {
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
/// Trailing null bytes are stripped from both fields.
pub fn decode_record(data: &[u8]) -> io::Result<(String, String)> {
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
    /// Append one fixed-size record to the pool file.
    ///
    /// Acquires an exclusive flock, writes the record, flushes, and
    /// releases the lock.
    fn write(&self, key: &str, value: &str) -> io::Result<()> {
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
    /// matching `key` (append-only semantics).
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

    /// Return every record in the pool file, including duplicates.
    fn entries(&self) -> io::Result<Vec<(String, String)>> {
        let mut file = match self.open_for_read() {
            Ok(f) => f,
            Err(ref e) if e.kind() == ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(e) => return Err(e),
        };

        FileExt::lock_shared(&file).map_err(|e| {
            io::Error::other(format!("failed to lock KVP file: {e}"))
        })?;

        let records = read_all_records(&mut file);
        let _ = FileExt::unlock(&file);
        records
    }

    /// Rewrite the pool file without the record(s) matching `key`.
    ///
    /// Returns `true` if at least one record was removed.
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
            // Seek to start after truncation -- set_len doesn't move
            // the cursor on all platforms, but with the append flag off
            // the next write goes to the current position. Reopen via
            // write to position at 0 by truncating.
            use std::io::Seek;
            file.seek(std::io::SeekFrom::Start(0))?;

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
    fn test_encode_truncates_long_key() {
        let key = "K".repeat(HV_KVP_EXCHANGE_MAX_KEY_SIZE + 100);
        let record = encode_record(&key, "v");
        let (decoded_key, _) = decode_record(&record).expect("decode failed");
        assert_eq!(decoded_key.len(), HV_KVP_EXCHANGE_MAX_KEY_SIZE);
    }

    #[test]
    fn test_encode_truncates_long_value() {
        let value = "V".repeat(HV_KVP_EXCHANGE_MAX_VALUE_SIZE + 100);
        let record = encode_record("k", &value);
        let (_, decoded_value) = decode_record(&record).expect("decode failed");
        assert_eq!(decoded_value.len(), HV_KVP_EXCHANGE_MAX_VALUE_SIZE);
    }

    #[test]
    fn test_write_and_read() {
        let tmp = NamedTempFile::new().unwrap();
        let store = HyperVKvpStore::new(tmp.path());

        store.write("key1", "value1").unwrap();
        store.write("key2", "value2").unwrap();

        assert_eq!(store.read("key1").unwrap(), Some("value1".to_string()));
        assert_eq!(store.read("key2").unwrap(), Some("value2".to_string()));
        assert_eq!(store.read("nonexistent").unwrap(), None);
    }

    #[test]
    fn test_read_returns_last_match() {
        let tmp = NamedTempFile::new().unwrap();
        let store = HyperVKvpStore::new(tmp.path());

        store.write("key", "first").unwrap();
        store.write("key", "second").unwrap();
        store.write("key", "third").unwrap();

        assert_eq!(store.read("key").unwrap(), Some("third".to_string()));
    }

    #[test]
    fn test_entries_includes_duplicates() {
        let tmp = NamedTempFile::new().unwrap();
        let store = HyperVKvpStore::new(tmp.path());

        store.write("key", "v1").unwrap();
        store.write("key", "v2").unwrap();
        store.write("other", "v3").unwrap();

        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], ("key".to_string(), "v1".to_string()));
        assert_eq!(entries[1], ("key".to_string(), "v2".to_string()));
        assert_eq!(entries[2], ("other".to_string(), "v3".to_string()));
    }

    #[test]
    fn test_delete_removes_all_matches() {
        let tmp = NamedTempFile::new().unwrap();
        let store = HyperVKvpStore::new(tmp.path());

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
    fn test_delete_nonexistent_returns_false() {
        let tmp = NamedTempFile::new().unwrap();
        let store = HyperVKvpStore::new(tmp.path());

        store.write("key", "value").unwrap();
        assert!(!store.delete("nonexistent").unwrap());
    }

    #[test]
    fn test_read_missing_file_returns_none() {
        let store = HyperVKvpStore::new("/tmp/nonexistent_kvp_pool_test");
        assert_eq!(store.read("key").unwrap(), None);
    }

    #[test]
    fn test_entries_missing_file_returns_empty() {
        let store = HyperVKvpStore::new("/tmp/nonexistent_kvp_pool_test");
        assert!(store.entries().unwrap().is_empty());
    }

    #[test]
    fn test_record_size_consistency() {
        let record = encode_record("k", "v");
        assert_eq!(record.len(), RECORD_SIZE);
        assert_eq!(RECORD_SIZE, 2560);
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
                    let store = HyperVKvpStore::new(&p);
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

        let store = HyperVKvpStore::new(&path);
        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), num_threads * iterations);
    }

    #[test]
    fn test_provisioning_report_via_store() {
        let tmp = NamedTempFile::new().unwrap();
        let store = HyperVKvpStore::new(tmp.path());

        store
            .write("PROVISIONING_REPORT", "result=success|agent=test")
            .unwrap();

        let value = store.read("PROVISIONING_REPORT").unwrap();
        assert_eq!(value, Some("result=success|agent=test".to_string()));
    }
}
