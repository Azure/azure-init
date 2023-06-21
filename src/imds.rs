use reqwest;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::Client;

use serde::Deserialize;
use serde_json;
use serde_json::Value;

#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct PublicKeys {
    #[serde(rename = "keyData")]
    pub key_data: String,
    #[serde(rename = "path")]
    pub path: String,
}

pub async fn query_imds() -> Result<String, Box<dyn std::error::Error>> {
    let url = "http://169.254.169.254/metadata/instance?api-version=2021-02-01";
    let client = Client::new();
    let mut headers = HeaderMap::new();

    headers.insert("Metadata", HeaderValue::from_static("true"));

    let request = client.get(url).headers(headers);
    let response = request.send().await?;

    if response.status().is_success() {
        let imds_body = response.text().await?;

        return Ok(imds_body);
    }

    println!(
        "Get IMDS request failed with status code: {}",
        response.status()
    );
    println!("{:?}", response.text().await);

    return Err(Box::from("Failed Get Call"));
}

pub fn get_ssh_keys(
    imds_body: String,
) -> Result<Vec<PublicKeys>, Box<dyn std::error::Error>> {
    let data: Value = serde_json::from_str(&imds_body)
        .expect("Failed to parse the IMDS JSON.");
    let public_keys =
        Vec::<PublicKeys>::deserialize(&data["compute"]["publicKeys"])
            .expect("Failed to deserialize the public ssh keys.");

    Ok(public_keys)
}

pub fn get_username(
    imds_body: String,
) -> Result<String, Box<dyn std::error::Error>> {
    let data: Value = serde_json::from_str(&imds_body)
        .expect("Failed to parse the IMDS JSON.");
    let username =
        String::deserialize(&data["compute"]["osProfile"]["adminUsername"])
            .expect("Failed to deserialize the admin username.");

    Ok(username)
}

pub fn get_hostname(
    imds_body: String,
) -> Result<String, Box<dyn std::error::Error>> {
    let data: Value = serde_json::from_str(&imds_body)
        .expect("Failed to parse the IMDS JSON.");
    let hostname =
        String::deserialize(&data["compute"]["osProfile"]["computerName"])
            .expect("Failed to deserialize the hostname.");

    Ok(hostname)
}

#[cfg(test)]
mod tests {
    use super::{get_hostname, get_ssh_keys, get_username};

    #[test]
    fn test_get_ssh_keys() {
        let file_body = r#"
        {
            "compute": {
              "azEnvironment": "AzurePublicCloud",
              "customData": "",
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

        let public_keys = get_ssh_keys(file_body)
            .expect("Failed to obtain ssh keys from the JSON file.");

        assert_eq!(public_keys[0].key_data, "ssh-rsa test_key1".to_string());
        assert_eq!(public_keys[1].key_data, "ssh-rsa test_key2".to_string());
    }

    #[test]
    fn test_get_username() {
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
              }
            }
        }"#
        .to_string();

        let username =
            get_username(file_body).expect("Failed to get username.");

        assert_eq!(username, "MinProvAgentUser".to_string());
    }

    #[test]
    fn test_get_hostname() {
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
              }
            }
        }"#
        .to_string();

        let hostname =
            get_hostname(file_body).expect("Failed to get hostname.");

        assert_eq!(hostname, "AzTux-MinProvAgent-Test-0001".to_string());
    }
}
