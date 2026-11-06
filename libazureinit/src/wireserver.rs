// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP-only provisioning-health reporting to the Azure wireserver.
//!
//! Callers publish KVP reports independently. Failure delivery uses the
//! supplied report's encoding without changing its timestamp or context.

use libazureinit_kvp::ProvisioningReport;
use reqwest::{
    header::{HeaderMap, HeaderValue, USER_AGENT},
    Client,
};
use std::time::Duration;
use tracing::instrument;

use reqwest::StatusCode;
use serde_json::json;

use crate::config::Wireserver;
use crate::error::Error;
use crate::http;

#[derive(Debug)]
enum ProvisioningState {
    Ready,
    NotReady,
}

#[derive(Debug)]
enum ProvisioningSubStatus {
    ProvisioningFailed,
    Provisioning,
}

impl std::fmt::Display for ProvisioningState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ProvisioningState::Ready => "Ready",
            ProvisioningState::NotReady => "NotReady",
        };
        write!(f, "{s}")
    }
}

impl std::fmt::Display for ProvisioningSubStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ProvisioningSubStatus::ProvisioningFailed => "ProvisioningFailed",
            ProvisioningSubStatus::Provisioning => "Provisioning",
        };
        write!(f, "{s}")
    }
}

/// Reports successful provisioning to the wireserver with a state-only request.
#[instrument(skip_all)]
pub async fn report_ready(config: &Wireserver) -> Result<(), Error> {
    tracing::info!("Reporting provisioning complete");
    _report(ProvisioningState::Ready, None, None, config).await
}

/// Reports provisioning failure using the supplied report's exact encoding.
pub async fn report_failure(
    report: &ProvisioningReport,
    config: &Wireserver,
) -> Result<(), Error> {
    _report(
        ProvisioningState::NotReady,
        Some(ProvisioningSubStatus::ProvisioningFailed),
        Some(report.encode()),
        config,
    )
    .await
}

/// Reports provisioning as still in progress over HTTP, without publishing KVP.
pub async fn report_in_progress(
    config: &Wireserver,
    vm_id: &str,
) -> Result<(), Error> {
    let desc = format!("Provisioning is still in progress for vm_id={vm_id}.");
    _report(
        ProvisioningState::NotReady,
        Some(ProvisioningSubStatus::Provisioning),
        Some(desc),
        config,
    )
    .await
}

/// Internal helper that handles all HTTP details for health reporting to the wireserver.
///
/// Builds the JSON payload, sets required headers, and performs retries as needed.
#[instrument(err, skip_all)]
async fn _report(
    state: ProvisioningState,
    substatus: Option<ProvisioningSubStatus>,
    description: Option<String>,
    config: &Wireserver,
) -> Result<(), Error> {
    tracing::info!(
        target: "libazureinit::health::status",
        "Initiating health report to wireserver: {}",
        state
    );

    let body = if let Some(sub) = substatus {
        json!({
            "state": state.to_string(),
            "details": {
                "subStatus": sub.to_string(),
                "description": description.unwrap_or_default(),
            }
        })
        .to_string()
    } else {
        json!({ "state": state.to_string() }).to_string()
    };

    tracing::debug!(body=%body, "Built provisioning-health JSON");

    let version = env!("CARGO_PKG_VERSION");
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(&format!("azure-init v{version}")).unwrap(),
    );
    headers.insert(
        "x-ms-guest-agent-name",
        HeaderValue::from_str(&format!("azure-init v{version}")).unwrap(),
    );
    headers
        .insert("content-type", HeaderValue::from_static("application/json"));

    tracing::debug!(?headers, "Prepared HTTP headers");

    let connect_timeout =
        Duration::from_secs_f64(config.connection_timeout_secs);
    let read_timeout = Duration::from_secs_f64(config.read_timeout_secs);
    let retry_for = Duration::from_secs_f64(config.total_retry_timeout_secs);

    let client = Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(read_timeout)
        .build()?;

    let mut remaining = retry_for;
    while !remaining.is_zero() {
        let (resp, new_remaining) = http::post(
            &client,
            headers.clone(),
            body.clone(),
            read_timeout,
            connect_timeout,
            remaining,
            &config.health_endpoint,
        )
        .await?;

        let status = resp.status();
        for (key, value) in resp.headers().iter() {
            tracing::info!(
                target: "libazureinit::health::status",
                header = %key,
                value = ?value,
                "Wireserver response header"
            );
        }
        tracing::info!(
            target: "libazureinit::health::status",
            "Wireserver replied with status {}",
            status
        );

        if status.is_success() {
            tracing::info!(
                target: "libazureinit::health::status",
                "Report '{}' succeeded",
                state
            );
            return Ok(());
        }

        if status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::SERVICE_UNAVAILABLE
            || status == StatusCode::INTERNAL_SERVER_ERROR
        {
            tracing::warn!(
                "Retryable HTTP status {} received. Will retry...",
                status
            );
        } else {
            tracing::error!(
                "Non-retryable HTTP status {}, bailing out",
                status
            );
            return Err(Error::HttpStatus {
                endpoint: config.health_endpoint.clone(),
                status,
            });
        }

        remaining = new_remaining;
    }

    tracing::warn!("Report '{}' timed out", state);
    Err(Error::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unittest::{get_http_response_payload, serve_requests};
    use reqwest::StatusCode;
    use rstest::rstest;
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;

    fn fast_config(mock_url: String) -> Wireserver {
        Wireserver {
            connection_timeout_secs: 0.1,
            read_timeout_secs: 0.1,
            total_retry_timeout_secs: 0.5,
            health_endpoint: mock_url,
        }
    }

    /// Verifies that `_report` times out after multiple attempts
    /// for retryable HTTP statuses 400, 404, 410, 429, 500, and 503.
    #[rstest]
    #[case::bad_request(StatusCode::BAD_REQUEST)]
    #[case::not_found(StatusCode::NOT_FOUND)]
    #[case::gone(StatusCode::GONE)]
    #[case::too_many_requests(StatusCode::TOO_MANY_REQUESTS)]
    #[case::internal_server_error(StatusCode::INTERNAL_SERVER_ERROR)]
    #[case::service_unavailable(StatusCode::SERVICE_UNAVAILABLE)]
    #[tokio::test]
    async fn test_report_all_retryable(#[case] status: StatusCode) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://{}", addr);
        let payload = get_http_response_payload(&status, "");
        let cancel = CancellationToken::new();
        let server =
            tokio::spawn(serve_requests(listener, payload, cancel.clone()));

        let cfg = fast_config(mock_url);
        let test_vm_id = "00000000-0000-0000-0000-000000000000";
        let err = Error::UnhandledError {
            details: "test_failure_retryable".to_string(),
        };
        let report = err.as_provisioning_report(test_vm_id);
        let result = report_failure(&report, &cfg).await;
        cancel.cancel();
        let requests = server.await.unwrap();
        assert!(
            matches!(&result, Err(Error::Timeout)),
            "expected a timeout after retrying HTTP {status}, got {result:?}"
        );
        assert!(
            requests >= 2,
            "HTTP {status} should cause multiple attempts, got {requests}"
        );
    }

    /// Verifies that `_report` succeeds immediately
    /// when the server responds with HTTP 200 OK or 201 Created on the first attempt.
    #[tokio::test]
    async fn test_report_immediate_success() {
        for status in [StatusCode::OK, StatusCode::CREATED] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let mock_url = format!("http://{}", addr);
            let payload = get_http_response_payload(&status, "");
            let cancel = CancellationToken::new();
            let server =
                tokio::spawn(serve_requests(listener, payload, cancel.clone()));

            let cfg = fast_config(mock_url);
            let result = report_ready(&cfg).await;

            cancel.cancel();
            assert_eq!(server.await.unwrap(), 1);
            assert!(
                result.is_ok(),
                "{status} should be accepted as success, got {result:?}"
            );
        }
    }

    /// Verifies that `_report` fails after exactly one request
    /// for non-retryable HTTP statuses 401, 403, and 405.
    #[rstest]
    #[case::unauthorized(StatusCode::UNAUTHORIZED)]
    #[case::forbidden(StatusCode::FORBIDDEN)]
    #[case::method_not_allowed(StatusCode::METHOD_NOT_ALLOWED)]
    #[tokio::test]
    async fn test_report_non_retryable_codes(#[case] status: StatusCode) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://{}", addr);
        let payload = get_http_response_payload(&status, "");
        let cancel = CancellationToken::new();
        let server =
            tokio::spawn(serve_requests(listener, payload, cancel.clone()));

        let cfg = fast_config(mock_url);
        let test_vm_id = "00000000-0000-0000-0000-000000000000";
        let err = Error::UnhandledError {
            details: "test_report_non_retryable_codes".to_string(),
        };
        let report = err.as_provisioning_report(test_vm_id);
        let result = report_failure(&report, &cfg).await;
        cancel.cancel();
        let requests = server.await.unwrap();
        assert_eq!(requests, 1, "HTTP {status} should not be retried");
        assert!(
            matches!(
                &result,
                Err(Error::Http(error)) if error.status() == Some(status)
            ),
            "expected HTTP {status}, got {result:?}"
        );
    }

    /// Reporting in progress should succeed on HTTP 200 or 201 immediately.
    #[tokio::test]
    async fn test_report_in_progress_immediate_success() {
        for status in [StatusCode::OK, StatusCode::CREATED] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let mock_url = format!("http://{}", addr);
            let payload = get_http_response_payload(&status, "");
            let cancel = CancellationToken::new();
            let server =
                tokio::spawn(serve_requests(listener, payload, cancel.clone()));

            let cfg = fast_config(mock_url);
            let test_vm_id = "00000000-0000-0000-0000-000000000000";
            let result = report_in_progress(&cfg, test_vm_id).await;

            cancel.cancel();
            assert_eq!(server.await.unwrap(), 1);
            assert!(
                result.is_ok(),
                "{status} should be accepted as success, got {result:?}"
            );
        }
    }

    /// The public wrappers at least compile and call into `_report`.
    /// Here we point at an unresponsive loopback endpoint, and with tiny timeouts
    /// we expect both `report_ready` and `report_failure` to error out fast.
    #[tokio::test]
    async fn test_public_wrappers_error_on_dead_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut cfg = fast_config(format!("http://{addr}"));
        cfg = Wireserver {
            connection_timeout_secs: 0.01,
            read_timeout_secs: 0.01,
            total_retry_timeout_secs: 0.01,
            ..cfg
        };

        let test_vm_id = "00000000-0000-0000-0000-000000000000";
        let r1 = report_ready(&cfg).await;
        let err = Error::UnhandledError {
            details: "no config".to_string(),
        };
        let report = err.as_provisioning_report(test_vm_id);
        let r2 = report_failure(&report, &cfg).await;
        assert!(
            matches!(&r1, Err(Error::Timeout)),
            "report_ready should time out against an unresponsive server, got {r1:?}"
        );
        assert!(
            matches!(&r2, Err(Error::Timeout)),
            "report_failure should time out against an unresponsive server, got {r2:?}"
        );
    }
}
