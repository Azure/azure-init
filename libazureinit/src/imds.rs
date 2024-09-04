// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::{Client, StatusCode};

use std::time::Duration;

use serde::{Deserialize, Deserializer};
use serde_json;
use serde_json::Value;

use tokio::time::timeout;

use crate::error::Error;
use crate::http;

#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct InstanceMetadata {
    /// Compute metadata
    pub compute: Compute,
}

/// Metadata about the instance's virtual machine.
#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct Compute {
    /// Metadata about the operating system.
    #[serde(rename = "osProfile")]
    pub os_profile: OsProfile,
    /// SSH Public keys.
    #[serde(rename = "publicKeys")]
    pub public_keys: Vec<PublicKeys>,
}

/// Metadata about the virtual machine's operating system.
#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct OsProfile {
    /// The admin account's username.
    #[serde(rename = "adminUsername")]
    pub admin_username: String,
    /// The name of the virtual machine.
    #[serde(rename = "computerName")]
    pub computer_name: String,
    /// Specifies whether or not password authentication is disabled.
    #[serde(
        rename = "disablePasswordAuthentication",
        deserialize_with = "string_bool"
    )]
    pub disable_password_authentication: bool,
}

/// An SSH public key.
#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct PublicKeys {
    /// The SSH public key certificate used to authenticate with the virtual machine.
    #[serde(rename = "keyData")]
    pub key_data: String,
    /// The full path on the virtual machine where the SSH public key is stored.
    #[serde(rename = "path")]
    pub path: String,
}

impl From<&str> for PublicKeys {
    fn from(value: &str) -> Self {
        Self {
            key_data: value.to_string(),
            path: String::new(),
        }
    }
}

/// Deserializer that handles the string "true" and "false" that the IMDS API returns.
fn string_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match Deserialize::deserialize(deserializer)? {
        Value::String(string) => match string.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            unknown => Err(serde::de::Error::unknown_variant(
                unknown,
                &["true", "false"],
            )),
        },
        Value::Bool(boolean) => Ok(boolean),
        _ => Err(serde::de::Error::custom(
            "Unexpected type, expected 'true' or 'false'",
        )),
    }
}

const DEFAULT_IMDS_URL: &str =
    "http://169.254.169.254/metadata/instance?api-version=2021-02-01";

pub async fn query(
    client: &Client,
    retry_interval: Duration,
    total_timeout: Duration,
    url: Option<&str>,
) -> Result<InstanceMetadata, Error> {
    let mut headers = HeaderMap::new();
    let url = url.unwrap_or(DEFAULT_IMDS_URL);

    headers.insert("Metadata", HeaderValue::from_static("true"));

    let response = timeout(total_timeout, async {
        let now = std::time::Instant::now();
        loop {
            if let Ok(response) = client
                .get(url)
                .headers(headers.clone())
                .timeout(Duration::from_secs(http::IMDS_HTTP_TIMEOUT_SEC))
                .send()
                .await
            {
                let statuscode = response.status();

                if statuscode == StatusCode::OK {
                    tracing::info!("HTTP response succeeded with status {}", statuscode);
                    return Ok(response);
                }

                if !http::RETRY_CODES.contains(&statuscode) {
                    return response.error_for_status().map_err(|error| {
                        tracing::error!(?error, "{}", format!("HTTP call failed due to status {}", statuscode));
                        error
                    });
                }
            }

            tracing::info!("Retrying to get HTTP response in {} sec, remaining timeout {} sec.", retry_interval.as_secs(), total_timeout.saturating_sub(now.elapsed()).as_secs());

            tokio::time::sleep(retry_interval).await;
        }
    })
    .await?;

    let imds_body = response?.text().await?;

    let metadata: InstanceMetadata = serde_json::from_str(&imds_body)?;

    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{query, InstanceMetadata, OsProfile};

    use reqwest::{header, Client, StatusCode};
    use std::time::Duration;
    use tokio::net::TcpListener;

    use crate::{http, unittest};

    static BODY_CONTENTS: &str = r#"
{
  "compute": {
    "azEnvironment": "cloud_env",
    "customData": "",
    "evictionPolicy": "",
    "isHostCompatibilityLayerVm": "false",
    "licenseType": "",
    "location": "eastus",
    "name": "AzTux-MinProvAgent-Test-0001",
    "offer": "0001-com-ubuntu-server-focal",
    "osProfile": {
      "adminUsername": "MinProvAgentUser",
      "computerName": "AzTux-MinProvAgent-Test-0001",
      "disablePasswordAuthentication": "true"
    },
    "publicKeys": [
      {
        "keyData": "ssh-rsa test_key1",
        "path": "/path/to/.ssh/authorized_keys"
      },
      {
        "keyData": "ssh-rsa test_key2",
        "path": "/path/to/.ssh/authorized_keys"
      }
    ]
  }
}"#;

    #[test]
    fn instance_metadata_deserialization() {
        let file_body = BODY_CONTENTS.to_string();

        let metadata: InstanceMetadata =
            serde_json::from_str(&file_body).unwrap();

        assert!(metadata.compute.os_profile.disable_password_authentication);
        assert_eq!(
            metadata.compute.public_keys[0].key_data,
            "ssh-rsa test_key1".to_string()
        );
        assert_eq!(
            metadata.compute.public_keys[1].key_data,
            "ssh-rsa test_key2".to_string()
        );
        assert_eq!(
            metadata.compute.os_profile.admin_username,
            "MinProvAgentUser".to_string()
        );
        assert_eq!(
            metadata.compute.os_profile.computer_name,
            "AzTux-MinProvAgent-Test-0001".to_string()
        );
        assert_eq!(
            metadata.compute.os_profile.disable_password_authentication,
            true
        );
    }

    #[test]
    fn deserialization_disable_password_true() {
        let os_profile = json!({
            "adminUsername": "MinProvAgentUser",
            "computerName": "AzTux-MinProvAgent-Test-0001",
            "disablePasswordAuthentication": "true"
        });
        let os_profile: OsProfile = serde_json::from_value(os_profile).unwrap();
        assert!(os_profile.disable_password_authentication);
    }

    #[test]
    fn deserialization_disable_password_false() {
        let os_profile = json!({
            "adminUsername": "MinProvAgentUser",
            "computerName": "AzTux-MinProvAgent-Test-0001",
            "disablePasswordAuthentication": "false"
        });
        let os_profile: OsProfile = serde_json::from_value(os_profile).unwrap();
        assert_eq!(os_profile.disable_password_authentication, false);
    }

    #[test]
    fn deserialization_disable_password_nonsense() {
        let os_profile = json!({
            "adminUsername": "MinProvAgentUser",
            "computerName": "AzTux-MinProvAgent-Test-0001",
            "disablePasswordAuthentication": "nonsense"
        });
        let os_profile: Result<OsProfile, _> =
            serde_json::from_value(os_profile);
        assert!(os_profile.is_err_and(|err| err.is_data()));
    }

    // Runs a test around sending via imds::query() with a given statuscode.
    async fn run_imds_query_retry(statuscode: &StatusCode) -> bool {
        const IMDS_HTTP_TOTAL_TIMEOUT_SEC: u64 = 5;
        const IMDS_HTTP_PERCLIENT_TIMEOUT_SEC: u64 = 5;
        const IMDS_HTTP_RETRY_INTERVAL_SEC: u64 = 1;

        let mut default_headers = header::HeaderMap::new();
        let user_agent =
            header::HeaderValue::from_str("azure-init test").unwrap();

        let ok_payload =
            unittest::get_http_response_payload(statuscode, BODY_CONTENTS);
        let serverlistener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = serverlistener.local_addr().unwrap();

        let cancel_token = tokio_util::sync::CancellationToken::new();

        let server = tokio::spawn(unittest::serve_requests(
            serverlistener,
            ok_payload,
            cancel_token.clone(),
        ));

        default_headers.insert(header::USER_AGENT, user_agent);
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(
                IMDS_HTTP_PERCLIENT_TIMEOUT_SEC,
            ))
            .default_headers(default_headers)
            .build()
            .unwrap();

        let res = query(
            &client,
            Duration::from_secs(IMDS_HTTP_RETRY_INTERVAL_SEC),
            Duration::from_secs(IMDS_HTTP_TOTAL_TIMEOUT_SEC),
            Some(format!("http://{:}:{:}/", addr.ip(), addr.port()).as_str()),
        )
        .await;

        cancel_token.cancel();

        let requests = server.await.unwrap();

        if http::HARDFAIL_CODES.contains(statuscode) {
            assert_eq!(requests, 1);
        }

        if http::RETRY_CODES.contains(statuscode) {
            assert!(requests >= 4);
        }

        res.is_ok()
    }

    #[tokio::test]
    async fn imds_query_retry() {
        // status codes that should succeed.
        assert!(run_imds_query_retry(&StatusCode::OK).await);

        // status codes that should be retried up to 5 minutes.
        for rc in http::RETRY_CODES {
            assert!(!run_imds_query_retry(rc).await);
        }

        // status codes that should result into immediate failures.
        for rc in http::HARDFAIL_CODES {
            assert!(!run_imds_query_retry(rc).await);
        }
    }
}
