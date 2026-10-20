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
///
/// Read the complete headers and any Content-Length body before replying.
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

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Client;
    use std::time::Duration;
    use tokio::{net::TcpStream, time::timeout};

    #[tokio::test]
    async fn serve_requests_waits_for_complete_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let payload = get_http_response_payload(&StatusCode::OK, "response");
        let server = tokio::spawn(serve_requests(
            listener,
            payload.clone(),
            cancel.clone(),
        ));
        let mut client = TcpStream::connect(addr).await.unwrap();
        let mut byte = [0; 1];

        client
            .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\n")
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(100), client.read(&mut byte))
                .await
                .is_err(),
            "mock replied before the request headers were complete"
        );

        client
            .write_all(b"cOnTeNt-LeNgTh: 4\r\n\r\nte")
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(100), client.read(&mut byte))
                .await
                .is_err(),
            "mock replied before the request body was complete"
        );

        client.write_all(b"st").await.unwrap();
        let mut response = String::new();
        timeout(Duration::from_secs(5), client.read_to_string(&mut response))
            .await
            .expect("mock did not reply to the completed request")
            .unwrap();

        cancel.cancel();
        assert_eq!(server.await.unwrap(), 1);
        assert_eq!(response, payload);
    }

    #[tokio::test]
    async fn serve_requests_survives_disconnected_client() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let server = tokio::spawn(serve_requests(
            listener,
            get_http_response_payload(&StatusCode::OK, "response"),
            cancel.clone(),
        ));

        let disconnected = TcpStream::connect(addr).await.unwrap();
        drop(disconnected);

        let response = Client::new()
            .get(format!("http://{addr}"))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .expect("mock failed after a client disconnected");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "response");

        cancel.cancel();
        assert_eq!(server.await.unwrap(), 1);
    }
}
