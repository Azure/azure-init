use reqwest;
use reqwest::Client;
use reqwest::header::HeaderValue;
use reqwest::header::HeaderMap;

use serde::{Deserialize};
use serde_json;
use serde_json::Value;

use std::process::Command;
use std::fs::File;
use std::io::Write;

#[derive(Debug, Deserialize, PartialEq)]
pub struct PublicKeys {
    #[serde(rename = "keyData")]
    key_data: String,
    #[serde(rename = "path")]
    path: String,
}

pub async fn create_ssh_directory(username: &str, home_path: String){
    let mut file_path = home_path;
    file_path.push_str("/.ssh");

    let _create_ssh_directory = Command::new("mkdir")
    .arg(file_path.clone())
    .output()
    .expect("Failed to execute mkdir command");

    set_ssh_keys(file_path.clone()).await;

    let _transfer_ssh_ownership = Command::new("chown")
    .arg("-hR")
    .arg(username)
    .arg(file_path.clone())
    .output()
    .expect("Failed to execute chown command");

    let _transfer_ssh_group = Command::new("chgrp")
    .arg(username)
    .arg(file_path.clone())
    .output()
    .expect("Failed to execute chgrp command");

    let _set_permissions_value = Command::new("chmod")
    .arg("-R")
    .arg("700")                     // 600 does not allow me to access the folder even as the owner
    .arg(file_path.clone())
    .output()
    .expect("Failed to execute chmod command");
}

pub async fn get_ssh_keys() -> Result<Vec<PublicKeys>, Box<dyn std::error::Error>>
{
    let url = "http://169.254.169.254/metadata/instance?api-version=2021-02-01";
    let client = Client::new();
    let mut headers = HeaderMap::new();

    headers.insert("Metadata", HeaderValue::from_static("true"));

    let request = client.get(url).headers(headers);
    let response = request.send().await?;

    if !response.status().is_success() {
        println!("Get IMDS request failed with status code: {}", response.status());
        println!("{:?}", response.text().await);
        return Err(Box::from("Failed Get Call"));
    }

    let body = response.text().await?;

    let data: Value = serde_json::from_str(&body).unwrap();
    let content = Vec::<PublicKeys>::deserialize(&data["compute"]["publicKeys"]).unwrap();
    println!("{:?}", content);

    Ok(content)
}

pub async fn set_ssh_keys(file_path: String){
    let keys = get_ssh_keys().await;
    match keys {
        Ok(keys) => {
            let mut authorized_keys_path = file_path;
            authorized_keys_path.push_str("/authorized_keys");
            let mut authorized_keys = File::create(authorized_keys_path.clone()).unwrap();
            for key in keys{
                writeln!(authorized_keys, "{}", key.key_data).unwrap();
            }
            let _set_permissions_value = Command::new("chmod")
            .arg("600")
            .arg(authorized_keys_path.clone())
            .spawn()
            .expect("Failed to execute chmod command");
            return;
        },
        Err(_error) => {
            // handle the error
            return;
        }
    }
}