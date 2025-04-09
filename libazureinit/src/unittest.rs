// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::StatusCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

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

/// Accepts incoming connections until the cancellation token is triggered.
/// For each accepted connection, it reads some of the incoming request (to drain it),
/// writes the provided payload, flushes the stream, and then explicitly shuts down the connection.
/// This helper simulates a proper POST server.
pub(crate) async fn serve_post_requests(
    listener: TcpListener,
    payload: String,
    cancel_token: CancellationToken,
) -> u32 {
    let mut request_count = 0;
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => break,
            result = listener.accept() => {
                let (mut stream, _) = result.expect("Failed to accept connection");

                // Drain incoming request data (simulate reading the POST body)
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await; // We ignore the content

                // Wait briefly to give the client time to finish sending the request.
                sleep(Duration::from_millis(10)).await;

                // Now write the full response payload.
                stream.write_all(payload.as_bytes()).await.expect("Failed to write payload");
                stream.flush().await.expect("Failed to flush stream");
                let _ = stream.shutdown().await;

                request_count += 1;
            }
        }
    }
    request_count
}
