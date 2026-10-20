// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public diagnostics API round trips, cloud-init compatibility, and store
//! behavior.

use std::io::Read;
use std::thread;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::read::ZlibDecoder;
use libazureinit_kvp::{
    write_report, DecodeError, Diagnostic, DiagnosticPayload, DiagnosticReader,
    DiagnosticWriter, Encoding, Entry, Kind, KvpError, KvpPool, KvpPoolStore,
    Outcome, PoolMode, ProvisioningReport, RawKeyValue, ReportPpsType,
    DIAGNOSTIC_VERSION_ID, MAX_CHUNK_BYTES,
};
use rstest::rstest;
use tempfile::TempDir;

#[path = "fixtures/cloud_init.rs"]
mod cloud_init_fixtures;
use cloud_init_fixtures::COMPRESSED_LOG_CHUNKS;

const AGENT: &str = "azure-init-test";
const VM_ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
const EVENT_ID: &str = "8f3e9c4a-1b2c-4d5e-9f01-234567890abc";

fn store_at(dir: &TempDir) -> KvpPoolStore {
    KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe).unwrap()
}

fn diagnostic(entry: &Entry) -> &Diagnostic {
    let Entry::Diagnostic(diagnostic) = entry else {
        panic!("expected a diagnostic, got {entry:?}");
    };
    diagnostic
}

/// Real cloud-init reporting entries captured from a guest pool 1 file.
/// Each tuple is one record's `(key, JSON value)`.
const CLOUD_INIT_RECORDS: &[(&str, &str)] = &[
    (
        "CLOUD_INIT|1785187982|finish|modules-final/config-scripts_user|0e5e179d-5341-478b-8456-fbb90621bdf8|e5f01809-a7a3-4279-aa64-1f18e21eda6e",
        r#"{"name":"modules-final/config-scripts_user","type":"finish","ts":"2026-07-27T21:33:24.339006+00:00","result":"SUCCESS","duration":0.0006448590000012189,"msg":"config-scripts_user ran successfully and took 0.001 seconds"}"#,
    ),
    (
        "CLOUD_INIT|1785187982|start|modules-final/config-ssh_authkey_fingerprints|0e5e179d-5341-478b-8456-fbb90621bdf8|c4d4a08d-fe93-4c7a-9be6-9a38c212e212",
        r#"{"name":"modules-final/config-ssh_authkey_fingerprints","type":"start","ts":"2026-07-27T21:33:24.339170+00:00","msg":"running config-ssh_authkey_fingerprints with frequency once-per-instance"}"#,
    ),
    (
        "CLOUD_INIT|1785187982|finish|modules-final|0e5e179d-5341-478b-8456-fbb90621bdf8|126f969f-13fd-4b4b-a136-b7114518491f",
        r#"{"name":"modules-final","type":"finish","ts":"2026-07-27T21:33:24.431885+00:00","result":"SUCCESS","duration":0.340712044,"msg":"running modules for final"}"#,
    ),
];

#[rstest]
#[case::legacy(false)]
#[case::current(true)]
fn reads_real_cloud_init_pool_in_both_layouts(#[case] include_vm_id: bool) {
    let dir = TempDir::new().unwrap();
    let store = store_at(&dir);
    store
        .append_multiple(CLOUD_INIT_RECORDS.iter().map(|&(key, value)| {
            let key = if include_vm_id {
                key.to_owned()
            } else {
                without_vm_id(key)
            };
            (key, value)
        }))
        .unwrap();

    let entries = DiagnosticReader::new(store).entries().unwrap();
    assert_eq!(entries.len(), CLOUD_INIT_RECORDS.len());

    let Diagnostic::Finish(finish) = diagnostic(&entries[0]) else {
        panic!("expected a finish");
    };
    assert_eq!(finish.key.agent, "CLOUD_INIT");
    assert_eq!(finish.key.name, "modules-final/config-scripts_user");
    assert_eq!(
        finish.key.vm_id.as_deref(),
        include_vm_id.then_some(CLOUD_INIT_VM_ID)
    );
    assert_eq!(finish.result, Outcome::Success);
    assert_eq!(finish.duration, Duration::from_micros(645));
    assert_eq!(
        finish.payload,
        DiagnosticPayload::from(
            "config-scripts_user ran successfully and took 0.001 seconds"
        )
    );
    assert!(matches!(diagnostic(&entries[1]), Diagnostic::Start(_)));
    assert!(matches!(diagnostic(&entries[2]), Diagnostic::Finish(finish)
        if finish.result == Outcome::Success
            && finish.duration == Duration::from_micros(340712)));
}

#[test]
fn span_and_point_events_round_trip_with_a_report() {
    let dir = TempDir::new().unwrap();
    let store = store_at(&dir);
    let writer = DiagnosticWriter::new(store.clone(), AGENT, VM_ID).unwrap();
    let reader = DiagnosticReader::new(store.clone());
    assert!(!store.path().exists());
    assert!(reader.entries().unwrap().is_empty());

    writer
        .emit_start(EVENT_ID, "provision:run", "starting", None)
        .unwrap();
    writer
        .emit_event(
            "imds",
            "ok",
            None,
            Some(Outcome::Success),
            Some(Duration::from_micros(17)),
        )
        .unwrap();
    writer
        .emit_finish(
            EVENT_ID,
            "provision:run",
            "finished",
            None,
            Outcome::Success,
            Duration::from_micros(120),
        )
        .unwrap();
    let report = ProvisioningReport::success(AGENT, VM_ID, ReportPpsType::None)
        .with_extra("build", "test-123");
    write_report(&store, &report).unwrap();

    let entries = reader.entries().unwrap();
    let [Entry::Diagnostic(Diagnostic::Start(start)), Entry::Diagnostic(Diagnostic::Event(event)), Entry::Diagnostic(Diagnostic::Finish(finish)), Entry::Report(decoded_report)] =
        entries.as_slice()
    else {
        panic!("unexpected entries: {entries:?}");
    };
    assert_eq!(start.key.event_id, EVENT_ID);
    assert_eq!(finish.key.event_id, EVENT_ID);
    assert_eq!(start.key.name, finish.key.name);
    assert_eq!(start.key.agent, AGENT);
    assert_eq!(start.key.vm_id.as_deref(), Some(VM_ID));
    assert_eq!(start.payload, DiagnosticPayload::from("starting"));
    assert_eq!(event.key.name, "imds");
    assert_eq!(event.payload, DiagnosticPayload::from("ok"));
    assert_eq!(event.result, Some(Outcome::Success));
    assert_eq!(event.duration, Some(Duration::from_micros(17)));
    assert_eq!(
        uuid::Uuid::parse_str(&event.key.event_id)
            .unwrap()
            .get_version_num(),
        4
    );
    assert_ne!(event.key.event_id, EVENT_ID);
    assert_eq!(finish.payload, DiagnosticPayload::from("finished"));
    assert_eq!(finish.result, Outcome::Success);
    assert_eq!(finish.duration, Duration::from_micros(120));
    assert_eq!(decoded_report, &report);

    let dumped = store.dump().unwrap();
    for (key, _) in &dumped[..3] {
        assert!(key
            .starts_with(&format!("{DIAGNOSTIC_VERSION_ID}|{AGENT}|{VM_ID}|")));
        assert!(key.ends_with("|0"));
    }
}

#[rstest]
#[case::text(None)]
#[case::gzip(Some(Encoding::GzB64))]
#[case::zlib(Some(Encoding::ZlibB64))]
fn long_payload_round_trips_through_host_visible_records(
    #[case] encoding: Option<Encoding>,
) {
    let dir = TempDir::new().unwrap();
    let store = store_at(&dir);
    let writer = DiagnosticWriter::new(store.clone(), AGENT, VM_ID).unwrap();
    let message: String = (0..MAX_CHUNK_BYTES)
        .map(|index| format!("{index:04x}\u{20ac};"))
        .collect();
    writer
        .emit_event(
            "config:dump",
            message.as_str(),
            encoding.clone(),
            None,
            None,
        )
        .unwrap();

    let dumped = store.dump().unwrap();
    assert!(dumped.len() > 1);
    assert_eq!(store.entries().unwrap().len(), dumped.len());
    let base = dumped[0].0.rsplit_once('|').unwrap().0;
    for (index, (key, value)) in dumped.iter().enumerate() {
        assert_eq!(key, &format!("{base}|{index}"));
        assert!(key.len() <= 254);
        assert!(value.len() <= MAX_CHUNK_BYTES);
    }

    let decoded =
        decode_single(DiagnosticReader::new(store).entries().unwrap());
    assert_eq!(decoded.kind(), Kind::Event);
    assert_eq!(decoded.key().encoding, encoding);
    let expected = if encoding.is_some() {
        DiagnosticPayload::Bytes(message.into_bytes())
    } else {
        DiagnosticPayload::Text(message)
    };
    assert_eq!(decoded.payload(), &expected);
}

#[test]
fn raw_and_malformed_records_are_preserved_beside_diagnostics() {
    let dir = TempDir::new().unwrap();
    let store = store_at(&dir);
    let records = vec![
        (format!("{AGENT}|100|{VM_ID}|event|legacy|{EVENT_ID}|2026-08-31T00:00:00Z|0"), "legacy", None),
        ("DIAG|bad".into(), "junk", Some(DecodeError::Malformed)),
        (format!("CLOUD_INIT|100|event|broken|{EVENT_ID}"), "not-json", Some(DecodeError::Malformed)),
        ("PROVISIONING_REPORT".into(), "result=success", Some(DecodeError::Malformed)),
        ("DIAG_V2|future".into(), "unknown", Some(DecodeError::UnsupportedVersion)),
    ];
    store
        .append_multiple(records.iter().map(|(key, value, _)| (key, *value)))
        .unwrap();
    DiagnosticWriter::new(store.clone(), AGENT, VM_ID)
        .unwrap()
        .emit_event("valid", "visible", None, None, None)
        .unwrap();
    let before = std::fs::read(store.path()).unwrap();

    let entries = DiagnosticReader::new(store.clone()).entries().unwrap();
    assert_eq!(entries.len(), records.len() + 1);
    for (entry, (key, value, error)) in entries.iter().zip(&records) {
        assert_eq!(
            entry,
            &Entry::Raw(RawKeyValue {
                key: key.clone(),
                value: (*value).to_owned(),
                error: *error,
            })
        );
    }
    assert_eq!(
        diagnostic(entries.last().unwrap()).payload(),
        &DiagnosticPayload::from("visible")
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}

#[test]
fn chunked_entries_survive_store_swap_deletion() {
    let dir = TempDir::new().unwrap();
    let store = store_at(&dir);
    let writer = DiagnosticWriter::new(store.clone(), AGENT, VM_ID).unwrap();
    let first_message = "a".repeat(MAX_CHUNK_BYTES * 2 + 7);
    let second_message = "b".repeat(MAX_CHUNK_BYTES * 2 + 7);

    store.append("remove-me", "raw").unwrap();
    writer
        .emit_event("first", first_message.as_str(), None, None, None)
        .unwrap();
    writer
        .emit_event("second", second_message.as_str(), None, None, None)
        .unwrap();

    assert!(store.delete("remove-me").unwrap());

    let entries = DiagnosticReader::new(store).entries().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(diagnostic(&entries[0]).key().name, "second");
    assert_eq!(
        diagnostic(&entries[0]).payload(),
        &DiagnosticPayload::Text(second_message)
    );
    assert_eq!(diagnostic(&entries[1]).key().name, "first");
    assert_eq!(
        diagnostic(&entries[1]).payload(),
        &DiagnosticPayload::Text(first_message)
    );
}

#[test]
fn emit_rejects_delimiter_in_event_fields() {
    let dir = TempDir::new().unwrap();
    let store = store_at(&dir);
    let writer = DiagnosticWriter::new(store.clone(), AGENT, VM_ID).unwrap();

    assert!(matches!(
        writer.emit_event("a|b", "msg", None, None, None),
        Err(KvpError::EventFieldContainsDelimiter { field: "name" })
    ));
    assert!(!store.path().exists());
}

#[test]
fn concurrent_multichunk_emits_reassemble_without_interleaving() {
    let dir = TempDir::new().unwrap();
    let store = store_at(&dir);
    let writer = DiagnosticWriter::new(store.clone(), AGENT, VM_ID).unwrap();

    const THREADS: usize = 5;
    const PER_THREAD: usize = 8;
    let len = MAX_CHUNK_BYTES * 2 + 7;

    let handles: Vec<_> = (0..THREADS)
        .map(|thread_index| {
            let writer = writer.clone();
            let marker = (b'a' + u8::try_from(thread_index).unwrap()) as char;
            thread::spawn(move || {
                for _ in 0..PER_THREAD {
                    let message = marker.to_string().repeat(len);
                    writer
                        .emit_event(
                            &format!("thread:{marker}"),
                            message,
                            None,
                            None,
                            None,
                        )
                        .unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let dumped = store.dump().unwrap();
    assert_eq!(dumped.len(), THREADS * PER_THREAD * 3);
    for group in dumped.chunks_exact(3) {
        let base = group[0].0.rsplit_once('|').unwrap().0;
        for (index, (key, _)) in group.iter().enumerate() {
            assert_eq!(key, &format!("{base}|{index}"));
        }
    }

    let events = DiagnosticReader::new(store).entries().unwrap();
    assert_eq!(events.len(), THREADS * PER_THREAD);
    for entry in &events {
        let event = diagnostic(entry);
        let DiagnosticPayload::Text(message) = event.payload() else {
            panic!("expected text payload");
        };
        assert_eq!(message.len(), len);
        let first = message.chars().next().unwrap();
        assert!(message.chars().all(|character| character == first));
        assert_eq!(event.key().name, format!("thread:{first}"));
    }
}

/// The reassembled `msg` across all three chunks.
const EXPECTED_COMPRESSED_MSG: &str = "{\"encoding\": \"gz+b64\", \"data\": \"eJxtV0uObDUMnbOKt4GHYjuJbYYIxAoYIQZx7EhIDBjAgN1ziumz1K2uqtY9lWuf3+XB++uwr2N/\\n4fEDfsb48vXLP3//8ef3f/37208///jrL7//8OVq2K09s4zekFEn71Lyp2fiBZsU1wxZ5zv+BpAa\\nwIzY128A1jhGRMikPJSsPJym6x5ih681gNwA6jzvnvK4U+6dhTdqu/Z4OsaY0y9xutDOkeIZDaw0\\nsKTytj1N9x3zVFjMoKnFtGaZNDCzganSNHnj8Hg4Am1eK4fcY2Nn7djuLuvJ5Iq5zVP2PPdWA78a\\n+CcyRXBfig14WKnskaNU6UWe1cDsBiZ0LamKNLIJIPyJVDl3nrHyzdh7lvt42F4DqQ3kfPXY2e9h\\nMdUyfhqYQDCJvsJoa4VY3KG1j5F3E7UGeC0GE9Xm5p0X5EzeRxYdnG+BCrV0F7Ht7qTeANrZ59Sk\\new4YjeU6mIjxSQNAnUZoGU1i2eDc3Ya71KQpo7m8UwTRfc/3oUFKk4c9cPbMoho6nhn+iQ8r1iQn\\nrdfAdrrAeeja0gxPp9w3d0FwV+LiVy4YVwMUH9hTp17qVOGQpwHZDs60VwTfqINRsY17Q2l9pmcR\\nj+4+qztrp5LYy/KCfmfbXbX4rlBfonb1NEZArRaW1+XDdtRTL0Nm+9LQLaI15t2Yx/tY18WJa3oD\\n22mDRtLaV1QGtHAfdvCSyavRKHVKyLev7u34WrOSISCLQQRnPgJ0sBskTw7Ts+hutlOBqN85Rplz\\nXuWwj6iyaAhjjIL7P5qqfM9rhEWdDl48ObTXmF44zUznfd+kby/nTgVF70HnA0sQ0h1SL/zZilP1\\nsAm4C6UW7ASEz7iBEzbQnUL8DD83hUtijQtvWjYOAogZfCGPdZMbtXKni0Ovzj2g/Ra4G3zZDVx5\\niKDbQHQqsHvXgQWB4ONOz21IHWKu3QB0fL8Yx8sNLlgG3Wv78IvN1xdpo0Xu+L6PrwejqVFX59R1\\nPgu86zGvSyDdXgQhra3I8Fmy2btddpwvQRbV2dc0RMaathBMrJCmyHIoynHqkQzv9VUb6x7Yazbw\\nnSIO3HFlbnWOElSMkncWwntin5KO1C5enuCOn6PdVDtN6COky/gkbjgh6su2R/qD/zFfHU0ccqcE\\nZKqmvPWK4Re+IovPxHi9tic+hU0DMO6zSfSpH9zcurRZYR+/NOS9ZwVxIRWLHzoFLB/rM4w469Nn\\nZoxN6ZMzpzdZIp1SRsbbG3Vj7YPocHpzudkUxxjxNQZCbIMxLKfAC1AuF0ljZdJpR8ZF3RvTEh0r\\nWN8q0Tu6y9tOlSOU5crBpfYxKA6y4ubyTjV8PsG639xy4Smw0oKp66B4oUdjolrFJ48uBFGjsVLp\\ndHQwaJ+JsISJokGIvIc6YBAkyLMPPIsnVo6CBKYf7QxVOhU5PDqPwe8hbFsuA2VSRx4nytmAtFqB\\n3SEDJbFIWM1aMI6gj82CNzJmV2ilU8dZajljvmFgSJrivtEeAaAo4rhlR/k5DLo1gK1KtvO8+TJj\\nfnxHH1wRwvj28tkp4RmthXGj7zAhqdCpBc8Ad+OMoIgQyTiINgGRdeNkzT5np4GHkNCZaFDCmDOC\\ntpYzTKFggG6JlhHPPiUykVQNaMf86yAGcgjJUQPlZI3/4xGPGPdsjsazZ6eAWHiUmugR8JI9HKx6\\n6NkPhfPDPopEZ1eFMTZbnZ0m8IQDlaPl/S8IG4GF8MJqcOD/AFeindw=\\n\"}";

const CLOUD_INIT_VM_ID: &str = "0e5e179d-5341-478b-8456-fbb90621bdf8";

/// Insert a `vm_id` segment after `name`, turning an old-format key into
/// the current layout (any trailing chunk index is preserved):
/// `CLOUD_INIT|inc|type|name|uuid[|i]`
///   -> `CLOUD_INIT|inc|type|name|vm_id|uuid[|i]`.
fn with_vm_id(old_key: &str, vm_id: &str) -> String {
    let mut segments: Vec<&str> = old_key.split('|').collect();
    segments.insert(4, vm_id);
    segments.join("|")
}

fn without_vm_id(current_key: &str) -> String {
    let mut segments: Vec<&str> = current_key.split('|').collect();
    segments.remove(4);
    segments.join("|")
}

/// Append the given records to a fresh guest pool and normalize them.
fn entries_of<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> Vec<Entry> {
    let dir = TempDir::new().unwrap();
    let store = store_at(&dir);
    store
        .append_multiple(
            pairs
                .iter()
                .map(|(key, value)| (key.as_ref(), value.as_ref())),
        )
        .unwrap();
    DiagnosticReader::new(store).entries().unwrap()
}

fn decode_single(entries: Vec<Entry>) -> Diagnostic {
    assert_eq!(entries.len(), 1, "expected one entry, got: {entries:?}");
    match entries.into_iter().next().unwrap() {
        Entry::Diagnostic(diagnostic) => diagnostic,
        entry => panic!("expected a diagnostic, got {entry:?}"),
    }
}

#[rstest]
#[case::legacy(false)]
#[case::current(true)]
fn captured_compressed_log_reassembles_and_decodes(
    #[case] include_vm_id: bool,
) {
    let records: Vec<(String, &str)> = COMPRESSED_LOG_CHUNKS
        .iter()
        .rev()
        .map(|&(key, value)| {
            let key = if include_vm_id {
                with_vm_id(key, CLOUD_INIT_VM_ID)
            } else {
                key.to_owned()
            };
            (key, value)
        })
        .collect();
    let event = decode_single(entries_of(&records));
    assert_eq!(event.kind(), Kind::Event);
    assert_eq!(
        event.key().vm_id.as_deref(),
        include_vm_id.then_some(CLOUD_INIT_VM_ID)
    );
    assert_eq!(event.key().name, "cloud-init.log");
    assert_eq!(event.key().encoding, Some(Encoding::GzB64));

    let envelope: serde_json::Value =
        serde_json::from_str(EXPECTED_COMPRESSED_MSG).unwrap();
    let data: String = envelope["data"]
        .as_str()
        .unwrap()
        .split_ascii_whitespace()
        .collect();
    let compressed = STANDARD.decode(data).unwrap();
    let mut expected = Vec::new();
    ZlibDecoder::new(compressed.as_slice())
        .read_to_end(&mut expected)
        .unwrap();
    assert_eq!(expected.len(), 3554);
    assert_eq!(event.payload(), &DiagnosticPayload::Bytes(expected));
}

#[test]
fn incomplete_cloud_init_group_preserves_each_physical_record() {
    let base = "CLOUD_INIT|1786047606|event|test|\
                b7a822ba-4eea-46c0-b559-e84396101132";
    let chunk = |index: u32, message: &str| {
        format!(
            r#"{{"name":"test","type":"event","ts":"2026-08-06T20:20:13Z","msg_i":{index},"msg":"{message}"}}"#
        )
    };
    let records = vec![
        (format!("{base}|2"), chunk(2, "third")),
        ("note".to_owned(), "untouched".to_owned()),
        (format!("{base}|0"), chunk(0, "first")),
    ];

    let entries = entries_of(&records);
    let expected: Vec<_> = records
        .into_iter()
        .map(|(key, value)| {
            let error = (key != "note").then_some(DecodeError::IncompleteGroup);
            Entry::Raw(RawKeyValue { key, value, error })
        })
        .collect();
    assert_eq!(entries, expected);
}
