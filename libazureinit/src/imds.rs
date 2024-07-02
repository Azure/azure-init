// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::Client;

use serde::{Deserialize, Deserializer};
use serde_json;
use serde_json::Value;

use crate::error::Error;

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

pub async fn query(client: &Client) -> Result<InstanceMetadata, Error> {
    let url = "http://169.254.169.254/metadata/instance?api-version=2021-02-01";
    let mut headers = HeaderMap::new();

    headers.insert("Metadata", HeaderValue::from_static("true"));

    let request = client.get(url).headers(headers);
    let response = request.send().await?;

    if response.status().is_success() {
        let imds_body = response.text().await?;
        let metadata: InstanceMetadata = serde_json::from_str(&imds_body)?;

        Ok(metadata)
    } else {
        Err(Error::HttpStatus {
            endpoint: url.to_owned(),
            status: response.status(),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{InstanceMetadata, OsProfile};

    #[test]
    fn instance_metadata_deserialization() {
        let file_body = r#"
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
        }"#
        .to_string();

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
}
