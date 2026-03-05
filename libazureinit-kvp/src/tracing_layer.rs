// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A `tracing_subscriber::Layer` that translates span and event data
//! into diagnostic KVP entries via `DiagnosticsKvp`.

use std::fmt::{self, Debug, Write as _};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use tracing::field::Visit;
use tracing::span::{Attributes, Id};
use tracing::Subscriber;
use tracing_subscriber::layer::Context as TracingContext;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;
use uuid::Uuid;

use crate::diagnostics::{DiagnosticEvent, DiagnosticsKvp};
use crate::KvpStore;

const HV_KVP_AZURE_MAX_VALUE_SIZE: usize = 1022;

/// A wrapper around `std::time::Instant` for time tracking in spans.
///
/// Stored as a span extension to measure elapsed time between
/// `on_new_span` and `on_close`.
#[derive(Clone)]
struct MyInstant(Instant);

impl std::ops::Deref for MyInstant {
    type Target = Instant;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl MyInstant {
    fn now() -> Self {
        MyInstant(Instant::now())
    }
}

/// A visitor that captures all fields from a tracing event into a
/// single string with `field=value` pairs separated by commas.
pub struct StringVisitor<'a> {
    string: &'a mut String,
}

impl Visit for StringVisitor<'_> {
    fn record_debug(
        &mut self,
        field: &tracing::field::Field,
        value: &dyn Debug,
    ) {
        if !self.string.is_empty() {
            self.string.push_str(", ");
        }
        write!(self.string, "{}={:?}", field.name(), value)
            .expect("Writing to a string should never fail");
    }
}

/// Given a span's metadata, constructs a span name in the format
/// `module:function`.
///
/// Strips common crate prefixes (`libazureinit::`, `azure_init::`)
/// so that span names are concise.
fn format_span_name(metadata: &tracing::Metadata<'_>) -> String {
    let target = metadata.target();
    let name = metadata.name();

    let module_path = target
        .strip_prefix("libazureinit::")
        .or_else(|| target.strip_prefix("azure_init::"))
        .unwrap_or(target);

    if module_path.is_empty() || module_path == target && target == name {
        name.to_string()
    } else {
        format!("{module_path}:{name}")
    }
}

/// Tracing subscriber layer that emits KVP diagnostic entries.
///
/// Translates span open/close and event occurrences into
/// `DiagnosticsKvp::emit()` calls. For events with a `health_report`
/// field, the report string is written directly to the store as a
/// `PROVISIONING_REPORT` entry.
pub struct TracingKvpLayer<S: KvpStore + 'static> {
    diagnostics: DiagnosticsKvp<S>,
}

impl<S: KvpStore + 'static> TracingKvpLayer<S> {
    pub fn new(diagnostics: DiagnosticsKvp<S>) -> Self {
        Self { diagnostics }
    }
}

impl<S, Sub> Layer<Sub> for TracingKvpLayer<S>
where
    S: KvpStore + 'static,
    Sub: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        ctx: TracingContext<'_, Sub>,
    ) {
        // Check for health_report events first -- these exist outside
        // of a span and are written directly as PROVISIONING_REPORT.
        let mut health_report = None;
        event.record(&mut |field: &tracing::field::Field,
                           value: &dyn fmt::Debug| {
            if field.name() == "health_report" {
                health_report =
                    Some(format!("{value:?}").trim_matches('"').to_string());
            }
        });

        if let Some(report_str) = health_report {
            if report_str.len() <= HV_KVP_AZURE_MAX_VALUE_SIZE {
                let _ = self
                    .diagnostics
                    .store()
                    .write("PROVISIONING_REPORT", &report_str);
            } else {
                for chunk in
                    report_str.as_bytes().chunks(HV_KVP_AZURE_MAX_VALUE_SIZE)
                {
                    let chunk_str = String::from_utf8_lossy(chunk);
                    let _ = self
                        .diagnostics
                        .store()
                        .write("PROVISIONING_REPORT", &chunk_str);
                }
            }
            return;
        }

        // All other events are inside a span.
        if let Some(span) = ctx.lookup_current() {
            let mut event_message = String::new();
            let mut visitor = StringVisitor {
                string: &mut event_message,
            };
            event.record(&mut visitor);

            let span_context = span.metadata();
            let span_id = Uuid::new_v4();

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

            let formatted_span_name = format_span_name(span_context);
            let diag_event = DiagnosticEvent {
                level: event.metadata().level().as_str().to_string(),
                name: formatted_span_name,
                span_id: span_id.to_string(),
                message: event_value,
                timestamp: Utc::now(),
            };

            let _ = self.diagnostics.emit(&diag_event);
        }
    }

    fn on_new_span(
        &self,
        _attrs: &Attributes<'_>,
        id: &Id,
        ctx: TracingContext<'_, Sub>,
    ) {
        let start_instant = MyInstant::now();
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(start_instant);
        }
    }

    fn on_close(&self, id: Id, ctx: TracingContext<Sub>) {
        if let Some(span) = ctx.span(&id) {
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

                let event_value =
                    format!("Start: {start_time_dt} | End: {end_time_dt}");

                let formatted_span_name = format_span_name(span_context);
                let diag_event = DiagnosticEvent {
                    level: span_context.level().as_str().to_string(),
                    name: formatted_span_name,
                    span_id: span_id.to_string(),
                    message: event_value,
                    timestamp: Utc::now(),
                };

                let _ = self.diagnostics.emit(&diag_event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HyperVKvpStore, InMemoryKvpStore, KvpStore};
    use tempfile::NamedTempFile;
    use tracing::{event, instrument, Level};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    fn make_test_layer() -> (InMemoryKvpStore, TracingKvpLayer<InMemoryKvpStore>)
    {
        let store = InMemoryKvpStore::default();
        let diagnostics =
            DiagnosticsKvp::new(store.clone(), "test-vm", "test-prefix");
        let layer = TracingKvpLayer::new(diagnostics);
        (store, layer)
    }

    #[test]
    fn test_on_event_writes_to_store() {
        let (store, layer) = make_test_layer();
        let subscriber = Registry::default().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let _span = tracing::info_span!("test_span").entered();
            event!(Level::INFO, msg = "hello from test");
        });

        let entries = store.entries().unwrap();
        assert!(
            !entries.is_empty(),
            "Expected at least one entry from the event + span close"
        );
    }

    #[test]
    fn test_health_report_writes_provisioning_report() {
        let (store, layer) = make_test_layer();
        let subscriber = Registry::default().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                health_report = "result=success|agent=test",
                "provisioning done"
            );
        });

        let value = store.read("PROVISIONING_REPORT").unwrap();
        assert_eq!(value, Some("result=success|agent=test".to_string()));
    }

    #[test]
    fn test_health_report_long_value_is_chunked() {
        let tmp = NamedTempFile::new().expect("create temp file");
        let store = HyperVKvpStore::new(tmp.path());
        let diagnostics = DiagnosticsKvp::new(store.clone(), "test-vm", "pfx");
        let layer = TracingKvpLayer::new(diagnostics);
        let subscriber = Registry::default().with(layer);

        let long_report = "X".repeat(HV_KVP_AZURE_MAX_VALUE_SIZE * 2 + 17);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(health_report = %long_report, "long report");
        });

        let entries = store.entries().expect("read entries");
        let report_entries: Vec<(String, String)> = entries
            .into_iter()
            .filter(|(k, _)| k == "PROVISIONING_REPORT")
            .collect();

        assert_eq!(report_entries.len(), 3);
        assert!(report_entries
            .iter()
            .all(|(_, v)| v.len() <= HV_KVP_AZURE_MAX_VALUE_SIZE));

        let reconstructed = report_entries
            .iter()
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(reconstructed, long_report);
    }

    #[test]
    fn test_span_close_emits_start_end() {
        let (store, layer) = make_test_layer();
        let subscriber = Registry::default().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let _span = tracing::info_span!("my_span").entered();
        });

        let entries = store.entries().unwrap();
        assert!(!entries.is_empty());

        let has_start_end = entries
            .iter()
            .any(|(_, v)| v.contains("Start:") && v.contains("End:"));
        assert!(
            has_start_end,
            "Expected a span close entry with Start/End timestamps"
        );
    }

    #[test]
    fn test_format_span_name_strips_prefix() {
        let expected = vec![
            (
                "libazureinit::provision::user",
                "create_user",
                "provision::user:create_user",
            ),
            ("azure_init::main", "run", "main:run"),
            ("my_crate", "my_func", "my_crate:my_func"),
        ];

        for (target, name, want) in expected {
            let module_path = target
                .strip_prefix("libazureinit::")
                .or_else(|| target.strip_prefix("azure_init::"))
                .unwrap_or(target);

            let result = if module_path.is_empty()
                || module_path == target && target == name
            {
                name.to_string()
            } else {
                format!("{module_path}:{name}")
            };

            assert_eq!(result, want, "target={target}, name={name}");
        }
    }

    #[test]
    fn test_instrumented_function_emits_entries() {
        let (store, layer) = make_test_layer();
        let subscriber = Registry::default().with(layer);

        #[instrument]
        fn do_work() {
            event!(Level::INFO, msg = "working");
        }

        tracing::subscriber::with_default(subscriber, || {
            do_work();
        });

        let entries = store.entries().unwrap();
        assert!(
            entries.len() >= 2,
            "Expected at least event + span close, got {}",
            entries.len()
        );
    }
}
