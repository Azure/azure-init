// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::{
    header::{HeaderMap, HeaderValue, USER_AGENT},
    Client,
};
use std::time::Duration;
use tracing::instrument;

use chrono::Utc;
use reqwest::StatusCode;
use serde_json::json;

use crate::config::Config;
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

/// Constructs a KVP entry representing a successful provisioning event.
pub fn encoded_success_report(
    vm_id: &str,
    optional_key_value: Option<(&str, &str)>,
) -> String {
    let agent = format!("Azure-Init/{}", env!("CARGO_PKG_VERSION"));
    let timestamp = Utc::now().to_rfc3339();

    let mut data = vec![
        "result=success".to_string(),
        format!("agent={}", agent),
        "pps_type=None".to_string(),
        format!("vm_id={}", vm_id),
        format!("timestamp={}", timestamp),
    ];
    if let Some((k, v)) = optional_key_value {
        data.push(format!("{k}={v}"));
    }
    encode_report(&data)
}

/// Serializes a slice of key-value strings as a single pipe-delimited entry.
pub fn encode_report(data: &[String]) -> String {
    let mut wtr = csv::WriterBuilder::new()
        .delimiter(b'|')
        .quote_style(csv::QuoteStyle::Necessary)
        .from_writer(vec![]);
    wtr.write_record(data).expect("CSV write failed");
    let mut bytes = wtr.into_inner().unwrap();
    if let Some(b'\n') = bytes.last() {
        bytes.pop();
    }
    if let Some(b'\r') = bytes.last() {
        bytes.pop();
    }
    String::from_utf8(bytes).expect("CSV was not utf-8")
}

/// Reports provisioning as successfully completed to the wireserver and/or KVP.
pub async fn report_ready(
    config: &Config,
    vm_id: &str,
    optional_key_value: Option<(&str, &str)>,
) -> Result<(), Error> {
    tracing::info!("Reporting provisioning complete");
    let desc = encoded_success_report(vm_id, optional_key_value);
    _report(ProvisioningState::Ready, None, Some(desc), config).await
}

/// Reports provisioning failure to the wireserver and/or KVP.
pub async fn report_failure(
    report_str: String,
    config: &Config,
) -> Result<(), Error> {
    _report(
        ProvisioningState::NotReady,
        Some(ProvisioningSubStatus::ProvisioningFailed),
        Some(report_str),
        config,
    )
    .await
}

/// Reports provisioning as still in progress to the wireserver and/or KVP.
pub async fn report_in_progress(
    config: &Config,
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
    config: &Config,
) -> Result<(), Error> {
    if let Some(description_str) = &description {
        tracing::info!(
            target: "libazureinit::health::report",
            health_report = %description_str
        );
    }

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
        Duration::from_secs_f64(config.wireserver.connection_timeout_secs);
    let read_timeout =
        Duration::from_secs_f64(config.wireserver.read_timeout_secs);
    let retry_for =
        Duration::from_secs_f64(config.wireserver.total_retry_timeout_secs);

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
            &config.wireserver.health_endpoint,
        )
        .await?;

        tracing::info!(
            target: "libazureinit::health::status",
            "Wireserver responded with {:?}",
            resp
        );

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
                endpoint: config.wireserver.health_endpoint.clone(),
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
    use crate::config::{Config, Wireserver};
    use crate::unittest::{get_http_response_payload, serve_requests};
    use reqwest::StatusCode;
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;

    fn fast_config(mock_url: Option<String>) -> Config {
        let mut cfg = Config::default();
        cfg.wireserver = Wireserver {
            connection_timeout_secs: 0.01,
            read_timeout_secs: 0.01,
            total_retry_timeout_secs: 0.05,
            health_endpoint: mock_url.unwrap_or(cfg.wireserver.health_endpoint),
        };
        cfg
    }

    /// Verifies that `_report` times out after multiple attempts
    /// when the server consistently responds with a retryable error (HTTP 503).
    #[tokio::test]
    async fn test_report_all_retryable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://{}", addr);
        let payload =
            get_http_response_payload(&StatusCode::SERVICE_UNAVAILABLE, "");
        let cancel = CancellationToken::new();
        let _server = tokio::spawn(serve_requests(
            listener,
            payload.clone(),
            cancel.clone(),
        ));

        let cfg = fast_config(Some(mock_url));
        let test_vm_id = "00000000-0000-0000-0000-000000000000";
        let err = Error::UnhandledError {
            details: "test_failure_retryable".to_string(),
        };
        let report_str = err.as_encoded_report(test_vm_id);
        let result = report_failure(report_str, &cfg).await;
        assert!(result.is_err(), "should have timed out after retrying");
        cancel.cancel();
    }

    /// Verifies that `_report` succeeds immediately
    /// when the server responds with HTTP 201 Created on the first attempt.
    #[tokio::test]
    async fn test_report_immediate_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://{}", addr);
        let payload = get_http_response_payload(&StatusCode::CREATED, "");
        let cancel = CancellationToken::new();
        let _server = tokio::spawn(serve_requests(
            listener,
            payload.clone(),
            cancel.clone(),
        ));

        let cfg = fast_config(Some(mock_url));
        let test_vm_id = "00000000-0000-0000-0000-000000000000";
        let result = report_ready(&cfg, test_vm_id, None).await;
        assert!(result.is_ok(), "201 Created should be accepted as success");

        cancel.cancel();
    }

    /// Verifies that `_report` fails immediately
    /// when the server responds with a non-retryable error (e.g. HTTP 400).
    #[tokio::test]
    async fn test_report_unexpected_code() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://{}", addr);
        let payload = get_http_response_payload(&StatusCode::BAD_REQUEST, "");
        let cancel = CancellationToken::new();
        let _server = tokio::spawn(serve_requests(
            listener,
            payload.clone(),
            cancel.clone(),
        ));

        let cfg = fast_config(Some(mock_url));
        let test_vm_id = "00000000-0000-0000-0000-000000000000";
        let err = Error::UnhandledError {
            details: "test_report_unexpected_code".to_string(),
        };
        let report_str = err.as_encoded_report(test_vm_id);
        let result = report_failure(report_str, &cfg).await;
        assert!(result.is_err(), "400 Bad Request should fail immediately");
        cancel.cancel();
    }

    /// “InProgress” should be treated as success on 200 or 201 immediately.
    #[tokio::test]
    async fn test_report_in_progress_immediate_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_url = format!("http://{}", addr);
        let payload = get_http_response_payload(&StatusCode::CREATED, "");
        let cancel = CancellationToken::new();
        let _server =
            tokio::spawn(serve_requests(listener, payload, cancel.clone()));

        let cfg = fast_config(Some(mock_url));
        let test_vm_id = "00000000-0000-0000-0000-000000000000";
        let res = report_in_progress(&cfg, test_vm_id).await;
        assert!(
            res.is_ok(),
            "201 Created (or 200 OK) should be accepted as success"
        );

        cancel.cancel();
    }

    /// The public wrappers at least compile and call into `_report`.
    /// Here we point at a “dead” endpoint (no test server), and with tiny timeouts
    /// we expect both `report_ready` and `report_failure` to error out fast.
    #[tokio::test]
    async fn test_public_wrappers_error_on_dead_endpoint() {
        let mut cfg = fast_config(None);
        // Shrink the wireserver timeouts so we fail immediately
        cfg.wireserver = crate::config::Wireserver {
            connection_timeout_secs: 0.01,
            read_timeout_secs: 0.01,
            total_retry_timeout_secs: 0.01,
            ..Default::default()
        };

        let test_vm_id = "00000000-0000-0000-0000-000000000000";
        // no override == real health_endpoint, which we can't reach in tests
        let r1 = report_ready(&cfg, test_vm_id, None).await;
        let err = Error::UnhandledError {
            details: "no config".to_string(),
        };
        let report_str = err.as_encoded_report(test_vm_id);
        let r2 = report_failure(report_str, &cfg).await;
        assert!(
            r1.is_err(),
            "report_ready should fail against a dead server"
        );
        assert!(r2.is_err(), "report_failure should also fail");
    }

    // Verifies encoded_success_report() creates the correct
    // success KVP string format, including optional custom key-value pairs.
    #[test]
    fn test_encoded_success_report_format() {
        let vm_id = "00000000-0000-0000-0000-000000000abc";
        let encoded =
            encoded_success_report(vm_id, Some(("build", "test-123")));

        assert!(encoded.contains("result=success"));
        assert!(encoded.contains("agent=Azure-Init/"));
        assert!(encoded.contains("vm_id=00000000-0000-0000-0000-000000000abc"));
        assert!(encoded.contains("build=test-123"));
        assert!(encoded.contains("pps_type=None"));
        assert!(encoded.contains("timestamp="));
        assert!(encoded.contains("|"));
        assert!(!encoded.contains(","));
    }
}
