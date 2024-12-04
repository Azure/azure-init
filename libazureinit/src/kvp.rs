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

use std::{
    fmt::{self as std_fmt, Write as std_write},
    fs::{File, OpenOptions},
    io::{self, Error, ErrorKind, Write},
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use tracing::{
    field::Visit,
    span::{Attributes, Id},
    Subscriber,
};

use tracing_subscriber::{
    layer::Context as TracingContext, registry::LookupSpan, Layer,
};

use sysinfo::{System, SystemExt};

use tokio::sync::{mpsc::UnboundedReceiver, mpsc::UnboundedSender};

//use crate::tracing::{handle_event, handle_span};

use chrono::{DateTime, Utc};
use std::fmt;
use uuid::Uuid;

const HV_KVP_EXCHANGE_MAX_KEY_SIZE: usize = 512;
const HV_KVP_EXCHANGE_MAX_VALUE_SIZE: usize = 2048;
const HV_KVP_AZURE_MAX_VALUE_SIZE: usize = 1024;
const EVENT_PREFIX: &str = concat!("azure-init-", env!("CARGO_PKG_VERSION"));

/// A wrapper around `std::time::Instant` that provides convenient methods
/// for time tracking in spans and events. Implements the `Deref` trait, allowing
/// access to the underlying `Instant` methods.
///
/// This struct captures the start time of spans/events and measures the elapsed time.
#[derive(Clone)]
struct MyInstant(Instant);

impl std::ops::Deref for MyInstant {
    type Target = Instant;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl MyInstant {
    pub fn now() -> Self {
        MyInstant(Instant::now())
    }
}

/// A custom visitor that captures the value of the `msg` field from a tracing event.
/// It implements the `tracing::field::Visit` trait and records the value into
/// a provided mutable string reference.
///
/// This visitor is primarily used in the `on_event` method of the `EmitKVPLayer`
/// to extract event messages and log them as key-value pairs.
pub struct StringVisitor<'a> {
    string: &'a mut String,
}

impl Visit for StringVisitor<'_> {
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

/// Represents the state of a span within the `EmitKVPLayer`.
enum SpanStatus {
    Success,
    Failure,
}

impl SpanStatus {
    fn as_str(&self) -> &'static str {
        match self {
            SpanStatus::Success => "success",
            SpanStatus::Failure => "failure",
        }
    }

    fn level(&self) -> &'static str {
        match self {
            SpanStatus::Success => "INFO",
            SpanStatus::Failure => "ERROR",
        }
    }
}

impl fmt::Display for SpanStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
/// A custom tracing layer that emits span and event data as key-value pairs (KVP)
/// to a file for Hyper-V telemetry consumption. The layer manages the asynchronous
/// writing of telemetry data to a specified file in KVP format.
///
/// `EmitKVPLayer` initializes the file at creation, manages a dedicated writer
/// task, and provides functions to send encoded data for logging.
pub struct EmitKVPLayer {
    events_tx: UnboundedSender<Vec<u8>>,
}

impl EmitKVPLayer {
    /// Creates a new `EmitKVPLayer`, initializing the log file and starting
    /// an asynchronous writer task for handling incoming KVP data.
    ///
    /// # Arguments
    /// * `file_path` - The file path where the KVP data will be stored.
    ///
    pub fn new(file_path: std::path::PathBuf) -> Result<Self, std::io::Error> {
        truncate_guest_pool_file(&file_path)?;

        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)?;

        let (events_tx, events_rx): (
            UnboundedSender<Vec<u8>>,
            UnboundedReceiver<Vec<u8>>,
        ) = tokio::sync::mpsc::unbounded_channel();

        tokio::spawn(Self::kvp_writer(file, events_rx));

        Ok(Self { events_tx })
    }

    /// An asynchronous task that serializes incoming KVP data to the specified file.
    /// This function manages the file I/O operations to ensure the data is written
    /// and flushed consistently.
    ///
    /// # Arguments
    /// * `file` - The file where KVP data will be written.
    /// * `events` - A receiver that provides encoded KVP data as it arrives.
    async fn kvp_writer(
        mut file: File,
        mut events: UnboundedReceiver<Vec<u8>>,
    ) -> io::Result<()> {
        while let Some(encoded_kvp) = events.recv().await {
            if let Err(e) = file.write_all(&encoded_kvp) {
                eprintln!("Failed to write to log file: {}", e);
            }
            if let Err(e) = file.flush() {
                eprintln!("Failed to flush the log file: {}", e);
            }
        }
        Ok(())
    }

    /// Sends encoded KVP data to the writer task for asynchronous logging.
    ///
    /// # Arguments
    /// * `message` - The encoded data to send as a vector of bytes (Vec<u8>).
    pub fn send_event(&self, message: Vec<u8>) {
        let _ = self.events_tx.send(message);
    }
}

impl<S> Layer<S> for EmitKVPLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    /// Handles event occurrences within a span, capturing and recording the event's message
    /// and context metadata as key-value pairs (KVP) for logging.
    ///
    /// This function extracts the event's `msg` field using `StringVisitor`, constructs a
    /// formatted event string, and then encodes it as KVP data to be sent to the
    /// `EmitKVPLayer` for asynchronous file storage.
    ///
    /// If an `ERROR` level event is encountered, it marks the span's status as a failure,
    /// which will be reflected in the span's data upon closure.
    ///
    /// # Arguments
    /// * `event` - The tracing event instance containing the message and metadata.
    /// * `ctx` - The current tracing context, which is used to access the span associated
    ///   with the event.
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
                extensions.insert(SpanStatus::Failure);
            }

            let span_context = span.metadata();
            let span_id: Uuid = Uuid::new_v4();

            let event_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_else(|_| {
                    span.extensions()
                        .get::<MyInstant>()
                        .map(|instant| instant.elapsed())
                        .unwrap_or_default()
                });

            let event_time_dt = DateTime::<Utc>::from(UNIX_EPOCH + event_time)
                .format("%Y-%m-%dT%H:%M:%S%.3fZ");

            let event_value =
                format!("Time: {} | Event: {}", event_time_dt, event_message);

            handle_kvp_operation(
                self,
                "INFO",
                span_context.name(),
                &span_id.to_string(),
                &event_value,
            );
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
    /// Called when a span is closed, finalizing and logging the span's data. This method
    /// records the span's start and end times, status (e.g., success or failure), and other metadata,
    /// then sends it to `EmitKVPLayer` for KVP logging.
    ///
    /// If any errors were recorded in the span (such as `ERROR` level events), the span
    /// status is marked as `Failure`; otherwise, it is marked as `Success`.
    ///
    /// # Arguments
    /// * `id` - The unique identifier for the span.
    /// * `ctx` - The current tracing context, used to access the span's metadata and status.
    fn on_close(&self, id: Id, ctx: TracingContext<S>) {
        if let Some(span) = ctx.span(&id) {
            let span_status = if span.extensions().get::<SpanStatus>().is_some()
            {
                SpanStatus::Failure
            } else {
                SpanStatus::Success
            };

            let end_time = SystemTime::now();

            let span_context = span.metadata();
            let span_id = Uuid::new_v4();

            if let Some(start_instant) = span.extensions().get::<MyInstant>() {
                let elapsed = start_instant.elapsed();

                let start_time =
                    end_time.checked_sub(elapsed).unwrap_or(UNIX_EPOCH);

                let start_time_dt = DateTime::<Utc>::from(
                    UNIX_EPOCH
                        + start_time
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default(),
                )
                .format("%Y-%m-%dT%H:%M:%S%.3fZ");

                let end_time_dt = DateTime::<Utc>::from(
                    UNIX_EPOCH
                        + end_time
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default(),
                )
                .format("%Y-%m-%dT%H:%M:%S%.3fZ");

                let event_value = format!(
                    "Start: {} | End: {} | Status: {}",
                    start_time_dt, end_time_dt, span_status
                );

                handle_kvp_operation(
                    self,
                    span_status.level(),
                    span_context.name(),
                    &span_id.to_string(),
                    &event_value,
                );
            }
        }
    }
}

/// Handles the orchestration of key-value pair (KVP) encoding and logging operations
/// by generating a unique event key, encoding it with the provided value, and sending
/// it to the `EmitKVPLayer` for logging.
pub fn handle_kvp_operation(
    emit_kvp_layer: &EmitKVPLayer,
    event_level: &str,
    event_name: &str,
    span_id: &str,
    event_value: &str,
) {
    let event_key = generate_event_key(event_level, event_name, span_id);
    let encoded_kvp = encode_kvp_item(&event_key, event_value);
    let encoded_kvp_flattened: Vec<u8> = encoded_kvp.concat();
    emit_kvp_layer.send_event(encoded_kvp_flattened);
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

/// Retrieves the system's uptime using the `sysinfo` crate, returning the duration
/// since the system booted. This can be useful for time-based calculations or checks,
/// such as determining whether data is stale or calculating the approximate boot time.
fn get_uptime() -> Result<Duration, Error> {
    let mut system = System::new();
    system.refresh_system();

    let uptime_seconds = system.uptime();
    Ok(Duration::from_secs(uptime_seconds))
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
        let mut system = System::new();
        system.refresh_system();

        let kernel_version = system
            .kernel_version()
            .unwrap_or("Unknown Kernel Version".to_string());
        let os_version = system
            .os_version()
            .unwrap_or("Unknown OS Version".to_string());
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

        let emit_kvp_layer = EmitKVPLayer::new(temp_path.clone())
            .expect("Failed to create EmitKVPLayer");

        let subscriber = Registry::default().with(emit_kvp_layer);
        let default_guard = tracing::subscriber::set_default(subscriber);

        let _ = mock_provision().await;
        let _ = mock_failure_function().await;

        sleep(Duration::from_secs(1)).await;

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

    #[tokio::test]
    async fn test_truncate_guest_pool_file() {
        let temp_file =
            NamedTempFile::new().expect("Failed to create tempfile");
        let temp_path = temp_file.path().to_path_buf();

        std::fs::write(&temp_path, "Some initial data")
            .expect("Failed to write initial data");

        let result = truncate_guest_pool_file(&temp_path);

        assert!(
            result.is_ok(),
            "truncate_guest_pool_file returned an error: {:?}",
            result
        );

        if let Ok(contents) = std::fs::read_to_string(&temp_path) {
            if contents.is_empty() {
                println!("File was truncated as expected.");
            } else {
                println!("File was not truncated (this is expected if file has been truncated since boot).");
            }
        } else {
            panic!("Failed to read the temp file after truncation attempt.");
        }
    }
}
