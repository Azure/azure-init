// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::{
    header::{HeaderMap, HeaderValue, USER_AGENT},
    Client,
};
use tracing::instrument;

use std::time::Duration;

use reqwest::StatusCode;
use serde_json::json;

use crate::error::Error;
use crate::http;

const DEFAULT_HEALTH_URL: &str = "http://168.63.129.16/provisioning/health";
/// Build JSON for a provisioning health report.
///
/// Example:
/// ```json
/// {
///   "state": "NotReady",
///   "details": {
///     "subStatus": "ProvisioningFailed",
///     "description": "Some error"
///   }
/// }
/// ```
/// If `state == "Ready"`, we omit `details` entirely.
fn build_provisioning_health_json(
    state: &str,
    sub_status: Option<&str>,
    description: Option<&str>,
) -> String {
    if state == "NotReady" {
        let details_obj = json!({
            "subStatus": sub_status.unwrap_or("Provisioning"),
            "description": description
        });
        json!({
            "state": state,
            "details": details_obj
        })
        .to_string()
    } else {
        json!({
            "state": state
        })
        .to_string()
    }
}

#[instrument(err, skip_all)]
pub async fn report_provisioning_health(
    state: &str,
    sub_status: Option<&str>,
    description: Option<&str>,
    retry_interval: Duration,
    mut total_timeout: Duration,
    url: Option<&str>,
) -> Result<(), Error> {
    let post_request =
        build_provisioning_health_json(state, sub_status, description);

    let mut headers = HeaderMap::new();
    let version = env!("CARGO_PKG_VERSION");

    let user_agent = format!("azure-init v{}", version);
    headers.insert(USER_AGENT, HeaderValue::from_str(&user_agent).unwrap());

    headers.insert(
        "x-ms-guest-agent-name",
        format!("azure-init/{}", version).parse().unwrap(),
    );
    headers.insert("Content-Type", "application/json".parse().unwrap());

    let client = Client::builder()
        .timeout(Duration::from_secs(
            crate::http::WIRESERVER_HTTP_TIMEOUT_SEC,
        ))
        .build()?;

    let url = url.unwrap_or(DEFAULT_HEALTH_URL);

    let request_timeout =
        Duration::from_secs(http::WIRESERVER_HTTP_TIMEOUT_SEC);

    while !total_timeout.is_zero() {
        let (response, remaining_timeout) = http::post(
            &client,
            headers.clone(),
            post_request.clone(),
            request_timeout,
            retry_interval,
            total_timeout,
            url,
        )
        .await?;

        tracing::debug!("Received status: {}", response.status());
        tracing::debug!("Remaining timeout: {:?}", remaining_timeout);

        match response.status() {
            StatusCode::CREATED => {
                return Ok(());
            }
            StatusCode::TOO_MANY_REQUESTS => {
                tracing::warn!(
                    "Got 429 from wireserver: rate-limited. Retrying..."
                );
            }
            StatusCode::SERVICE_UNAVAILABLE => {
                tracing::warn!(
                    "Got 503 from wireserver: not ready. Retrying..."
                );
            }
            other => {
                return Err(Error::HttpStatus {
                    endpoint: url.to_owned(),
                    status: other,
                });
            }
        }

        total_timeout = remaining_timeout;
    }

    Err(Error::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    use crate::unittest;
    use reqwest::StatusCode;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn test_build_report_health_not_ready() {
        let json_str = build_provisioning_health_json(
            "NotReady",
            Some("ProvisioningFailed"),
            Some("Test error"),
        );
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["state"], "NotReady");
        assert_eq!(parsed["details"]["subStatus"], "ProvisioningFailed");
        assert_eq!(parsed["details"]["description"], "Test error");
    }

    /// Verifies that `report_provisioning_health` times out after multiple attempts
    /// when the server consistently responds with a retryable error (HTTP 503).
    #[tokio::test]
    async fn test_report_provisioning_health_all_retryable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = unittest::get_http_response_payload(
            &StatusCode::SERVICE_UNAVAILABLE,
            "",
        );
        let cancel_token = CancellationToken::new();
        let server = tokio::spawn(unittest::serve_requests(
            listener,
            payload.clone(),
            cancel_token.clone(),
        ));

        let result = report_provisioning_health(
            "NotReady",
            Some("ProvisioningFailed"),
            Some("Test error"),
            Duration::from_millis(100),
            Duration::from_secs(5),
            Some(&format!("http://{}", addr)),
        )
        .await;

        assert!(result.is_err());

        cancel_token.cancel();
        let request_count = server.await.unwrap();
        assert!(request_count >= 3);
    }

    /// Verifies that `report_provisioning_health` succeeds immediately
    /// when the server responds with HTTP 201 Created on the first attempt.
    #[tokio::test]
    async fn test_report_provisioning_health_immediate_success() {
        let payload =
            unittest::get_http_response_payload(&StatusCode::CREATED, "");

        println!("Payload built: {:?}", payload);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        println!("Listener bound at: {}", addr);

        let cancel_token = CancellationToken::new();
        let server = tokio::spawn(crate::unittest::serve_post_requests(
            listener,
            payload.clone(),
            cancel_token.clone(),
        ));

        println!("Calling report_provisioning_health...");
        let result = report_provisioning_health(
            "Ready",
            None,
            None,
            Duration::from_secs(1),
            Duration::from_secs(5),
            Some(&format!("http://{}", addr)),
        )
        .await;

        println!("Result: {:?}", result);
        assert!(result.is_ok());

        cancel_token.cancel();
        let request_count = server.await.unwrap();
        println!("Total request count: {}", request_count);
        assert_eq!(request_count, 1);
    }

    /// Verifies that `report_provisioning_health` fails immediately
    /// when the server responds with a non-retryable error (e.g. HTTP 400).
    #[tokio::test]
    async fn test_report_provisioning_health_unexpected_code() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let response =
            unittest::get_http_response_payload(&StatusCode::BAD_REQUEST, "");

        let cancel_token = CancellationToken::new();
        let server = tokio::spawn(unittest::serve_requests(
            listener,
            response,
            cancel_token.clone(),
        ));

        let result = report_provisioning_health(
            "NotReady",
            Some("ProvisioningFailed"),
            Some("Test error"),
            Duration::from_secs(1),
            Duration::from_secs(5),
            Some(&format!("http://{}", addr)),
        )
        .await;

        cancel_token.cancel();
        let request_count = server.await.unwrap();

        assert_eq!(request_count, 5);
        assert!(result.is_err());
    }
}
