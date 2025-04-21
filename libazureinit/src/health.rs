// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::{
    header::{HeaderMap, HeaderValue, USER_AGENT},
    Client,
};
use std::time::Duration;
use tracing::instrument;

use reqwest::StatusCode;
use serde_json::json;

use crate::config::Config;
use crate::error::Error;
use crate::http;

const DEFAULT_HEALTH_URL: &str = "http://168.63.129.16/provisioning/health";

/// Report that provisioning is ready.
pub async fn report_ready(
    config: &Config,
    url_override: Option<&str>,
) -> Result<(), Error> {
    tracing::info!("Reporting ready provisioning health");
    _report("Ready", None, None, &config.wireserver, url_override).await
}

/// Report that provisioning has failed.
pub async fn report_failure(
    message: &str,
    config: &Config,
    url_override: Option<&str>,
) -> Result<(), Error> {
    tracing::info!(failure_reason=%message, "Reporting failure provisioning health");
    _report(
        "NotReady",
        Some("ProvisioningFailed"),
        Some(message),
        &config.wireserver,
        url_override,
    )
    .await
}

/// Internal helper: all of the header setup, JSON‐body construction, retry loop, etc.
#[instrument(err, skip_all)]
async fn _report(
    state: &str,
    substatus: Option<&str>,
    description: Option<&str>,
    cfg: &crate::config::Wireserver,
    url_override: Option<&str>,
) -> Result<(), Error> {
    let body = if state == "NotReady" {
        json!({
            "state": state,
            "details": {
                "subStatus": substatus.unwrap_or("Provisioning"),
                "description": description
            }
        })
        .to_string()
    } else {
        json!({ "state": state }).to_string()
    };

    tracing::debug!(body=%body, "Built provisioning-health JSON");

    let version = env!("CARGO_PKG_VERSION");
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(&format!("azure-init v{}", version)).unwrap(),
    );
    headers.insert(
        "x-ms-guest-agent-name",
        HeaderValue::from_str(&format!("azure-init/{}", version)).unwrap(),
    );
    headers
        .insert("content-type", HeaderValue::from_static("application/json"));

    tracing::info!(?headers, "Prepared HTTP headers");

    let url = url_override.unwrap_or(DEFAULT_HEALTH_URL);

    let connect_timeout = Duration::from_secs_f64(cfg.connection_timeout_secs);
    let read_timeout = Duration::from_secs_f64(cfg.read_timeout_secs);
    let retry_for = Duration::from_secs_f64(cfg.total_retry_timeout_secs);

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
            url,
        )
        .await?;

        match resp.status() {
            StatusCode::CREATED | StatusCode::OK => {
                tracing::info!("Provisioning-health report succeeded");
                return Ok(());
            }
            StatusCode::TOO_MANY_REQUESTS => {
                tracing::warn!("429 from wireserver, retrying…")
            }
            StatusCode::SERVICE_UNAVAILABLE => {
                tracing::warn!("503 from wireserver, retrying…")
            }
            other => {
                tracing::warn!("Non-retryable status, bailing out");
                return Err(Error::HttpStatus {
                    endpoint: url.to_string(),
                    status: other,
                });
            }
        }

        remaining = new_remaining;
    }

    tracing::warn!("Provisioning-health report timed out");
    Err(Error::Timeout)
}

#[cfg(test)]
mod tests {
    use super::{report_failure, report_ready};
    use crate::config::{Config, Wireserver};
    use crate::unittest::{get_http_response_payload, serve_requests};
    use reqwest::StatusCode;
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;

    fn fast_config() -> Config {
        let mut cfg = Config::default();
        cfg.wireserver = Wireserver {
            connection_timeout_secs: 0.01,
            read_timeout_secs: 0.01,
            total_retry_timeout_secs: 0.05,
            ..Default::default()
        };
        cfg
    }

    /// Verifies that `_report` times out after multiple attempts
    /// when the server consistently responds with a retryable error (HTTP 503).
    #[tokio::test]
    async fn test_report_all_retryable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let payload =
            get_http_response_payload(&StatusCode::SERVICE_UNAVAILABLE, "");
        let cancel = CancellationToken::new();
        let _server = tokio::spawn(serve_requests(
            listener,
            payload.clone(),
            cancel.clone(),
        ));

        let cfg = fast_config();
        let result =
            report_failure("oops", &cfg, Some(&format!("http://{}", addr)))
                .await;

        assert!(result.is_err(), "should have timed out after retrying");
        cancel.cancel();
    }

    /// Verifies that `_report` succeeds immediately
    /// when the server responds with HTTP 201 Created on the first attempt.
    #[tokio::test]
    async fn test_report_immediate_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = get_http_response_payload(&StatusCode::CREATED, "");
        let cancel = CancellationToken::new();
        let _server = tokio::spawn(serve_requests(
            listener,
            payload.clone(),
            cancel.clone(),
        ));

        let cfg = fast_config();
        let result =
            report_ready(&cfg, Some(&format!("http://{}", addr))).await;
        assert!(result.is_ok(), "201 Created should be accepted as success");

        cancel.cancel();
    }

    /// Verifies that `_report` fails immediately
    /// when the server responds with a non-retryable error (e.g. HTTP 400).
    #[tokio::test]
    async fn test_report_unexpected_code() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = get_http_response_payload(&StatusCode::BAD_REQUEST, "");
        let cancel = CancellationToken::new();
        let _server = tokio::spawn(serve_requests(
            listener,
            payload.clone(),
            cancel.clone(),
        ));

        let cfg = fast_config();
        let result =
            report_failure("err", &cfg, Some(&format!("http://{}", addr)))
                .await;

        assert!(result.is_err(), "400 Bad Request should fail immediately");
        cancel.cancel();
    }

    /// The public wrappers at least compile and call into `_report`.
    /// Here we point at a “dead” endpoint (no test server), and with tiny timeouts
    /// we expect both `report_ready` and `report_failure` to error out fast.
    #[tokio::test]
    async fn test_public_wrappers_error_on_dead_endpoint() {
        let mut cfg = fast_config();
        // shrink the wireserver timeouts so we fail immediately
        cfg.wireserver = crate::config::Wireserver {
            connection_timeout_secs: 0.01,
            read_timeout_secs: 0.01,
            total_retry_timeout_secs: 0.01,
            ..Default::default()
        };

        // no override == real DEFAULT_HEALTH_URL, which we can't reach in tests
        let r1 = report_ready(&cfg, None).await;
        let r2 = report_failure("no config", &cfg, None).await;
        assert!(
            r1.is_err(),
            "report_ready should fail against a dead server"
        );
        assert!(r2.is_err(), "report_failure should also fail");
    }
}
