// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use libazureinit_kvp::{
    Diagnostic, DiagnosticPayload, DiagnosticReader, DiagnosticWriter,
    DiagnosticsKvp, Entry, KvpPool, KvpPoolStore, PoolMode,
};
use reqwest::StatusCode;
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{filter::LevelFilter, prelude::*};

pub(crate) fn capture_kvp_at_info<T>(
    operation: impl FnOnce() -> T,
) -> (T, Vec<Diagnostic>) {
    let dir = TempDir::new().unwrap();
    let store =
        KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe)
            .unwrap();
    let writer = DiagnosticWriter::new(
        store.clone(),
        "test/1",
        "00000000-0000-0000-0000-000000000001",
    )
    .unwrap();
    let subscriber = tracing_subscriber::registry()
        .with(DiagnosticsKvp::new(writer).with_filter(LevelFilter::INFO));
    let result = tracing::subscriber::with_default(subscriber, operation);
    let diagnostics = DiagnosticReader::new(store)
        .entries()
        .unwrap()
        .into_iter()
        .map(|entry| match entry {
            Entry::Diagnostic(diagnostic) => diagnostic,
            other => panic!("expected a diagnostic, got {other:?}"),
        })
        .collect();
    (result, diagnostics)
}

pub(crate) fn kvp_error_fields(diagnostics: &[Diagnostic]) -> Vec<Value> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let Diagnostic::Event(event) = diagnostic else {
                return None;
            };
            let DiagnosticPayload::Text(text) = &event.payload else {
                panic!("expected a JSON text payload");
            };
            let payload: Value = serde_json::from_str(text).unwrap();
            (payload["level"] == "ERROR").then(|| payload["fields"].clone())
        })
        .collect()
}

/// Returns expected HTTP response for the given status code and body string.
pub(crate) fn get_http_response_payload(
    statuscode: &StatusCode,
    body_str: &str,
) -> String {
    // Reply message includes the whole body in case of OK, otherwise empty data.
    let res = match statuscode {
            &StatusCode::OK => format!("HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", statuscode.as_u16(), statuscode.to_string(), body_str.len(), body_str.to_string()),
            _ => {
                format!("HTTP/1.1 {} {}\r\n\r\n", statuscode.as_u16(), statuscode.to_string())
            }
        };

    res
}

/// Accept incoming connections until the cancellation token is used, then return the count
/// of accepted connections.
pub(crate) async fn serve_requests(
    listener: TcpListener,
    payload: String,
    cancel_token: CancellationToken,
) -> u32 {
    let mut request_count = 0;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                break;
            }
            _ = async {
                let (mut serverstream, _) = listener.accept().await.unwrap();

                serverstream.write_all(payload.as_bytes()).await.unwrap();
            } => {
                request_count += 1;
            }
        }
    }

    request_count
}
