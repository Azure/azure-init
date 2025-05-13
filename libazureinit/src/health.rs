// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::{
    header::{HeaderMap, HeaderValue, USER_AGENT},
    Client,
};
use std::time::Duration;
use tracing::instrument;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;

use crate::config::Config;
use crate::error::Error;
use crate::http;

#[derive(Debug)]
enum ProvisioningState {
    Ready,
    NotReady,
    InProgress,
}

#[derive(Debug)]
enum ProvisioningSubStatus {
    ProvisioningFailed,
    ProvisioningInProgress,
}

impl std::fmt::Display for ProvisioningState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ProvisioningState::Ready => "Ready",
            ProvisioningState::NotReady => "NotReady",
            ProvisioningState::InProgress => "InProgress",
        };
        write!(f, "{}", s)
    }
}

impl std::fmt::Display for ProvisioningSubStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ProvisioningSubStatus::ProvisioningFailed => "ProvisioningFailed",
            ProvisioningSubStatus::ProvisioningInProgress => {
                "ProvisioningInProgress"
            }
        };
        write!(f, "{}", s)
    }
}

#[derive(Serialize)]
pub(crate) struct ReportableError {
    result: &'static str,
    reason: String,
    agent: String,
    documentation_url: &'static str,
    vm_id: String,
    timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pps_type: Option<String>,
    #[serde(flatten)]
    extra: HashMap<String, String>,
}

impl ReportableError {
    fn new(reason: impl Into<String>, vm_id: impl Into<String>) -> Self {
        ReportableError {
            result: "error",
            reason: reason.into(),
            agent: format!("Azure-Init/{}", env!("CARGO_PKG_VERSION")),
            documentation_url: "https://aka.ms/linuxprovisioningerror",
            vm_id: vm_id.into(),
            timestamp: Utc::now(),
            pps_type: Some("None".to_string()),
            extra: HashMap::new(),
        }
    }

    fn with_extra(mut self, key: &str, value: &str) -> Self {
        self.extra.insert(key.to_string(), value.to_string());
        self
    }
}

/// Report that provisioning is ready.
pub async fn report_ready(config: &Config) -> Result<(), Error> {
    tracing::info!("Reporting provisioning complete");
    _report(ProvisioningState::Ready, None, None, &config.wireserver).await
}

/// Report that provisioning has failed.
pub async fn report_failure(
    message: &str,
    config: &Config,
    vm_id: &str,
) -> Result<(), Error> {
    let error = ReportableError::new(message, vm_id.to_string())
        .with_extra("component", "provisioning");

    tracing::info!(
        reason = %error.reason,
        vm_id = %error.vm_id,
        "Reporting provisioning failure"
    );

    let body = serde_json::to_string(&error)?;

    _report(
        ProvisioningState::NotReady,
        Some(ProvisioningSubStatus::ProvisioningFailed),
        Some(&body),
        &config.wireserver,
    )
    .await
}

/// Report that provisioning is in progress.
pub async fn report_in_progress(
    message: &str,
    config: &Config,
) -> Result<(), Error> {
    _report(
        ProvisioningState::InProgress,
        Some(ProvisioningSubStatus::ProvisioningInProgress),
        Some(message),
        &config.wireserver,
    )
    .await
}

/// Internal helper: all of the header setup, JSON‐body construction, retry loop, etc.
#[instrument(err, skip_all)]
async fn _report(
    state: ProvisioningState,
    substatus: Option<ProvisioningSubStatus>,
    description: Option<&str>,
    cfg: &crate::config::Wireserver,
) -> Result<(), Error> {
    let body = if let Some(sub) = substatus {
        json!({
            "state": state.to_string(),
            "details": {
                "subStatus": sub.to_string(),
                "description": description
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
        HeaderValue::from_str(&format!("azure-init v{}", version)).unwrap(),
    );
    headers.insert(
        "x-ms-guest-agent-name",
        HeaderValue::from_str(&format!("azure-init/{}", version)).unwrap(),
    );
    headers
        .insert("content-type", HeaderValue::from_static("application/json"));

    tracing::info!(?headers, "Prepared HTTP headers");

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
            &cfg.health_endpoint,
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
                endpoint: cfg.health_endpoint.clone(),
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
    use serde_json::Value;
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
        let result = report_failure("oops", &cfg, &test_vm_id).await;

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
        let result = report_ready(&cfg).await;
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
        let result = report_failure("err", &cfg, &test_vm_id).await;

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
        let res = report_in_progress("Halfway there", &cfg).await;
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
        // shrink the wireserver timeouts so we fail immediately
        cfg.wireserver = crate::config::Wireserver {
            connection_timeout_secs: 0.01,
            read_timeout_secs: 0.01,
            total_retry_timeout_secs: 0.01,
            ..Default::default()
        };
        let test_vm_id = "00000000-0000-0000-0000-000000000000";

        // no override == real health_endpoint, which we can't reach in tests
        let r1 = report_ready(&cfg).await;
        let r2 = report_failure("no config", &cfg, &test_vm_id).await;
        assert!(
            r1.is_err(),
            "report_ready should fail against a dead server"
        );
        assert!(r2.is_err(), "report_failure should also fail");
    }

    #[test]
    fn test_reportable_error_formatting() {
        let vm_id = "00000000-0000-0000-0000-000000000000";
        let reason = "Test failure";
        let err = ReportableError::new(reason, vm_id.to_string())
            .with_extra("debug_info", "42");

        let json = serde_json::to_string_pretty(&err)
            .expect("should serialize to JSON");

        println!("Serialized ReportableError:\n{}", json);

        let parsed: Value =
            serde_json::from_str(&json).expect("should parse JSON");

        assert_eq!(parsed["result"], "error");
        assert_eq!(parsed["reason"], reason);
        assert_eq!(
            parsed["agent"].as_str().unwrap().starts_with("Azure-Init/"),
            true
        );
        assert_eq!(
            parsed["documentation_url"],
            "https://aka.ms/linuxprovisioningerror"
        );
        assert_eq!(parsed["vm_id"], vm_id);
        assert_eq!(parsed["pps_type"], "None");
        assert_eq!(parsed["debug_info"], "42");

        let timestamp: DateTime<Utc> = parsed["timestamp"]
            .as_str()
            .unwrap()
            .parse()
            .expect("timestamp should parse");
        assert!(timestamp <= Utc::now());
    }
}
