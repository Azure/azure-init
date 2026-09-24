//! Azure wireserver provisioning-health transport.
//!
//! These functions only send HTTP reports. They neither publish KVP records nor
//! inspect telemetry configuration. Callers control KVP delivery independently
//! using the same [`ProvisioningReport`] when reporting a failure.

use std::time::Duration;

use libazureinit_kvp::ProvisioningReport;
use reqwest::{
    header::{HeaderMap, HeaderValue, USER_AGENT},
    Client, StatusCode,
};
use serde_json::json;
use tracing::instrument;

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
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Ready => "Ready",
            Self::NotReady => "NotReady",
        })
    }
}

impl std::fmt::Display for ProvisioningSubStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ProvisioningFailed => "ProvisioningFailed",
            Self::Provisioning => "Provisioning",
        })
    }
}

/// Reports successful provisioning to the wireserver.
///
/// The request contains only the ready state, not an encoded KVP report.
#[instrument(err, skip_all)]
pub async fn report_ready(config: &Wireserver) -> Result<(), Error> {
    tracing::info!("Reporting provisioning complete");
    report(ProvisioningState::Ready, None, None, config).await
}

/// Reports provisioning failure using the supplied failure report.
///
/// Its encoded description preserves the caller's report timestamp and context.
pub async fn report_failure(
    failure: &ProvisioningReport,
    config: &Wireserver,
) -> Result<(), Error> {
    report(
        ProvisioningState::NotReady,
        Some(ProvisioningSubStatus::ProvisioningFailed),
        Some(failure.encode()),
        config,
    )
    .await
}

/// Reports that provisioning is still in progress, without publishing a result.
pub async fn report_in_progress(
    config: &Wireserver,
    vm_id: &str,
) -> Result<(), Error> {
    report(
        ProvisioningState::NotReady,
        Some(ProvisioningSubStatus::Provisioning),
        Some(format!(
            "Provisioning is still in progress for vm_id={vm_id}."
        )),
        config,
    )
    .await
}

#[instrument(err, skip_all, fields(state = %state))]
async fn report(
    state: ProvisioningState,
    substatus: Option<ProvisioningSubStatus>,
    description: Option<String>,
    config: &Wireserver,
) -> Result<(), Error> {
    tracing::info!("Initiating health report to wireserver: {}", state);

    if let Some(description) = &description {
        tracing::debug!(%description, "Provisioning report");
    }

    let body = if let Some(substatus) = substatus {
        json!({
            "state": state.to_string(),
            "details": {
                "subStatus": substatus.to_string(),
                "description": description.unwrap_or_default(),
            }
        })
        .to_string()
    } else {
        json!({ "state": state.to_string() }).to_string()
    };

    tracing::debug!(body = %body, "Built provisioning-health JSON");

    let version = env!("CARGO_PKG_VERSION");
    let agent =
        HeaderValue::from_str(&format!("azure-init v{version}")).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, agent.clone());
    headers.insert("x-ms-guest-agent-name", agent);
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
        let (response, new_remaining) = http::post(
            &client,
            headers.clone(),
            body.clone(),
            read_timeout,
            connect_timeout,
            remaining,
            &config.health_endpoint,
        )
        .await?;

        let status = response.status();
        for (key, value) in response.headers() {
            tracing::debug!(header = %key, value = ?value, "Wireserver response header");
        }
        tracing::info!("Wireserver replied with status {}", status);

        if status.is_success() {
            tracing::info!("Report '{}' succeeded", state);
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
    use reqwest::header::HeaderName;
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_util::sync::CancellationToken;

    const VM_ID: &str = "00000000-0000-0000-0000-000000000001";

    fn fast_config(endpoint: String) -> Wireserver {
        Wireserver {
            connection_timeout_secs: 0.01,
            read_timeout_secs: 0.1,
            total_retry_timeout_secs: 0.2,
            health_endpoint: endpoint,
        }
    }

    struct CapturedRequest {
        method: String,
        path: String,
        headers: HeaderMap,
        body: Value,
    }

    async fn capture_request(
        listener: &TcpListener,
    ) -> Result<CapturedRequest, Box<dyn std::error::Error>> {
        let (mut stream, _) = listener.accept().await?;
        let mut bytes = Vec::new();
        let (body_start, method, path, headers) = loop {
            if stream.read_buf(&mut bytes).await? == 0 {
                return Err("connection closed before request headers".into());
            }
            let mut parsed_headers = [httparse::EMPTY_HEADER; 16];
            let mut request = httparse::Request::new(&mut parsed_headers);
            if let httparse::Status::Complete(offset) = request.parse(&bytes)? {
                let mut headers = HeaderMap::new();
                for header in request.headers {
                    headers.insert(
                        HeaderName::from_bytes(header.name.as_bytes())?,
                        HeaderValue::from_bytes(header.value)?,
                    );
                }
                break (
                    offset,
                    request.method.ok_or("missing method")?.to_owned(),
                    request.path.ok_or("missing path")?.to_owned(),
                    headers,
                );
            }
        };
        let content_length: usize = headers
            .get("content-length")
            .ok_or("missing content length")?
            .to_str()?
            .parse()?;
        while bytes.len() - body_start < content_length {
            if stream.read_buf(&mut bytes).await? == 0 {
                return Err("connection closed before request body".into());
            }
        }
        let body = serde_json::from_slice(
            &bytes[body_start..body_start + content_length],
        )?;
        stream
            .write_all(
                get_http_response_payload(&StatusCode::CREATED, "").as_bytes(),
            )
            .await?;
        Ok(CapturedRequest {
            method,
            path,
            headers,
            body,
        })
    }

    #[tokio::test]
    async fn reports_preserve_http_contract(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let config = fast_config(format!(
            "http://{}/provisioning/health",
            listener.local_addr()?
        ));
        let failure = Error::LoadSshdConfig {
            details: "bad | \"quoted\"\nconfig".to_owned(),
        }
        .as_provisioning_report(VM_ID);
        let send = async {
            report_ready(&config).await?;
            report_failure(&failure, &config).await?;
            report_in_progress(&config, VM_ID).await?;
            Ok::<_, Error>(())
        };
        let receive = async {
            let mut requests = Vec::new();
            for _ in 0..3 {
                requests.push(capture_request(&listener).await?);
            }
            Ok::<_, Box<dyn std::error::Error>>(requests)
        };
        let (sent, received) =
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(send, receive)
            })
            .await?;
        sent?;
        let requests = received?;
        for request in &requests {
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, "/provisioning/health");
            let agent = format!("azure-init v{}", env!("CARGO_PKG_VERSION"));
            assert_eq!(request.headers[USER_AGENT], agent);
            assert_eq!(request.headers["x-ms-guest-agent-name"], agent);
            assert_eq!(request.headers["content-type"], "application/json");
        }
        assert_eq!(requests[0].body, json!({"state": "Ready"}));
        assert_eq!(
            requests[1].body,
            json!({
                "state": "NotReady",
                "details": {
                    "subStatus": "ProvisioningFailed",
                    "description": failure.encode(),
                },
            })
        );
        let description = requests[1].body["details"]["description"]
            .as_str()
            .ok_or("missing failure description")?;
        assert_eq!(description.parse::<ProvisioningReport>()?, failure);
        assert_eq!(
            requests[2].body,
            json!({
                "state": "NotReady",
                "details": {
                    "subStatus": "Provisioning",
                    "description": format!("Provisioning is still in progress for vm_id={VM_ID}."),
                },
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_report_all_retryable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config =
            fast_config(format!("http://{}", listener.local_addr().unwrap()));
        let cancel = CancellationToken::new();
        let server = tokio::spawn(serve_requests(
            listener,
            get_http_response_payload(&StatusCode::SERVICE_UNAVAILABLE, ""),
            cancel.clone(),
        ));
        let failure = Error::Timeout.as_provisioning_report(VM_ID);
        let result = report_failure(&failure, &config).await;
        cancel.cancel();
        let requests = server.await.unwrap();
        assert!(matches!(result, Err(Error::Timeout)));
        assert!(requests > 1);
    }

    #[tokio::test]
    async fn test_report_unexpected_code() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config =
            fast_config(format!("http://{}", listener.local_addr().unwrap()));
        let cancel = CancellationToken::new();
        let server = tokio::spawn(serve_requests(
            listener,
            get_http_response_payload(&StatusCode::FORBIDDEN, ""),
            cancel.clone(),
        ));
        let failure = Error::Timeout.as_provisioning_report(VM_ID);
        let result = report_failure(&failure, &config).await;
        cancel.cancel();
        let requests = server.await.unwrap();
        assert!(matches!(result, Err(Error::Http(error))
            if error.status() == Some(StatusCode::FORBIDDEN)));
        assert_eq!(requests, 1);
    }

    #[tokio::test]
    async fn public_reports_fail_on_unresponsive_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config =
            fast_config(format!("http://{}", listener.local_addr().unwrap()));
        let failure = Error::Timeout.as_provisioning_report(VM_ID);
        assert!(report_ready(&config).await.is_err());
        assert!(report_failure(&failure, &config).await.is_err());
    }
}
