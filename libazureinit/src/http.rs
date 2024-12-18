// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use reqwest::{header::HeaderMap, Client, Request, StatusCode};
use tokio::time::timeout;
use tracing::{instrument, Instrument};

use crate::error::Error;

/// Set of StatusCodes that should be retried,
/// e.g. 400, 404, 410, 429, 500, 503.
///
/// # Example
///
/// ```rust,ignore
/// # use libazureinit::http::RETRY_CODES;
/// # use reqwest::StatusCode;
///
/// assert!(RETRY_CODES.contains(StatusCode::NOT_FOUND));
/// ```
pub(crate) const RETRY_CODES: &[StatusCode] = &[
    StatusCode::BAD_REQUEST,
    StatusCode::NOT_FOUND,
    StatusCode::GONE,
    StatusCode::TOO_MANY_REQUESTS,
    StatusCode::INTERNAL_SERVER_ERROR,
    StatusCode::SERVICE_UNAVAILABLE,
];

/// Set of StatusCodes that should immediately fail,
/// e.g. 401, 403, 405.
///
/// # Example
///
/// ```rust,ignore
/// # use libazureinit::http::HARDFAIL_CODES;
/// # use reqwest::StatusCode;
///
/// assert!(HARDFAIL_CODES.contains(StatusCode::FORBIDDEN));
/// ```
#[allow(dead_code)]
pub(crate) const HARDFAIL_CODES: &[StatusCode] = &[
    StatusCode::UNAUTHORIZED,
    StatusCode::FORBIDDEN,
    StatusCode::METHOD_NOT_ALLOWED,
];

/// Timeout for communicating with IMDS.
pub(crate) const IMDS_HTTP_TIMEOUT_SEC: u64 = 30;
/// Timeout for communicating with wireserver for goalstate, health.
pub(crate) const WIRESERVER_HTTP_TIMEOUT_SEC: u64 = 30;

/// Send an HTTP GET request to the given URL with an empty body.
#[instrument(err, skip_all)]
pub(crate) async fn get(
    client: &Client,
    headers: HeaderMap,
    request_timeout: Duration,
    retry_interval: Duration,
    retry_for: Duration,
    url: &str,
) -> Result<(reqwest::Response, Duration), Error> {
    let req = client
        .get(url)
        .headers(headers)
        .timeout(request_timeout)
        .build()?;
    request(client, req, retry_interval, retry_for).await
}

/// Send an HTTP GET request to the given URL with an empty body.
///
/// `body` must implement Clone as retries must clone the entire request.
#[instrument(err, skip_all)]
pub(crate) async fn post<T: Into<reqwest::Body> + Clone>(
    client: &Client,
    headers: HeaderMap,
    body: T,
    request_timeout: Duration,
    retry_interval: Duration,
    retry_for: Duration,
    url: &str,
) -> Result<(reqwest::Response, Duration), Error> {
    let req = client
        .post(url)
        .headers(headers)
        .body(body)
        .timeout(request_timeout)
        .build()?;
    request(client, req, retry_interval, retry_for).await
}

/// Retry an HTTP request until it returns HTTP 200 or the timeout is reached.
///
/// In the event that the request succeeds, the total remaining timeout is returned with the response.
/// This can be used to resume retrying in the event that the body is malformed.
///
/// # Panics
///
/// This function will panic if the request passed cannot be cloned (i.e. the body is a Stream).
/// Functions wrapping this must ensure to include an additional bound on `Body` (see [`post`]).
async fn request(
    client: &Client,
    request: Request,
    retry_interval: Duration,
    retry_for: Duration,
) -> Result<(reqwest::Response, Duration), Error> {
    timeout(retry_for, async {
        let now = std::time::Instant::now();
        let mut attempt =  0_u32;
        loop {
            let span = tracing::debug_span!("request", attempt, http_status = tracing::field::Empty);
            let req = request.try_clone().expect("The request body MUST be clone-able");
            match client
                .execute(req)
                .instrument(span.clone())
                .await {
                    Ok(response) => {
                        let _enter = span.enter();
                        let statuscode = response.status();
                        span.record("http_status", statuscode.as_u16());
                        tracing::info!(url=response.url().as_str(), "HTTP response received");

                        match response.error_for_status() {
                            Ok(response) => {
                                if statuscode == StatusCode::OK {
                                    tracing::info!("HTTP response succeeded with status {}", statuscode);
                                    return Ok((response, retry_for.saturating_sub(now.elapsed() + retry_interval)));
                                }
                            },
                            Err(error) => {
                                if !RETRY_CODES.contains(&statuscode) {
                                    tracing::error!(
                                        ?error,
                                        "HTTP response status code is fatal and the request will not be retried",
                                    );
                                    return Err(error.into());
                                }
                            },
                        }

                    },
                    Err(error) => {
                        let _enter = span.enter();
                        tracing::error!(?error, "HTTP request failed to complete");
                    },
                }
            span.in_scope(||{
                tracing::warn!(
                    "Failed to get a successful HTTP response, retrying in {} sec, remaining timeout {} sec.",
                    retry_interval.as_secs(),
                    retry_for.saturating_sub(now.elapsed()).as_secs()
                );
            });
            // Explicitly dropping here to ensure the sleep isn't included in the request timings
            drop(span);

            attempt += 1;
            tokio::time::sleep(retry_interval).await;
        }
    }).await?
}

#[cfg(test)]
pub(crate) mod tests {
    use reqwest::{header, Client, StatusCode};
    use std::time::Duration;
    use tokio::{io::AsyncWriteExt, net::TcpListener};

    use crate::unittest::{get_http_response_payload, serve_requests};

    const BODY_CONTENTS: &str = "hello world";

    // Helper that returns how many attempts were made on a given HTTP status code.
    async fn serve_valid_http_with(
        statuscode: &StatusCode,
        body: &str,
    ) -> bool {
        let serverlistener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = serverlistener.local_addr().unwrap();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let server = tokio::spawn(serve_requests(
            serverlistener,
            get_http_response_payload(statuscode, body),
            cancel_token.clone(),
        ));

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(1))
            .build()
            .unwrap();

        let res = super::get(
            &client,
            header::HeaderMap::new(),
            Duration::from_millis(500),
            Duration::from_millis(5),
            Duration::from_millis(100),
            format!("http://{:}:{:}/", addr.ip(), addr.port()).as_str(),
        )
        .await;

        cancel_token.cancel();

        let requests = server.await.unwrap();

        if super::HARDFAIL_CODES.contains(statuscode) {
            assert_eq!(requests, 1);
        }

        if *statuscode == StatusCode::OK {
            assert_eq!(requests, 1);
        }

        if super::RETRY_CODES.contains(statuscode) {
            assert!(requests >= 10);
        }

        res.is_ok()
    }

    // Assert requests that don't receive data after the connection is accepted retry.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn get_slow_write() {
        let serverlistener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = serverlistener.local_addr().unwrap();
        let task_cancel = tokio_util::sync::CancellationToken::new();
        let cancel_token = task_cancel.clone();
        let server = tokio::spawn(async move {
            let mut requests_accepted = 0;
            loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => {
                        break;
                    }
                    _ = async {
                        let (mut serverstream, _) = serverlistener.accept().await.unwrap();
                        requests_accepted += 1;
                        // Do this asynchronously so we accept the next request in a timely manner;
                        // there's a separate test for slow accepts.
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            let _ = serverstream.write_all(
                                get_http_response_payload(&StatusCode::FORBIDDEN, "too slow").as_bytes()
                            ).await;
                        });
                    } => {}
                }
            }
            requests_accepted
        });

        let client = Client::builder().build().unwrap();

        let res = super::get(
            &client,
            header::HeaderMap::new(),
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(500),
            format!("http://{:}:{:}/", addr.ip(), addr.port()).as_str(),
        )
        .await;

        cancel_token.cancel();

        let requests = server.await.unwrap();
        assert!(requests >= 2);
        match res {
            Err(crate::error::Error::Timeout) => {}
            _ => panic!("Response should have timed out"),
        };
    }

    // Assert requests that never get accepted retry
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn get_slow_accept() {
        let serverlistener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = serverlistener.local_addr().unwrap();
        let task_cancel = tokio_util::sync::CancellationToken::new();
        let cancel_token = task_cancel.clone();
        let server = tokio::spawn(async move {
            let mut requests_attempted = 0;
            loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => {
                        break;
                    }
                    _ = async {
                        requests_attempted += 1;
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        let _ = serverlistener.accept().await;
                    } => {}
                }
            }
            requests_attempted
        });

        let client = Client::builder().build().unwrap();

        let res = super::get(
            &client,
            header::HeaderMap::new(),
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(1000),
            format!("http://{:}:{:}/", addr.ip(), addr.port()).as_str(),
        )
        .await;

        cancel_token.cancel();

        let requests = server.await.unwrap();
        assert!(requests >= 2);
        match res {
            Err(crate::error::Error::Timeout) => {}
            _ => panic!("Response should have timed out"),
        };
    }

    // Assert a response with 200 OK is returned.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn get_ok() {
        assert!(serve_valid_http_with(&StatusCode::OK, BODY_CONTENTS).await);
        assert!(logs_contain("HTTP response succeeded with status 200 OK"));
    }

    // Assert status codes in the list are retried
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn get_retry_responses() {
        for rc in super::RETRY_CODES {
            assert!(!serve_valid_http_with(rc, BODY_CONTENTS).await);
        }
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn get_fast_fail() {
        // status codes that should result into immediate failures.
        for rc in super::HARDFAIL_CODES {
            assert!(!serve_valid_http_with(rc, BODY_CONTENTS).await);
        }
    }
}
