// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::Client;
use tracing::instrument;

use std::time::Duration;

use serde::{Deserialize, Deserializer};
use serde_json;
use serde_json::Value;

use crate::config::Config;
use crate::error::Error;
use crate::http;

/// Azure instance metadata obtained from IMDS. Written in JSON format.
///
/// Required fields are osProfile and publicKeys.
///
/// # Example
///
/// ```
/// # use libazureinit::imds;
///    static TESTDATA: &str = r#"
///{
///  "compute": {
///    "osProfile": {
///      "adminUsername": "testuser",
///      "computerName": "testcomputer",
///      "disablePasswordAuthentication": "true"
///    },
///    "publicKeys": []
///  }
///}"#;
/// let metadata: imds::InstanceMetadata =
///     serde_json::from_str(&TESTDATA.to_string()).unwrap();
/// ```
#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct InstanceMetadata {
    /// Compute metadata
    pub compute: Compute,
}

/// Metadata about the instance's virtual machine. Written in JSON format.
#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct Compute {
    /// Metadata about the operating system.
    #[serde(rename = "osProfile")]
    pub os_profile: OsProfile,
    /// SSH Public keys.
    #[serde(rename = "publicKeys")]
    pub public_keys: Vec<PublicKeys>,
}

/// Azure Metadata about the virtual machine's operating system, obtained from IMDS.
/// Written in JSON format.
///
/// Required fields are adminUsername, computerName, disablePasswordAuthentication.
///
/// # Example
///
/// ```
/// # use serde_json::json;
/// # use libazureinit::imds::OsProfile;
///
/// let TESTDATA = json!({
///     "adminUsername": "testuser",
///     "computerName": "testcomputer",
///     "disablePasswordAuthentication": "true"
/// });
/// let os_profile: OsProfile = serde_json::from_value(TESTDATA).unwrap();
/// ```
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

/// Azure Metadata's SSH public key obtained from IMDS. Written in JSON format.
///
/// # Example
///
/// ```
/// # use serde_json::json;
/// # use libazureinit::imds::PublicKeys;
///
/// let TESTDATA = json!({
///     "keyData": "ssh-rsa test_key1",
///     "path": "/path/to/.ssh/authorized_keys"
/// });
/// let ssh_key: PublicKeys = serde_json::from_value(TESTDATA).unwrap();
/// ```
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
    "http://169.254.169.254/metadata/instance?api-version=2023-11-15&extended=true";

/// Send queries to IMDS to fetch Azure instance metadata.
///
/// Caller needs to pass 3 required parameters, client, retry_interval,
/// total_timeout. It is therefore required to create a reqwest::Client
/// variable with possible options, to pass it as parameter.
///
/// Parameter url optional. If None is passed, it defaults to
/// DEFAULT_IMDS_URL, an internal IMDS URL available in the Azure VM.
///
/// # Example
///
/// ```
/// # use reqwest::Client;
/// # use std::time::Duration;
/// # use libazureinit::config;
///
/// let client = Client::builder()
///     .timeout(std::time::Duration::from_secs(5))
///     .build()
///     .unwrap();
///
/// let config = config::Config::default();
/// let res = libazureinit::imds::query(
///     &client,
///     Some(&config),
///     Some("http://127.0.0.1:8000/"),
/// );
/// ```
#[instrument(err, skip_all)]
pub async fn query(
    client: &Client,
    config: Option<&Config>,
    url: Option<&str>,
) -> Result<InstanceMetadata, Error> {
    let imds = config.map(|c| c.imds.clone()).unwrap_or_default();
    let mut headers = HeaderMap::new();
    headers.insert("Metadata", HeaderValue::from_static("true"));
    let url = url.unwrap_or(DEFAULT_IMDS_URL);
    let request_timeout = Duration::from_secs_f64(imds.request_timeout_secs);
    let retry_interval = Duration::from_secs_f64(imds.retry_interval_secs);
    let mut total_timeout =
        Duration::from_secs_f64(imds.total_retry_timeout_secs);

    while !total_timeout.is_zero() {
        let (response, remaining_timeout) = http::get(
            client,
            headers.clone(),
            request_timeout,
            retry_interval,
            total_timeout,
            url,
        )
        .await?;
        match response.text().await {
            Ok(text) => {
                let metadata =
                    serde_json::from_str(text.as_str()).map_err(|error| {
                        tracing::warn!(
                            ?error,
                            "The response body was invalid and could not be deserialized"
                        );
                        error.into()
                    });
                if metadata.is_ok() {
                    return metadata;
                }
            }
            Err(error) => {
                tracing::warn!(?error, "Failed to read the full response body")
            }
        }

        total_timeout = remaining_timeout;
    }

    Err(Error::Timeout)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{query, InstanceMetadata, OsProfile};
    use crate::config;
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
        let mut config = config::Config::default();
        config.imds.total_retry_timeout_secs = 5.0;
        config.imds.request_timeout_secs = 5.0;
        config.imds.retry_interval_secs = 1.0;

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
                config.imds.request_timeout_secs as u64,
            ))
            .default_headers(default_headers)
            .build()
            .unwrap();

        let res = query(
            &client,
            Some(&config),
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

    // Assert malformed responses are retried.
    //
    // In this case the server declares a content-type of JSON, but doesn't return JSON.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn malformed_response() {
        let body = "not json, whoops";
        let payload = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
             StatusCode::OK.as_u16(),
             StatusCode::OK.to_string(),
             body.len(),
             body
        );

        let mut config = config::Config::default();
        config.imds.retry_interval_secs = 0.01;
        config.imds.total_retry_timeout_secs = 0.05;

        let serverlistener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = serverlistener.local_addr().unwrap();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let server = tokio::spawn(unittest::serve_requests(
            serverlistener,
            payload,
            cancel_token.clone(),
        ));

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();

        let res = query(
            &client,
            Some(&config),
            Some(format!("http://{:}:{:}/", addr.ip(), addr.port()).as_str()),
        )
        .await;

        cancel_token.cancel();

        let requests = server.await.unwrap();
        assert!(requests >= 2);
        assert!(logs_contain(
            "The response body was invalid and could not be deserialized"
        ));
        match res {
            Err(crate::error::Error::Timeout) => {}
            _ => panic!("Response should have timed out"),
        };
    }
}
