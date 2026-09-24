// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Synchronous `tracing`-to-KVP bridge.

use std::fmt;
use std::time::Instant;

use chrono::Utc;
use serde_json::{Map, Number, Value};
use tracing::{
    field::Visit,
    span::{Attributes, Id, Record},
    Event, Level, Metadata, Subscriber,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};
use uuid::Uuid;

use super::{
    Diagnostic, DiagnosticEvent, DiagnosticFinish, DiagnosticPayload,
    DiagnosticStart, DiagnosticWriter, Outcome,
};
use crate::KvpError;

const OUTCOME_FIELD: &str = "diagnostic.result";
const INVALID_OUTCOME: &str =
    "diagnostic.result must be a string containing success or fail";

/// Bridges `tracing` spans and events to a [`DiagnosticWriter`].
///
/// Requires the `tracing` feature. Callbacks write synchronously through
/// [`emit`](Self::emit), using the writer's validation, encoding and pool locks.
/// Storage and lock contention can block the calling thread indefinitely;
/// callback errors are reported to stderr.
///
/// The caller installs the subscriber, selects a [`Layer::with_filter`] filter,
/// and handles pool cleanup. No runtime or background worker is required.
/// Payloads are JSON text with `target`, `level` and `fields`. All recorded
/// fields are included; exclude secrets with `#[instrument(skip_all)]` or
/// explicit field selection.
///
/// A span's start and finish share a UUID; events, including unspanned events,
/// receive independent UUIDs. Finishes use `diagnostic.result` (`success` or
/// `fail`) when recorded, otherwise an observed ERROR implies failure and no
/// observed ERROR implies success. Unwinding implies failure regardless of an
/// override. Invalid explicit outcomes drop the event or finish. Filtering,
/// missing instrumentation and cancellation can hide failures.
///
/// # Example
/// ```no_run
/// use libazureinit_kvp::{
///     DiagnosticsKvp, DiagnosticWriter, KvpPool, KvpPoolStore, PoolMode,
/// };
/// use tracing_subscriber::{filter::LevelFilter, prelude::*};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let store = KvpPoolStore::new(KvpPool::Guest, PoolMode::Safe)?;
/// let writer = DiagnosticWriter::new(
///     store, "example/1", "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
/// )?;
/// let kvp = DiagnosticsKvp::new(writer);
/// let subscriber = tracing_subscriber::registry()
///     .with(kvp.clone().with_filter(LevelFilter::INFO));
/// tracing::subscriber::with_default(subscriber, || {
///     tracing::info_span!("configure").in_scope(|| {
///         tracing::info!(records = 3_u64, "configuration applied");
///     });
/// });
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct DiagnosticsKvp {
    writer: DiagnosticWriter,
}

impl DiagnosticsKvp {
    /// Creates a bridge that emits through the given writer.
    pub fn new(writer: DiagnosticWriter) -> Self {
        Self { writer }
    }

    /// Writes one diagnostic synchronously through the underlying writer.
    ///
    /// Used by callbacks and direct callers. An I/O error may leave a partial
    /// chunked payload in the pool.
    pub fn emit(&self, diagnostic: Diagnostic) -> Result<(), KvpError> {
        self.writer.emit(diagnostic)
    }

    /// Reports callback write errors to stderr without reentering tracing.
    fn deliver(&self, diagnostic: Diagnostic) {
        if let Err(error) = self.emit(diagnostic) {
            eprintln!("Failed to write KVP diagnostic: {error}");
        }
    }
}

/// Per-span state retained between a span's creation and its close.
struct SpanState {
    event_id: String,
    name: &'static str,
    started: Instant,
    fields: Fields,
    saw_error: bool,
}

impl<S> Layer<S> for DiagnosticsKvp
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(
        &self,
        attrs: &Attributes<'_>,
        id: &Id,
        ctx: Context<'_, S>,
    ) {
        let timestamp = Utc::now();
        let started = Instant::now();
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        let event_id = Uuid::new_v4().to_string();
        let name = attrs.metadata().name();
        let diagnostic = Diagnostic::Start(DiagnosticStart {
            key: self.writer.key_at(&event_id, name, None, timestamp),
            payload: fields.payload(attrs.metadata()),
        });
        span.extensions_mut().insert(SpanState {
            event_id,
            name,
            started,
            fields,
            saw_error: false,
        });
        self.deliver(diagnostic);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        if let Some(state) = extensions.get_mut::<SpanState>() {
            values.record(&mut state.fields);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let timestamp = Utc::now();
        let is_error = *event.metadata().level() == Level::ERROR;
        let enclosing = ctx.event_span(event);
        let name = match &enclosing {
            Some(span) => {
                if is_error {
                    if let Some(state) =
                        span.extensions_mut().get_mut::<SpanState>()
                    {
                        state.saw_error = true;
                    }
                }
                span.metadata().name()
            }
            None => event.metadata().name(),
        };
        let mut fields = Fields::default();
        event.record(&mut fields);
        let result = match fields.outcome() {
            Ok(result) => result,
            Err(error) => {
                eprintln!("Dropped KVP diagnostic event: {error}");
                return;
            }
        };
        self.deliver(Diagnostic::Event(DiagnosticEvent {
            key: self.writer.key_at(
                &Uuid::new_v4().to_string(),
                name,
                None,
                timestamp,
            ),
            payload: fields.payload(event.metadata()),
            result,
            duration: None,
        }));
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let timestamp = Utc::now();
        let closed_at = Instant::now();
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let Some(state) = span.extensions_mut().remove::<SpanState>() else {
            return;
        };
        let explicit = match state.fields.outcome() {
            Ok(result) => result,
            Err(error) => {
                eprintln!("Dropped KVP diagnostic finish: {error}");
                return;
            }
        };
        // A span closing during a panic reports failure even without an ERROR.
        let result = if std::thread::panicking() {
            Outcome::Failure
        } else {
            explicit.unwrap_or(if state.saw_error {
                Outcome::Failure
            } else {
                Outcome::Success
            })
        };
        self.deliver(Diagnostic::Finish(DiagnosticFinish {
            key: self.writer.key_at(
                &state.event_id,
                state.name,
                None,
                timestamp,
            ),
            payload: state.fields.payload(span.metadata()),
            result,
            duration: closed_at.saturating_duration_since(state.started),
        }));
    }
}

/// Structured tracing fields collected as JSON for a diagnostic payload.
#[derive(Clone, Default)]
struct Fields(Map<String, Value>);

impl Fields {
    fn insert(&mut self, field: &tracing::field::Field, value: Value) {
        self.0.insert(field.name().to_owned(), value);
    }

    fn outcome(&self) -> Result<Option<Outcome>, &'static str> {
        match self.0.get(OUTCOME_FIELD) {
            None => Ok(None),
            Some(Value::String(value)) if value == "success" => {
                Ok(Some(Outcome::Success))
            }
            Some(Value::String(value)) if value == "fail" => {
                Ok(Some(Outcome::Failure))
            }
            _ => Err(INVALID_OUTCOME),
        }
    }

    fn payload(&self, metadata: &Metadata<'_>) -> DiagnosticPayload {
        DiagnosticPayload::Text(
            serde_json::json!({
                "target": metadata.target(),
                "level": metadata.level().as_str(),
                "fields": self.0,
            })
            .to_string(),
        )
    }
}

impl Visit for Fields {
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.insert(field, Value::Bool(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.insert(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.insert(field, Value::from(value));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.insert(
            field,
            Number::from_f64(value)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(value.to_string())),
        );
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.insert(field, Value::String(value.to_owned()));
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.insert(field, Value::String(value.to_string()));
    }

    fn record_debug(
        &mut self,
        field: &tracing::field::Field,
        value: &dyn fmt::Debug,
    ) {
        self.insert(field, Value::String(format!("{value:?}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use tempfile::TempDir;
    use tracing_subscriber::prelude::*;

    use crate::{
        DiagnosticReader, Entry, Kind, KvpPool, KvpPoolStore, PoolMode,
    };

    const AGENT: &str = "test/1";
    const VM_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

    fn bridge(dir: &TempDir) -> (DiagnosticsKvp, KvpPoolStore) {
        let store =
            KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe)
                .unwrap();
        let writer =
            DiagnosticWriter::new(store.clone(), AGENT, VM_ID).unwrap();
        (DiagnosticsKvp::new(writer), store)
    }

    fn entries(store: &KvpPoolStore) -> Vec<Entry> {
        DiagnosticReader::new(store.clone()).entries().unwrap()
    }

    fn finish(entries: &[Entry]) -> &DiagnosticFinish {
        entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Diagnostic(Diagnostic::Finish(finish)) => Some(finish),
                _ => None,
            })
            .expect("a finish diagnostic")
    }

    #[test]
    fn span_emits_start_then_success_finish_with_nested_event() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("provision").in_scope(|| {
                tracing::info!(records = 3_u64, "applied");
            });
        });

        let entries = entries(&store);
        assert!(matches!(
            entries.as_slice(),
            [Entry::Diagnostic(Diagnostic::Start(start)),
             Entry::Diagnostic(Diagnostic::Event(event)),
             Entry::Diagnostic(Diagnostic::Finish(finish))]
                if start.key.name == "provision"
                    && finish.key.name == "provision"
                    && start.key.event_id == finish.key.event_id
                    && start.key.event_id != event.key.event_id
                    && finish.result == Outcome::Success
                    && event.key.name == "provision"
                    && matches!(&event.payload, DiagnosticPayload::Text(text)
                        if text.contains("\"records\":3")
                            && text.contains("applied")
                            && text.contains("\"level\":\"INFO\""))
        ));
    }

    #[test]
    fn spanless_event_uses_its_metadata_name() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("top level");
        });

        let entries = entries(&store);
        assert!(
            matches!(entries.as_slice(), [Entry::Diagnostic(Diagnostic::Event(event))]
                if event.key.name.starts_with("event ") && event.result.is_none())
        );
    }

    #[test]
    fn error_event_marks_span_finish_as_failure() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("provision").in_scope(|| {
                tracing::error!("boom");
            });
        });

        assert_eq!(finish(&entries(&store)).result, Outcome::Failure);
    }

    #[test]
    fn explicit_result_overrides_observed_error() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("op", diagnostic.result = "success");
            span.in_scope(|| tracing::error!("handled, not fatal"));
        });

        assert_eq!(finish(&entries(&store)).result, Outcome::Success);
    }

    #[test]
    fn invalid_explicit_result_drops_the_record() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(diagnostic.result = true, "not a valid outcome");
        });

        assert!(entries(&store).is_empty());
        assert!(!store.path().exists());
    }

    #[test]
    fn invalid_late_result_drops_only_the_finish() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("op", diagnostic.result = "success");
            span.record("diagnostic.result", "invalid");
        });
        assert!(matches!(
            entries(&store).as_slice(),
            [Entry::Diagnostic(Diagnostic::Start(_))]
        ));
    }

    #[test]
    fn typed_fields_and_explicit_failure_are_preserved() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        let error = std::io::Error::other("test I/O failure");
        let error: &(dyn std::error::Error + 'static) = &error;
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                name: "typed",
                finite = 1.25_f64,
                nan = f64::NAN,
                positive = f64::INFINITY,
                negative = f64::NEG_INFINITY,
                error,
                diagnostic.result = "fail",
            );
        });
        let expected = DiagnosticPayload::Text(
            serde_json::json!({
                "target": module_path!(),
                "level": "INFO",
                "fields": {
                    "finite": 1.25,
                    "nan": "NaN",
                    "positive": "inf",
                    "negative": "-inf",
                    "error": "test I/O failure",
                    "diagnostic.result": "fail",
                },
            })
            .to_string(),
        );
        let entries = entries(&store);
        assert!(
            matches!(entries.as_slice(), [Entry::Diagnostic(Diagnostic::Event(event))]
                if event.result == Some(Outcome::Failure) && event.payload == expected)
        );
    }

    #[test]
    fn callback_write_failure_does_not_panic_or_disable_later_events() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        std::fs::create_dir(store.path()).unwrap();
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("unwritable").in_scope(|| {
                tracing::info!("this write also fails");
            });
            assert!(store.path().is_dir());
            std::fs::remove_dir(store.path()).unwrap();
            tracing::info!(name: "recovered", "writes work again");
        });
        assert!(matches!(
            entries(&store).as_slice(),
            [Entry::Diagnostic(Diagnostic::Event(event))]
                if event.key.name == "recovered"
        ));
    }

    struct GuardProbe {
        kvp: DiagnosticsKvp,
        calls: Arc<AtomicUsize>,
    }

    impl Layer<tracing_subscriber::Registry> for GuardProbe {
        fn on_new_span(
            &self,
            attrs: &Attributes<'_>,
            id: &Id,
            ctx: Context<'_, tracing_subscriber::Registry>,
        ) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let missing = Id::from_u64(u64::MAX);
            self.kvp.on_new_span(attrs, &missing, ctx.clone());
            self.kvp.on_close(missing, ctx.clone());
            // This span belongs to the registry, but was never observed by KVP.
            self.kvp.on_close(id.clone(), ctx);
        }

        fn on_record(
            &self,
            id: &Id,
            values: &Record<'_>,
            ctx: Context<'_, tracing_subscriber::Registry>,
        ) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.kvp
                .on_record(&Id::from_u64(u64::MAX), values, ctx.clone());
            self.kvp.on_record(id, values, ctx);
        }
    }

    #[test]
    fn callbacks_ignore_missing_spans_and_missing_kvp_state() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let calls = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry().with(GuardProbe {
            kvp,
            calls: Arc::clone(&calls),
        });
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(
                "unobserved",
                updated = tracing::field::Empty
            );
            span.record("updated", true);
        });
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(!store.path().exists());
    }

    #[test]
    fn late_recorded_fields_appear_in_finish() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(
                "request",
                http_status = tracing::field::Empty
            );
            span.in_scope(|| span.record("http_status", 200));
        });

        let entries = entries(&store);
        assert!(
            matches!(&finish(&entries).payload, DiagnosticPayload::Text(text)
            if text.contains("\"http_status\":200"))
        );
    }

    #[test]
    fn span_closing_while_unwinding_finishes_as_failure() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            let unwound =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _entered = tracing::info_span!("boom").entered();
                    panic!("provisioning exploded");
                }));
            assert!(unwound.is_err());
        });

        assert_eq!(finish(&entries(&store)).result, Outcome::Failure);
    }

    #[test]
    fn concurrent_spans_are_all_written() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let dispatch =
            tracing::Dispatch::new(tracing_subscriber::registry().with(kvp));
        let workers = 8;
        let ready = Arc::new(Barrier::new(workers));
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                let dispatch = dispatch.clone();
                let ready = Arc::clone(&ready);
                thread::spawn(move || {
                    tracing::dispatcher::with_default(&dispatch, || {
                        ready.wait();
                        tracing::info_span!("op", worker)
                            .in_scope(|| tracing::info!("work"));
                    });
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let entries = entries(&store);
        let count = |want: Kind| {
            entries
                .iter()
                .filter(|entry| {
                    matches!(entry, Entry::Diagnostic(diagnostic)
                        if diagnostic.kind() == want)
                })
                .count()
        };
        assert_eq!(count(Kind::Start), workers);
        assert_eq!(count(Kind::Finish), workers);
        assert_eq!(count(Kind::Event), workers);
    }
}
