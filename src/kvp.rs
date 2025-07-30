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
    collections::HashMap,
    fmt::{self as std_fmt, Write as std_write},
    fs::{File, OpenOptions},
    io::{self, ErrorKind, Write},
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

use sysinfo::System;

use tokio::{
    sync::{mpsc::UnboundedReceiver, mpsc::UnboundedSender},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use chrono::{DateTime, Utc};
use libazureinit::health::encoded_success_report;
use std::fmt;
use uuid::Uuid;

use libazureinit::error::Error as LibError;

const HV_KVP_EXCHANGE_MAX_KEY_SIZE: usize = 512;
const HV_KVP_EXCHANGE_MAX_VALUE_SIZE: usize = 2048;
const HV_KVP_AZURE_MAX_VALUE_SIZE: usize = 1022;
const EVENT_PREFIX: &str = concat!("azure-init-", env!("CARGO_PKG_VERSION"));

/// Encapsulates the KVP (Key-Value Pair) tracing infrastructure.
///
/// This struct holds both the `tracing` layer (`EmitKVPLayer`) that generates
/// telemetry data and the `JoinHandle` for the background task that writes this
/// data to the KVP file. This allows the caller to manage the lifecycle of the
/// writer task separately from the tracing layer.
pub struct Kvp {
    /// The `tracing` layer that captures span and event data and sends it
    /// to the KVP writer task.
    pub tracing_layer: EmitKVPLayer,
    /// The `JoinHandle` for the background task responsible for writing
    /// KVP data to the file. The caller can use this handle to wait for
    /// the writer to finish.
    pub writer: JoinHandle<io::Result<()>>,
}

impl Kvp {
    /// Creates a new `Kvp` instance, spawning a background task for writing
    /// KVP telemetry data to a file.
    ///
    /// This function initializes the necessary components for KVP logging:
    /// - It truncates the KVP file if it contains stale data.
    /// - It creates an unbounded channel for passing encoded KVP data from the
    ///   tracing layer to the writer task.
    /// - It spawns the `kvp_writer` task, which listens for data and shutdown signals.
    pub fn new(
        file_path: std::path::PathBuf,
        vm_id: &str,
        graceful_shutdown: CancellationToken,
    ) -> Result<Self, anyhow::Error> {
        truncate_guest_pool_file(&file_path)?;

        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)?;

        let (events_tx, events_rx): (
            UnboundedSender<Vec<u8>>,
            UnboundedReceiver<Vec<u8>>,
        ) = tokio::sync::mpsc::unbounded_channel();

        let writer =
            tokio::spawn(Self::kvp_writer(file, events_rx, graceful_shutdown));

        Ok(Self {
            tracing_layer: EmitKVPLayer {
                events_tx,
                vm_id: vm_id.to_string(),
            },
            writer,
        })
    }

    /// The background task that writes encoded KVP data to a file.
    ///
    /// This asynchronous function runs in a loop, waiting for two events:
    /// 1. Receiving encoded KVP data from the `events` channel, which it then
    ///    writes to the specified `file`.
    /// 2. A cancellation signal from the `token`.
    ///
    /// Upon receiving the cancellation signal, it stops accepting new events,
    /// drains the `events` channel of any remaining messages, and writes them
    /// to the file before exiting gracefully.
    async fn kvp_writer(
        mut file: File,
        mut events: UnboundedReceiver<Vec<u8>>,
        token: CancellationToken,
    ) -> io::Result<()> {
        loop {
            tokio::select! {
                biased;

                Some(encoded_kvp) = events.recv() => {
                    if let Err(e) = file.write_all(&encoded_kvp) {
                        eprintln!("Failed to write to log file: {e}");
                    }
                    if let Err(e) = file.flush() {
                         eprintln!("Failed to flush the log file: {e}");
                    }
                }

                _ = token.cancelled() => {
                    // Shutdown signal received.
                    // close the channel and drain remaining messages.
                    events.close();
                    while let Some(encoded_kvp) = events.recv().await {
                        if let Err(e) = file.write_all(&encoded_kvp) {
                            eprintln!("Failed to write to log file during shutdown: {e}");
                        }
                        if let Err(e) = file.flush() {
                            eprintln!("Failed to flush the log file during shutdown: {e}");
                        }
                    }
                    // All messages are drained, exit the loop.
                    break;
                }
            }
        }
        Ok(())
    }
}

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
        field: &tracing::field::Field,
        value: &dyn std_fmt::Debug,
    ) {
        write!(self.string, "{}={:?}; ", field.name(), value)
            .expect("Writing to a string should never fail");
    }
}

/// Represents the state of a span within the `EmitKVPLayer`.
#[derive(Copy, Clone, Debug)]
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
    vm_id: String,
}

impl EmitKVPLayer {
    /// Sends encoded KVP data to the writer task for asynchronous logging.
    ///
    /// # Arguments
    /// * `message` - The encoded data to send as a vector of bytes (Vec<u8>).
    pub fn send_event(&self, message: Vec<u8>) {
        let _ = self.events_tx.send(message);
    }

    /// Handles the orchestration of key-value pair (KVP) encoding and logging operations
    /// by generating a unique event key, encoding it with the provided value, and sending
    /// it to the `EmitKVPLayer` for logging.
    pub fn handle_kvp_operation(
        &self,
        event_level: &str,
        event_name: &str,
        span_id: &str,
        event_value: &str,
    ) {
        let event_key =
            generate_event_key(&self.vm_id, event_level, event_name, span_id);
        let encoded_kvp = encode_kvp_item(&event_key, event_value);
        let encoded_kvp_flattened: Vec<u8> = encoded_kvp.concat();
        self.send_event(encoded_kvp_flattened);
    }

    /// Emit a health KVP report for success, failure, or in-progress.
    pub fn handle_health_report(
        &self,
        event: &tracing::Event<'_>,
        status: &str,
    ) {
        let mut reason: Option<String> = None;
        let mut supporting_data: Option<HashMap<String, String>> = None;
        let mut optional_key_value: Option<(String, String)> = None;

        event.record(
            &mut |field: &tracing::field::Field,
                  value: &dyn std::fmt::Debug| {
                match field.name() {
                    "reason" => {
                        reason = Some(
                            format!("{value:?}").trim_matches('"').to_string(),
                        );
                    }
                    "supporting_data" => {
                        let raw = format!("{value:?}");
                        let mut map = HashMap::new();
                        let entries = raw
                            .trim_matches(|c| c == '{' || c == '}')
                            .split(',');
                        for entry in entries {
                            let parts: Vec<&str> = entry
                                .split(':')
                                .map(|s| s.trim().trim_matches('"'))
                                .collect();
                            if parts.len() == 2 {
                                map.insert(
                                    parts[0].to_string(),
                                    parts[1].to_string(),
                                );
                            }
                        }
                        if !map.is_empty() {
                            supporting_data = Some(map);
                        }
                    }
                    "optional_key_value" => {
                        let raw =
                            format!("{value:?}").trim_matches('"').to_string();
                        if let Some((k, v)) = raw.split_once('=') {
                            optional_key_value = Some((
                                k.trim().to_string(),
                                v.trim().to_string(),
                            ));
                        }
                    }
                    _ => {}
                }
            },
        );

        let provisioning_status: Option<String> = match status {
            "success" => {
                let okv = optional_key_value
                    .as_ref()
                    .map(|(k, v)| (k.as_str(), v.as_str()));
                Some(encoded_success_report(&self.vm_id, okv))
            }
            "failure" => {
                let reason_str = reason.as_deref().unwrap_or("Unknown failure");
                let mut details = reason_str.to_string();
                if let Some(kvs) = supporting_data.as_ref() {
                    let mut extra = String::new();
                    for (k, v) in kvs {
                        extra.push_str(&format!("; {k}={v}"));
                    }
                    if !extra.is_empty() {
                        details.push_str(&extra);
                    }
                }
                let err = LibError::UnhandledError {
                    details: details.to_string(),
                };
                Some(err.as_encoded_report(&self.vm_id))
            }
            "in progress" => {
                let desc = format!(
                    "Provisioning is still in progress for vm_id={}.",
                    self.vm_id
                );
                Some(desc)
            }
            _ => {
                tracing::warn!(%status, "Invalid health report type");
                None
            }
        };

        if let Some(report_str) = provisioning_status {
            let msg =
                encode_kvp_item("PROVISIONING_REPORT", &report_str).concat();
            self.send_event(msg);
        }
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
    /// Additionally, this function checks if the event contains a `health_report` field.
    /// If present, the event is delegated to [`handle_health_report`] to be uniquely formatted.
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
        // Check for health_report events, as they exist outside of a span.
        let mut health_report = None;
        event.record(
            &mut |field: &tracing::field::Field,
                  value: &dyn std::fmt::Debug| {
                if field.name() == "health_report" {
                    health_report = Some(
                        format!("{value:?}").trim_matches('"').to_string(),
                    );
                }
            },
        );

        if let Some(health_str) = health_report {
            self.handle_health_report(event, &health_str);
            return;
        }

        // All other events are inside a span.
        if let Some(span) = ctx.lookup_current() {
            let mut event_message = String::new();
            let mut visitor = StringVisitor {
                string: &mut event_message,
            };
            event.record(&mut visitor);

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
                format!("Time: {event_time_dt} | Event: {event_message}");

            self.handle_kvp_operation(
                event.metadata().level().as_str(),
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
            let span_status = span
                .extensions()
                .get::<SpanStatus>()
                .copied()
                .unwrap_or(SpanStatus::Success);

            let end_time = SystemTime::now();

            let span_context = span.metadata();
            let span_id = Uuid::new_v4();

            if let Some(start_instant) = span.extensions().get::<MyInstant>() {
                let elapsed = start_instant.elapsed();

                let start_time =
                    end_time.checked_sub(elapsed).unwrap_or(UNIX_EPOCH);

                let start_time_dt = DateTime::<Utc>::from(start_time)
                    .format("%Y-%m-%dT%H:%M:%S%.3fZ");

                let end_time_dt = DateTime::<Utc>::from(end_time)
                    .format("%Y-%m-%dT%H:%M:%S%.3fZ");

                let event_value = format!(
                    "Start: {start_time_dt} | End: {end_time_dt} | Status: {span_status}"
                );

                self.handle_kvp_operation(
                    span_status.level(),
                    span_context.name(),
                    &span_id.to_string(),
                    &event_value,
                );
            }
        }
    }
}

/// Generates a unique event key by combining the event level, name, and span ID.
///
/// # Arguments
/// * `event_level` - The logging level (e.g., "INFO", "DEBUG").
/// * `event_name` - The name of the event.
/// * `span_id` - A unique identifier for the span.
fn generate_event_key(
    vm_id: &str,
    event_level: &str,
    event_name: &str,
    span_id: &str,
) -> String {
    format!("{EVENT_PREFIX}|{vm_id}|{event_level}|{event_name}|{span_id}")
}

/// Encodes a key-value pair (KVP) into one or more byte slices. If the value
/// exceeds the allowed size, it is split into multiple slices for encoding.
/// This is used for logging events to a KVP file.
///
/// # Note
/// - The key is zero-padded to `HV_KVP_EXCHANGE_MAX_KEY_SIZE`, and
///   the value is zero-padded to `HV_KVP_AZURE_MAX_VALUE_SIZE` to meet
///   Hyper-V's expected formatting.
///
/// # Arguments
/// * `key` - The key as a string slice.
/// * `value` - The value associated with the key.
fn encode_kvp_item(key: &str, value: &str) -> Vec<Vec<u8>> {
    let key_buf = key
        .as_bytes()
        .iter()
        .take(HV_KVP_EXCHANGE_MAX_KEY_SIZE)
        .chain(
            vec![0_u8; HV_KVP_EXCHANGE_MAX_KEY_SIZE.saturating_sub(key.len())]
                .iter(),
        )
        .copied()
        .collect::<Vec<_>>();

    debug_assert!(key_buf.len() == HV_KVP_EXCHANGE_MAX_KEY_SIZE);

    let kvp_slices = value
        .as_bytes()
        .chunks(HV_KVP_AZURE_MAX_VALUE_SIZE)
        .map(|chunk| {
            let mut buffer = Vec::with_capacity(
                HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE,
            );
            buffer.extend_from_slice(&key_buf);
            buffer.extend_from_slice(chunk);
            while buffer.len()
                < HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE
            {
                buffer.push(0);
            }

            buffer
        })
        .collect::<Vec<Vec<u8>>>();

    debug_assert!(kvp_slices.iter().all(|kvp| kvp.len()
        == HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE));

    kvp_slices
}

/// Decodes a KVP byte slice into its corresponding key and value strings.
/// This is useful for inspecting or logging raw KVP data.
#[cfg(test)]
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
        record_data
            .iter()
            .take(HV_KVP_EXCHANGE_MAX_KEY_SIZE)
            .cloned()
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| String::new())
    .trim_end_matches('\x00')
    .to_string();

    let value = String::from_utf8(
        record_data
            .iter()
            .skip(HV_KVP_EXCHANGE_MAX_KEY_SIZE)
            .take(HV_KVP_AZURE_MAX_VALUE_SIZE)
            .cloned()
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| String::new())
    .trim_end_matches('\x00')
    .to_string();

    Ok((key, value))
}

/// Truncates the guest pool KVP file if it contains stale data (i.e., data
/// older than the system's boot time). Logs whether the file was truncated
/// or no action was needed.
fn truncate_guest_pool_file(kvp_file: &Path) -> Result<(), anyhow::Error> {
    let boot_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
        - get_uptime().as_secs();

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
            println!("File not found: {kvp_file:?}");
            return Ok(());
        }
        Err(e) => {
            return Err(anyhow::Error::from(e)
                .context("Failed to access file metadata"));
        }
    }

    Ok(())
}

/// Retrieves the system's uptime using the `sysinfo` crate, returning the duration
/// since the system booted. This can be useful for time-based calculations or checks,
/// such as determining whether data is stale or calculating the approximate boot time.
fn get_uptime() -> Duration {
    let mut system = System::new();
    system.refresh_memory();
    system.refresh_cpu_usage();

    let uptime_seconds = System::uptime();
    Duration::from_secs(uptime_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use libazureinit::config::{Config, Telemetry};
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
        system.refresh_memory();
        system.refresh_cpu_usage();

        let kernel_version = System::kernel_version()
            .unwrap_or("Unknown Kernel Version".to_string());
        let os_version =
            System::os_version().unwrap_or("Unknown OS Version".to_string());
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
        event!(
            Level::INFO,
            health_report = "success",
            optional_key_value = "origin=mock_source",
            "Provisioning completed successfully"
        );

        Ok(())
    }

    #[instrument]
    async fn mock_failure_function() -> Result<(), anyhow::Error> {
        let error_message = "Simulated failure during processing";
        let mut supporting = HashMap::new();
        supporting.insert("step", "mock_stuff");
        supporting.insert("source", "unit_test");
        event!(
            Level::ERROR,
            health_report = "failure",
            reason = "Simulated failure during processing",
            supporting_data = ?supporting,
            "Provisioning failed"
        );

        sleep(Duration::from_millis(100)).await;
        Err(anyhow::anyhow!(error_message))
    }

    #[tokio::test]
    async fn test_emit_kvp_layer() {
        let temp_file =
            NamedTempFile::new().expect("Failed to create tempfile");
        let temp_path = temp_file.path().to_path_buf();

        let test_vm_id = "00000000-0000-0000-0000-000000000001";

        let graceful_shutdown = CancellationToken::new();
        let kvp =
            Kvp::new(temp_path.clone(), test_vm_id, graceful_shutdown.clone())
                .expect("Failed to create Kvp");

        let subscriber = Registry::default().with(kvp.tracing_layer);
        let default_guard = tracing::subscriber::set_default(subscriber);

        let _ = mock_provision().await;
        let _ = mock_failure_function().await;

        drop(default_guard);
        graceful_shutdown.cancel();
        kvp.writer
            .await
            .expect("KVP writer task panicked")
            .expect("KVP writer task returned an IO error");

        let contents =
            std::fs::read(temp_path).expect("Failed to read temp file");
        println!("Contents of the file (in bytes):\n{contents:?}");

        let slice_size =
            HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE;

        let num_slices = contents.len().div_ceil(slice_size);
        let expected_len = num_slices * slice_size;

        assert_eq!(
            contents.len(),
            expected_len,
            "Encoded buffer length is incorrect. Expected {} but got {}",
            expected_len,
            contents.len()
        );

        let mut found_success = false;
        let mut found_failure = false;

        for i in 0..num_slices {
            let start = i * slice_size;
            let end = start + slice_size;
            let slice = &contents[start..end];

            println!("Processing slice {i}: start={start}, end={end}");
            println!("Slice length: {}", slice.len());

            let key_section = &slice[..HV_KVP_EXCHANGE_MAX_KEY_SIZE];
            let value_section = &slice[HV_KVP_EXCHANGE_MAX_KEY_SIZE..];

            match decode_kvp_item(slice) {
                Ok((key, value)) => {
                    println!("Decoded KVP - Key: {key}");
                    println!("Decoded KVP - Value: {value}\n");

                    // Check for success or failure reports
                    if key == "PROVISIONING_REPORT"
                        && value.contains("result=success")
                    {
                        found_success = true;
                    }

                    if key == "PROVISIONING_REPORT"
                        && value.contains("result=error")
                    {
                        found_failure = true;
                    }
                }
                Err(e) => {
                    panic!("Failed to decode KVP: {e}");
                }
            }

            assert!(
                key_section.iter().any(|&b| b != 0),
                "Key section in slice {i} should contain non-zero bytes"
            );

            assert!(
                value_section.iter().any(|&b| b != 0),
                "Value section in slice {i} should contain non-zero bytes"
            );
        }

        assert!(
            found_success,
            "Expected to find a 'result=success' entry but did not."
        );
        assert!(
            found_failure,
            "Expected to find a 'result=error' entry but did not."
        );
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
            "truncate_guest_pool_file returned an error: {result:?}",
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

    #[test]
    fn test_encode_kvp_item_value_length() {
        let key = "test_key";
        let value = "A".repeat(HV_KVP_AZURE_MAX_VALUE_SIZE * 2 + 50);

        let encoded_slices = encode_kvp_item(key, &value);

        assert!(
            !encoded_slices.is_empty(),
            "Encoded slices should not be empty"
        );

        for (i, slice) in encoded_slices.iter().enumerate() {
            assert_eq!(
                slice.len(),
                HV_KVP_EXCHANGE_MAX_KEY_SIZE + HV_KVP_EXCHANGE_MAX_VALUE_SIZE,
                "Slice {i} length is incorrect",
            );

            let (decoded_key, decoded_value) =
                decode_kvp_item(slice).expect("Failed to decode slice");

            println!("Slice {i}: Key: {decoded_key}");
            println!(
                "Slice {i}: Value (length {}): {decoded_value}",
                decoded_value.len()
            );

            assert_eq!(decoded_key, key, "Key mismatch in slice {i}");
            assert!(
                decoded_value.len() <= HV_KVP_AZURE_MAX_VALUE_SIZE,
                "Value length exceeds limit in slice {i}: {} > {HV_KVP_AZURE_MAX_VALUE_SIZE}",
                decoded_value.len()
            );
        }

        println!("All slices adhere to Azure's max value size limit.");
    }

    #[tokio::test]
    async fn test_emit_kvp_layer_disabled() {
        let temp_file =
            NamedTempFile::new().expect("Failed to create tempfile");
        let temp_path = temp_file.path().to_path_buf();

        let test_vm_id = "00000000-0000-0000-0000-000000000002";

        let telemetry_config = Telemetry {
            kvp_diagnostics: false,
        };

        let config = Config {
            telemetry: telemetry_config,
            ..Default::default()
        };

        let kvp_enabled = config.telemetry.kvp_diagnostics;

        let graceful_shutdown = CancellationToken::new();
        let emit_kvp_layer = if kvp_enabled {
            Some(
                Kvp::new(
                    temp_path.clone(),
                    test_vm_id,
                    graceful_shutdown.clone(),
                )
                .expect("Failed to create Kvp")
                .tracing_layer,
            )
        } else {
            None
        };

        let subscriber = Registry::default().with(emit_kvp_layer);
        let default_guard = tracing::subscriber::set_default(subscriber);

        let _ = mock_provision().await;

        sleep(Duration::from_secs(1)).await;

        drop(default_guard);

        let contents =
            std::fs::read(temp_path).expect("Failed to read temp file");

        assert!(
            contents.is_empty(),
            "KVP file should be empty because kvp_diagnostics is disabled, but found data: {contents:?}",
        );

        println!("KVP file is empty as expected because kvp_diagnostics is disabled.");
    }
}
