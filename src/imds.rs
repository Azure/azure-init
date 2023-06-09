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

pub async fn query_imds(
) -> Result<String, Box<dyn std::error::Error>> {
    let url = "http://169.254.169.254/metadata/instance?api-version=2021-02-01";
    let client = Client::new();
    let mut headers = HeaderMap::new();

    headers.insert("Metadata", HeaderValue::from_static("true"));

    let request = client.get(url).headers(headers);
    let response = request.send().await?;

    if !response.status().is_success() {
        println!(
            "Get IMDS request failed with status code: {}",
            response.status()
        );
        println!("{:?}", response.text().await);
        return Err(Box::from("Failed Get Call"));
    }

    let imds_body = response.text().await?;

    Ok(imds_body)
}


pub fn get_ssh_keys(imds_body: String
) -> Result<Vec<PublicKeys>, Box<dyn std::error::Error>> {
    let data: Value = serde_json::from_str(&imds_body).unwrap();
    let public_keys =
        Vec::<PublicKeys>::deserialize(&data["compute"]["publicKeys"]).unwrap();

    Ok(public_keys)
}

pub fn get_username(imds_body: String
) -> Result<String, Box<dyn std::error::Error>> {
    let data: Value = serde_json::from_str(&imds_body).unwrap();
    let username =
        String::deserialize(&data["compute"]["osProfile"]["adminUsername"]).unwrap();

    Ok(username)
}
