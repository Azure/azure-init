// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Azure Init's private, synchronous `tracing`-to-KVP adapter.

use std::fmt;
use std::time::Instant;

use libazureinit_kvp::{
    DiagnosticPayload, DiagnosticWriter, KvpError, Outcome,
};
use serde_json::{Map, Number, Value};
use tracing::{
    field::Visit,
    span::{Attributes, Id, Record},
    Event, Level, Metadata, Subscriber,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};
use uuid::Uuid;

const OUTCOME_FIELD: &str = "diagnostic.result";
const INVALID_OUTCOME: &str =
    "diagnostic.result must be a string containing success or fail";

/// Bridges `tracing` spans and events to a [`DiagnosticWriter`].
///
/// Callbacks use the public writer methods for validation, encoding and locking.
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
#[derive(Clone, Debug)]
pub(super) struct DiagnosticsKvp {
    writer: DiagnosticWriter,
}

impl DiagnosticsKvp {
    pub(super) fn new(writer: DiagnosticWriter) -> Self {
        Self { writer }
    }
}

/// Reports callback write errors to stderr without reentering tracing.
fn report_write_error(result: Result<(), KvpError>) {
    if let Err(error) = result {
        eprintln!("Failed to write KVP diagnostic: {error}");
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
        let started = Instant::now();
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        let event_id = Uuid::new_v4().to_string();
        let name = attrs.metadata().name();
        let payload = fields.payload(attrs.metadata());
        span.extensions_mut().insert(SpanState {
            event_id: event_id.clone(),
            name,
            started,
            fields,
            saw_error: false,
        });
        report_write_error(
            self.writer.emit_start(&event_id, name, payload, None),
        );
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
        report_write_error(self.writer.emit_event(
            name,
            fields.payload(event.metadata()),
            None,
            result,
            None,
        ));
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let closed_at = Instant::now();
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let Some(state) = span.extensions_mut().remove::<SpanState>() else {
            return;
        };
        // If this thread is already unwinding from a panic, record failure
        // regardless of the explicit outcome.
        let result = if std::thread::panicking() {
            Outcome::Fail
        } else {
            let explicit = match state.fields.outcome() {
                Ok(result) => result,
                Err(error) => {
                    eprintln!("Dropped KVP diagnostic finish: {error}");
                    return;
                }
            };
            explicit.unwrap_or(if state.saw_error {
                Outcome::Fail
            } else {
                Outcome::Success
            })
        };
        report_write_error(self.writer.emit_finish(
            &state.event_id,
            state.name,
            state.fields.payload(span.metadata()),
            None,
            result,
            closed_at.saturating_duration_since(state.started),
        ));
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
            Some(Value::String(value))
                if value == Outcome::Success.as_str() =>
            {
                Ok(Some(Outcome::Success))
            }
            Some(Value::String(value)) if value == Outcome::Fail.as_str() => {
                Ok(Some(Outcome::Fail))
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

    use libazureinit_kvp::{
        Diagnostic, DiagnosticFinish, DiagnosticReader, Entry, Kind, KvpPool,
        KvpPoolStore, PoolMode,
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
    fn span_preserves_writer_timestamps_and_measured_duration() {
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let started = Instant::now();
        let work = Duration::from_millis(2);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("timed").in_scope(|| thread::sleep(work));
        });
        let elapsed = started.elapsed();
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let entries = entries(&store);
        let [Entry::Diagnostic(Diagnostic::Start(start)), Entry::Diagnostic(Diagnostic::Finish(finish))] =
            entries.as_slice()
        else {
            panic!("expected start and finish, got {entries:?}");
        };
        for key in [&start.key, &finish.key] {
            let timestamp =
                u128::try_from(key.timestamp.timestamp_millis()).unwrap();
            assert!((before..=after).contains(&timestamp));
            assert_eq!(key.timestamp.timestamp_subsec_nanos() % 1_000_000, 0);
        }
        assert_eq!(start.key.event_id, finish.key.event_id);
        assert!(finish.duration >= work);
        assert!(finish.duration <= elapsed);
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

        assert_eq!(finish(&entries(&store)).result, Outcome::Fail);
    }

    #[test]
    fn explicit_result_overrides_observed_error() {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(
                "op",
                diagnostic.result = Outcome::Fail.as_str()
            );
            span.in_scope(|| {
                tracing::error!("handled, not fatal");
                span.record(OUTCOME_FIELD, Outcome::Success.as_str());
            });
        });

        assert_eq!(finish(&entries(&store)).result, Outcome::Success);
    }

    #[test]
    fn outcome_field_accepts_only_canonical_wire_tokens() {
        assert_eq!(Fields::default().outcome(), Ok(None));
        for (token, outcome) in
            [("success", Outcome::Success), ("fail", Outcome::Fail)]
        {
            let fields = Fields(Map::from_iter([(
                OUTCOME_FIELD.to_owned(),
                Value::String(token.to_owned()),
            )]));
            assert_eq!(fields.outcome(), Ok(Some(outcome)));
        }
        for value in [
            Value::String("failure".to_owned()),
            Value::String("Success".to_owned()),
            Value::String("".to_owned()),
            Value::Bool(true),
            Value::from(1),
            Value::Null,
        ] {
            let fields =
                Fields(Map::from_iter([(OUTCOME_FIELD.to_owned(), value)]));
            assert_eq!(fields.outcome(), Err(INVALID_OUTCOME));
        }
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

    #[rstest::rstest]
    #[case::invalid_text(&"invalid")]
    #[case::invalid_type(&true)]
    fn invalid_late_result_drops_only_the_finish(
        #[case] result: &dyn tracing::Value,
    ) {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("op", diagnostic.result = "success");
            span.record(OUTCOME_FIELD, result);
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
                if event.result == Some(Outcome::Fail) && event.payload == expected)
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

    #[rstest::rstest]
    #[case::absent(None)]
    #[case::success(Some::<&dyn tracing::Value>(&"success"))]
    #[case::fail(Some::<&dyn tracing::Value>(&"fail"))]
    #[case::invalid_text(Some::<&dyn tracing::Value>(&"invalid"))]
    #[case::invalid_type(Some::<&dyn tracing::Value>(&true))]
    fn span_closing_while_unwinding_finishes_as_failure(
        #[case] result: Option<&dyn tracing::Value>,
    ) {
        let dir = TempDir::new().unwrap();
        let (kvp, store) = bridge(&dir);
        let subscriber = tracing_subscriber::registry().with(kvp);
        tracing::subscriber::with_default(subscriber, || {
            let unwound =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let span = tracing::info_span!(
                        "boom",
                        diagnostic.result = tracing::field::Empty,
                    );
                    if let Some(result) = result {
                        span.record(OUTCOME_FIELD, result);
                    }
                    let _entered = span.entered();
                    panic!("provisioning exploded");
                }));
            assert!(unwound.is_err());
        });

        let entries = entries(&store);
        let [Entry::Diagnostic(Diagnostic::Start(start)), Entry::Diagnostic(Diagnostic::Finish(finish))] =
            entries.as_slice()
        else {
            panic!(
                "expected start and finish during unwinding, got {entries:?}"
            );
        };
        assert_eq!(start.key.event_id, finish.key.event_id);
        assert_eq!(finish.result, Outcome::Fail);
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
