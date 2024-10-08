// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This module provides core functionality for handling telemetry tracing.

use crate::kvp::handle_kvp_operation;

use chrono::{DateTime, Utc};
use opentelemetry::{global, sdk::trace as sdktrace, trace::TracerProvider};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// A wrapper around `std::time::Instant` that provides convenient methods
/// for time tracking in spans and events. Implements the `Deref` trait, allowing
/// access to the underlying `Instant` methods.
///
/// This struct captures the start time of spans/events and measures the elapsed time.
#[derive(Clone)]
pub struct MyInstant(Instant);
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

/// Initializes the tracing system by setting up a tracing provider and
/// registering it globally. This function returns a tracer instance
/// associated with the "azure-kvp" application.
///
/// # Returns
/// A `sdktrace::Tracer` object that can be used to create and manage spans.
pub fn initialize_tracing() -> sdktrace::Tracer {
    let provider = sdktrace::TracerProvider::builder().build();
    global::set_tracer_provider(provider.clone());
    provider.tracer("azure-kvp")
}

/// Handles span data by truncating the guest pool file, encoding key-value pairs
/// with span metadata, and writing the encoded data to a log file.
///
/// # Arguments
/// * `span` - A reference to the span being processed, which contains the metadata and context.
/// * `file_path` - A reference to the path where the span data should be written.
/// * `status` - A string representing the status of the span (e.g., "success", "failure").
/// * `event_level` - The logging level of the span (e.g., "INFO", "ERROR").
/// * `end_time` - The `SystemTime` representing when the span ended.
pub fn handle_span<S>(
    span: &tracing_subscriber::registry::SpanRef<'_, S>,
    file_path: &Path,
    status: &str,
    event_level: &str,
    end_time: SystemTime,
) where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    let span_context = span.metadata();
    let span_id = Uuid::new_v4();

    if let Some(start_instant) = span.extensions().get::<MyInstant>() {
        let elapsed = start_instant.elapsed();

        let start_time = end_time
            .checked_sub(elapsed)
            .expect("SystemTime subtraction failed");

        let start_time_dt = DateTime::<Utc>::from(
            UNIX_EPOCH
                + std::time::Duration::from_millis(
                    start_time.duration_since(UNIX_EPOCH).unwrap().as_millis()
                        as u64,
                ),
        )
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

        let end_time_dt = DateTime::<Utc>::from(
            UNIX_EPOCH
                + std::time::Duration::from_millis(
                    end_time.duration_since(UNIX_EPOCH).unwrap().as_millis()
                        as u64,
                ),
        )
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

        let event_value = format!(
            "Start: {} | End: {} | Status: {}",
            start_time_dt, end_time_dt, status
        );

        handle_kvp_operation(
            file_path,
            event_level,
            span_context.name(),
            &span_id.to_string(),
            &event_value,
        );
    }
}

/// Handles event data by encoding the message and metadata as key-value pairs (KVP)
/// and writing the encoded data to the specified log file.
///
/// # Arguments
/// * `event_message` - A string message associated with the event.
/// * `span` - The span associated with the event, used to retrieve span context and metadata.
/// * `file_path` - The path to the log file where the encoded KVP data should be written.
/// * `event_instant` - A `MyInstant` representing when the event occurred.
pub fn handle_event<S>(
    event_message: &str,
    span: &tracing_subscriber::registry::SpanRef<'_, S>,
    file_path: &Path,
    event_instant: MyInstant,
) where
    S: tracing::Subscriber
        + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    let span_context = span.metadata();
    let span_id: Uuid = Uuid::new_v4();

    let event_time = SystemTime::now()
        .checked_sub(event_instant.elapsed())
        .expect("SystemTime subtraction failed");

    let event_time_dt = DateTime::<Utc>::from(
        UNIX_EPOCH
            + std::time::Duration::from_millis(
                event_time.duration_since(UNIX_EPOCH).unwrap().as_millis()
                    as u64,
            ),
    )
    .format("%Y-%m-%dT%H:%M:%S%.3fZ")
    .to_string();

    let event_value =
        format!("Time: {} | Event: {}", event_time_dt, event_message);

    handle_kvp_operation(
        file_path,
        "INFO",
        span_context.name(),
        &span_id.to_string(),
        &event_value,
    );
}
