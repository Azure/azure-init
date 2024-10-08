// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::StatusCode;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
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
