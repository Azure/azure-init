// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This module provides core functionality for handling telemetry tracing
//! related to azure-init's interaction with Hyper-V KVP (Key-Value Pair) storage.
//!
//! # Constants
//! - `HV_KVP_EXCHANGE_MAX_KEY_SIZE`: Defines the maximum key size for KVP exchange.
//! - `HV_KVP_EXCHANGE_MAX_VALUE_SIZE`: Defines the maximum value size for KVP exchange.
//! - `HV_KVP_AZURE_MAX_VALUE_SIZE`: Maximum value size before splitting into multiple slices.
//!

use std::fmt as std_fmt;
use std::fmt::Write as std_write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::field::Visit;
use tracing_subscriber::Layer;

use crate::tracing::{handle_event, handle_span, MyInstant};

use tracing::span::{Attributes, Id};
use tracing::Subscriber;
use tracing_subscriber::layer::Context as TracingContext;
use tracing_subscriber::registry::LookupSpan;

use nix::fcntl::{flock, FlockArg};
use std::fs::OpenOptions;
use std::io::{self, Error, ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const HV_KVP_EXCHANGE_MAX_KEY_SIZE: usize = 512;
const HV_KVP_EXCHANGE_MAX_VALUE_SIZE: usize = 2048;
const HV_KVP_AZURE_MAX_VALUE_SIZE: usize = 1024;
const EVENT_PREFIX: &str = concat!("azure-init-", env!("CARGO_PKG_VERSION"));

/// A custom visitor that captures the value of the `msg` field from a tracing event.
/// It implements the `tracing::field::Visit` trait and records the value into
/// a provided mutable string reference.
///
/// This visitor is primarily used in the `on_event` method of the `EmitKVPLayer`
/// to extract event messages and log them as key-value pairs.
pub struct StringVisitor<'a> {
    string: &'a mut String,
}

impl<'a> Visit for StringVisitor<'a> {
    /// Records the debug representation of the event's value and stores it in the provided string.
    ///
    /// # Arguments
    /// * `_field` - A reference to the event's field metadata.
    /// * `value` - The debug value associated with the field.
    fn record_debug(
        &mut self,
        _field: &tracing::field::Field,
        value: &dyn std_fmt::Debug,
    ) {
        write!(self.string, "{:?}", value).unwrap();
    }
}
/// A custom tracing layer that emits span data as key-value pairs (KVP)
/// to a file for consumption by the Hyper-V daemon. This struct captures
/// spans and events from the tracing system and writes them to a
/// specified file in KVP format.
pub struct EmitKVPLayer {
    file_path: std::path::PathBuf,
}

impl EmitKVPLayer {
    pub fn new(file_path: std::path::PathBuf) -> Self {
        Self { file_path }
    }
}

impl<S> Layer<S> for EmitKVPLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    /// Handles event occurrences within a span by extracting the `msg` field,
    /// recording the event, and writing the event as key-value pairs (KVP) to a log file.
    ///
    /// This method uses the `StringVisitor` to capture the message of the event and
    /// links the event to the span by writing both span and event data into the same file.
    ///
    /// If an `ERROR` level event is encountered, a failure flag is inserted into the span's extensions.
    ///
    /// # Example
    /// ```rust
    /// use tracing::{event, Level};
    /// event!(Level::INFO, msg = "Event message");
    /// ```
    fn on_event(&self, event: &tracing::Event<'_>, ctx: TracingContext<'_, S>) {
        let mut event_message = String::new();

        let mut visitor = StringVisitor {
            string: &mut event_message,
        };

        event.record(&mut visitor);

        if let Some(span) = ctx.lookup_current() {
            let mut extensions = span.extensions_mut();

            if event.metadata().level() == &tracing::Level::ERROR {
                extensions.insert("failure".to_string());
            }

            let event_instant = MyInstant::now();

            handle_event(&event_message, &span, &self.file_path, event_instant);
        }
    }

    /// Called when a new span is created. Records the start time of the span
    /// and stores it as an extension within the span's context, to be used
    /// for generating telemetry data for Hyper-V.
    fn on_new_span(
        &self,
        _attrs: &Attributes<'_>,
        id: &Id,
        ctx: TracingContext<'_, S>,
    ) {
        let start_instant = MyInstant::now();
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(start_instant);
        }
    }

    /// Called when a span is closed and checks for any logged errors to report.
    /// This method handles encoding and writing the span data
    /// (start and end times) to the specified KVP file for Hyper-V telemetry.
    ///
    /// # Arguments
    /// * `id` - The unique identifier for the span.
    /// * `ctx` - The current tracing context, which can be used to access the span.
    fn on_close(&self, id: Id, ctx: TracingContext<S>) {
        if let Some(span) = ctx.span(&id) {
            let mut status = "success".to_string();
            let mut event_level = "INFO".to_string();

            if let Some(recorded_status) = span.extensions().get::<String>() {
                if recorded_status == "failure" {
                    status = "failure".to_string();
                    event_level = "ERROR".to_string();
                }
            }

            let end_time = SystemTime::now();

            handle_span(
                &span,
                self.file_path.as_path(),
                &status,
                &event_level,
                end_time,
            );
        }
    }
}

/// This function serves as a wrapper that orchestrates the necessary steps to log
/// telemetry data to a file. It first truncates the guest pool file if needed, then
/// generates a unique event key using the provided event metadata, encodes the key-value
/// pair, and writes the result to the KVP file.
pub fn handle_kvp_operation(
    file_path: &Path,
    event_level: &str,
    event_name: &str,
    span_id: &str,
    event_value: &str,
) {
    truncate_guest_pool_file(file_path).expect("Failed to truncate KVP file");

    let event_key = generate_event_key(event_level, event_name, span_id);

    let encoded_kvp = encode_kvp_item(&event_key, event_value);
    if let Err(e) = write_to_kvp_file(file_path, &encoded_kvp) {
        eprintln!("Error writing to KVP file: {}", e);
    }
}

/// Generates a unique event key by combining the event level, name, and span ID.
///
/// # Arguments
/// * `event_level` - The logging level (e.g., "INFO", "DEBUG").
/// * `event_name` - The name of the event.
/// * `span_id` - A unique identifier for the span.
fn generate_event_key(
    event_level: &str,
    event_name: &str,
    span_id: &str,
) -> String {
    format!(
        "{}|{}|{}|{}",
        EVENT_PREFIX, event_level, event_name, span_id
    )
}

/// Writes the encoded key-value pair (KVP) data to a specified KVP file.
///
/// This function appends the provided encoded KVP data to the specified file and ensures
/// that file locking is handled properly to avoid race conditions. It locks the file exclusively,
/// writes the data, flushes the output, and then unlocks the file.
fn write_to_kvp_file(
    file_path: &Path,
    encoded_kvp: &Vec<Vec<u8>>,
) -> io::Result<()> {
    let mut file =
        match OpenOptions::new().append(true).create(true).open(file_path) {
            Ok(file) => file,
            Err(e) => {
                eprintln!("Failed to open log file: {}", e);
                return Err(e); // Return the error if the file can't be opened
            }
        };

    let fd = file.as_raw_fd();
    if let Err(e) = flock(fd, FlockArg::LockExclusive) {
        eprintln!("Failed to lock the file: {}", e);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "File locking failed",
        ));
    }

    // Write the encoded KVP data
    for kvp in encoded_kvp {
        if let Err(e) = file.write_all(&kvp[..]) {
            eprintln!("Failed to write to log file: {}", e);
            return Err(e); // Return the error if writing fails
        }
    }

    if let Err(e) = file.flush() {
        eprintln!("Failed to flush the log file: {}", e);
        return Err(e); // Return the error if flushing fails
    }

    if let Err(e) = flock(fd, FlockArg::Unlock) {
        eprintln!("Failed to unlock the file: {}", e);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "File unlocking failed",
        ));
    }

    Ok(())
}

/// Encodes a key-value pair (KVP) into one or more byte slices. If the value
/// exceeds the allowed size, it is split into multiple slices for encoding.
/// This is used for logging events to a KVP file.
///
/// # Arguments
/// * `key` - The key as a string slice.
/// * `value` - The value associated with the key.
fn encode_kvp_item(key: &str, value: &str) -> Vec<Vec<u8>> {
    let key_bytes = key.as_bytes();
    let value_bytes = value.as_bytes();

    let key_len = key_bytes.len().min(HV_KVP_EXCHANGE_MAX_KEY_SIZE);
    let mut key_buf = vec![0u8; HV_KVP_EXCHANGE_MAX_KEY_SIZE];
    key_buf[..key_len].copy_from_slice(&key_bytes[..key_len]);

    if value_bytes.len() <= HV_KVP_AZURE_MAX_VALUE_SIZE {
        let mut value_buf = vec![0u8; HV_KVP_EXCHANGE_MAX_VALUE_SIZE];
        let value_len = value_bytes.len().min(HV_KVP_EXCHANGE_MAX_VALUE_SIZE);
        value_buf[..value_len].copy_from_slice(&value_bytes[..value_len]);

        vec![encode_kvp_slice(key_buf, value_buf)]
    } else {
        println!("Value exceeds max size, splitting into multiple slices.");

        let mut kvp_slices = Vec::new();
        let mut start = 0;
        while start < value_bytes.len() {
            let end =
                (start + HV_KVP_AZURE_MAX_VALUE_SIZE).min(value_bytes.len());
            let mut value_buf = vec![0u8; HV_KVP_EXCHANGE_MAX_VALUE_SIZE];
            value_buf[..end - start].copy_from_slice(&value_bytes[start..end]);

            kvp_slices.push(encode_kvp_slice(key_buf.clone(), value_buf));
            start += HV_KVP_AZURE_MAX_VALUE_SIZE;
        }
        kvp_slices
    }
}

/// Combines the key and value of a KVP into a single byte slice, ensuring
/// proper formatting for consumption by hv_kvp_daemon service,
/// which typically reads from /var/lib/hyperv/.kvp_pool_1.
fn encode_kvp_slice(key: Vec<u8>, value: Vec<u8>) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(
        HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE,
    );
    buffer.extend_from_slice(&key);
    buffer.extend_from_slice(&value);
    buffer
}

/// Decodes a KVP byte slice into its corresponding key and value strings.
/// This is useful for inspecting or logging raw KVP data.
pub fn decode_kvp_item(
    record_data: &[u8],
) -> Result<(String, String), &'static str> {
    let record_data_len = record_data.len();
    let expected_len =
        HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE;

    if record_data_len != expected_len {
        return Err("record_data len not correct.");
    }

    let key = String::from_utf8(
        record_data[0..HV_KVP_EXCHANGE_MAX_KEY_SIZE].to_vec(),
    )
    .unwrap_or_else(|_| String::new())
    .trim_end_matches('\x00')
    .to_string();

    let value = String::from_utf8(
        record_data[HV_KVP_EXCHANGE_MAX_KEY_SIZE..record_data_len].to_vec(),
    )
    .unwrap_or_else(|_| String::new())
    .trim_end_matches('\x00')
    .to_string();

    Ok((key, value))
}

/// Truncates the guest pool KVP file if it contains stale data (i.e., data
/// older than the system's boot time). Logs whether the file was truncated
/// or no action was needed.
fn truncate_guest_pool_file(kvp_file: &Path) -> Result<(), Error> {
    let boot_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::new(std::io::ErrorKind::Other, e))?
        .as_secs()
        - get_uptime()?.as_secs();

    match kvp_file.metadata() {
        Ok(metadata) => {
            if metadata.mtime() < boot_time as i64 {
                OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(kvp_file)?;
                println!("Truncated the KVP file due to stale data.");
            } else {
                println!(
                    "File has been truncated since boot, no action taken."
                );
            }
        }
        Err(ref e) if e.kind() == ErrorKind::NotFound => {
            println!("File not found: {:?}", kvp_file);
            return Ok(());
        }
        Err(e) => {
            return Err(e);
        }
    }

    Ok(())
}

/// Reads the system's uptime from `/proc/uptime`, which can be used for
/// various time-based calculations or checks, such as determining whether
/// a file contains stale data.
fn get_uptime() -> Result<Duration, Error> {
    let uptime = std::fs::read_to_string("/proc/uptime")?;
    let uptime_seconds: f64 = uptime
        .split_whitespace()
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0);
    Ok(Duration::from_secs(uptime_seconds as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tokio::time::{sleep, Duration};
    use tracing::instrument;
    use tracing::{event, Level};
    use tracing_subscriber::{layer::SubscriberExt, Registry};

    #[instrument]
    async fn mock_child_function(index: usize) {
        event!(
            Level::INFO,
            msg = format!("Event in child span for item {}", index)
        );
        sleep(Duration::from_millis(200)).await;
    }

    #[instrument]
    async fn mock_provision() -> Result<(), anyhow::Error> {
        let kernel_version = sys_info::os_release()
            .unwrap_or("Unknown Kernel Version".to_string());
        let os_version =
            sys_info::os_type().unwrap_or("Unknown OS Version".to_string());
        let azure_init_version = env!("CARGO_PKG_VERSION");

        event!(
            Level::INFO,
            msg = format!(
                "Kernel Version: {}, OS Version: {}, Azure-Init Version: {}",
                kernel_version, os_version, azure_init_version
            )
        );

        event!(Level::INFO, msg = "Provisioning started");

        mock_child_function(0).await;
        sleep(Duration::from_millis(300)).await;

        event!(Level::INFO, msg = "Provisioning completed");

        Ok(())
    }

    #[instrument]
    async fn mock_failure_function() -> Result<(), anyhow::Error> {
        let error_message = "Simulated failure during processing";
        event!(Level::ERROR, msg = %error_message);

        sleep(Duration::from_millis(100)).await;

        Err(anyhow::anyhow!(error_message))
    }

    #[tokio::test]
    async fn test_emit_kvp_layer() {
        let temp_file =
            NamedTempFile::new().expect("Failed to create tempfile");
        let temp_path = temp_file.path().to_path_buf();

        let emit_kvp_layer = EmitKVPLayer::new(temp_path.clone());

        let subscriber = Registry::default().with(emit_kvp_layer);
        let default_guard = tracing::subscriber::set_default(subscriber);

        let _ = mock_provision().await;
        let _ = mock_failure_function().await;

        drop(default_guard);

        let contents =
            std::fs::read(temp_path).expect("Failed to read temp file");
        println!("Contents of the file (in bytes):\n{:?}", contents);

        let slice_size = 512 + 2048;

        let num_slices = (contents.len() + slice_size - 1) / slice_size;
        let expected_len = num_slices * slice_size;

        assert_eq!(
            contents.len(),
            expected_len,
            "Encoded buffer length is incorrect. Expected {} but got {}",
            expected_len,
            contents.len()
        );

        for i in 0..num_slices {
            let start = i * slice_size;
            let end = start + slice_size;
            let slice = &contents[start..end];

            println!("Processing slice {}: start={}, end={}", i, start, end);
            println!("Slice length: {}", slice.len());

            let key_section = &slice[..512];
            let value_section = &slice[512..];

            match decode_kvp_item(slice) {
                Ok((key, value)) => {
                    println!("Decoded KVP - Key: {}", key);
                    println!("Decoded KVP - Value: {}\n", value);
                }
                Err(e) => {
                    panic!("Failed to decode KVP: {}", e);
                }
            }

            assert!(
                key_section.iter().any(|&b| b != 0),
                "Key section in slice {} should contain non-zero bytes",
                i
            );

            assert!(
                value_section.iter().any(|&b| b != 0),
                "Value section in slice {} should contain non-zero bytes",
                i
            );
        }
    }
}
