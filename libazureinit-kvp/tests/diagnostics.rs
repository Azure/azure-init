// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the [`DiagnosticsKvp`] layer: emit/read
//! round-trips, chunk reassembly, classification, scoped clearing, and
//! the concurrent-write atomicity guarantee that keeps chunked events
//! from interleaving.

use std::thread;

use libazureinit_kvp::{
    DiagnosticEvent, DiagnosticRecord, DiagnosticsKvp, KvpPool, KvpPoolStore,
    PoolMode, MAX_CHUNK_BYTES,
};
use tempfile::TempDir;
use tracing::Level;

const PREFIX: &str = "azure-init-test";
const VM_ID: &str = "vm-abc";

fn diagnostics(dir: &TempDir) -> DiagnosticsKvp {
    let store =
        KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe)
            .unwrap();
    DiagnosticsKvp::new(store, VM_ID, PREFIX)
}

#[test]
fn short_event_round_trips_as_single_record() {
    let dir = TempDir::new().unwrap();
    let diag = diagnostics(&dir);

    assert_eq!(diag.vm_id(), VM_ID);
    assert_eq!(diag.event_prefix(), PREFIX);

    let event =
        DiagnosticEvent::new(Level::INFO, "user:create_user", "created");
    diag.emit(&event).unwrap();

    assert_eq!(diag.store().dump().unwrap().len(), 1);

    let records = diag.records().unwrap();
    assert_eq!(records.len(), 1);
    match &records[0] {
        DiagnosticRecord::Event {
            event: decoded,
            chunks,
        } => {
            assert_eq!(*chunks, 1);
            assert_eq!(decoded.level, Level::INFO);
            assert_eq!(decoded.name, "user:create_user");
            assert_eq!(decoded.event_id, event.event_id);
            assert_eq!(decoded.message, "created");
        }
        other => panic!("expected event, got {other:?}"),
    }
}

#[test]
fn long_event_splits_across_records_and_reassembles() {
    let dir = TempDir::new().unwrap();
    let diag = diagnostics(&dir);

    let message = "x".repeat(MAX_CHUNK_BYTES * 3 + 50);
    let event = DiagnosticEvent::new(Level::DEBUG, "config:dump", &message);
    diag.emit(&event).unwrap();

    // Split across four records that all share one key (split keys).
    let dumped = diag.store().dump().unwrap();
    assert_eq!(dumped.len(), 4);
    let key = &dumped[0].0;
    assert!(dumped.iter().all(|(k, _)| k == key));

    let records = diag.records().unwrap();
    assert_eq!(records.len(), 1);
    match &records[0] {
        DiagnosticRecord::Event {
            event: decoded,
            chunks,
        } => {
            assert_eq!(*chunks, 4);
            assert_eq!(decoded.message, message);
        }
        other => panic!("expected event, got {other:?}"),
    }
}

#[test]
fn injected_malformed_key_is_classified() {
    let dir = TempDir::new().unwrap();
    let diag = diagnostics(&dir);

    // Five segments but an unrecognized level.
    diag.store()
        .append(&format!("{PREFIX}|{VM_ID}|NOPE|bad:level|id"), "junk")
        .unwrap();

    let records = diag.records().unwrap();
    assert_eq!(records.len(), 1);
    assert!(matches!(
        &records[0],
        DiagnosticRecord::Malformed { reason, .. } if reason.contains("NOPE")
    ));
}

#[test]
fn mixed_records_round_trip_together() {
    let dir = TempDir::new().unwrap();
    let diag = diagnostics(&dir);

    diag.emit(&DiagnosticEvent::new(Level::INFO, "a:b", "short"))
        .unwrap();
    diag.emit(&DiagnosticEvent::new(
        Level::WARN,
        "c:d",
        "y".repeat(MAX_CHUNK_BYTES + 5),
    ))
    .unwrap();
    diag.store()
        .append("PROVISIONING_REPORT", "result=success")
        .unwrap();
    diag.store()
        .append(&format!("{PREFIX}|{VM_ID}|NOPE|e:f|id"), "junk")
        .unwrap();

    let records = diag.records().unwrap();
    // Two events + one raw + one malformed.
    assert_eq!(records.len(), 4);
    assert_eq!(diag.events().unwrap().len(), 2);
}

#[test]
fn clear_removes_events_but_keeps_raw() {
    let dir = TempDir::new().unwrap();
    let diag = diagnostics(&dir);

    diag.emit(&DiagnosticEvent::new(Level::INFO, "a:b", "e1"))
        .unwrap();
    diag.emit(&DiagnosticEvent::new(
        Level::DEBUG,
        "c:d",
        "z".repeat(MAX_CHUNK_BYTES * 2),
    ))
    .unwrap();
    diag.store()
        .append("PROVISIONING_REPORT", "result=success")
        .unwrap();

    diag.clear().unwrap();

    let records = diag.records().unwrap();
    assert_eq!(records.len(), 1);
    assert!(matches!(
        &records[0],
        DiagnosticRecord::Raw { key, .. } if key == "PROVISIONING_REPORT"
    ));
    assert!(diag.events().unwrap().is_empty());
}

#[test]
fn clear_is_scoped_to_matching_prefix_and_vm_id() {
    let dir = TempDir::new().unwrap();
    let diag = diagnostics(&dir);

    diag.emit(&DiagnosticEvent::new(Level::INFO, "a:b", "mine"))
        .unwrap();
    // An event from a different agent/VM must survive clear().
    diag.store()
        .append("other-agent|other-vm|INFO|x:y|id", "theirs")
        .unwrap();

    diag.clear().unwrap();

    let events = diag.events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "theirs");
}

#[test]
fn emit_rejects_delimiter_in_event_fields() {
    let dir = TempDir::new().unwrap();
    let diag = diagnostics(&dir);

    // A pipe in the name would produce an ambiguous six-segment key.
    let event = DiagnosticEvent::new(Level::INFO, "a|b", "msg");
    assert!(diag.emit(&event).is_err());
    // Nothing was written.
    assert!(diag.store().dump().unwrap().is_empty());
}

#[test]
fn concurrent_multichunk_emits_reassemble_without_interleaving() {
    let dir = TempDir::new().unwrap();
    let diag = diagnostics(&dir);

    const THREADS: usize = 5;
    const PER_THREAD: usize = 8;
    // Force three chunks per event.
    let len = MAX_CHUNK_BYTES * 2 + 7;

    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let diag = diag.clone();
            let marker = (b'a' + t as u8) as char;
            thread::spawn(move || {
                for _ in 0..PER_THREAD {
                    let message = marker.to_string().repeat(len);
                    let event = DiagnosticEvent::new(
                        Level::INFO,
                        format!("thread:{marker}"),
                        message,
                    );
                    diag.emit(&event).unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let events = diag.events().unwrap();
    // If any event's chunks had been split by an interleaving writer, the
    // key would appear as multiple groups and the count would be wrong.
    assert_eq!(events.len(), THREADS * PER_THREAD);
    for event in &events {
        // Each message is homogeneous and full length: chunks stayed
        // contiguous on disk.
        assert_eq!(event.message.len(), len);
        let first = event.message.chars().next().unwrap();
        assert!(event.message.chars().all(|c| c == first));
        assert_eq!(event.name, format!("thread:{first}"));
    }
}
