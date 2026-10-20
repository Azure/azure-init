// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io;

use reqwest::StatusCode;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
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

/// Serve incoming requests until cancellation, then return the number served.
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
            result = async {
                let (serverstream, _) = listener.accept().await?;
                let mut serverstream = BufReader::new(serverstream);
                let mut content_length = 0;

                // Do not race the client's request with an unsolicited response.
                loop {
                    let mut line = String::new();
                    if serverstream.read_line(&mut line).await? == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "request closed before its headers completed",
                        ));
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            content_length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                }
                let mut body = vec![0; content_length];
                serverstream.read_exact(&mut body).await?;
                serverstream.get_mut().write_all(payload.as_bytes()).await?;
                Ok::<_, io::Error>(())
            } => {
                match result {
                    Ok(()) => request_count += 1,
                    Err(error) if matches!(
                        error.kind(),
                        io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::BrokenPipe
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                    ) => {
                        tracing::debug!(?error, "HTTP mock client disconnected");
                    }
                    Err(error) => panic!("HTTP mock failed: {error}"),
                }
            }
        }
    }

    request_count
}
