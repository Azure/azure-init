use reqwest;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::Client;

use serde::Deserialize;
use serde_json;
use serde_json::Value;

use std::fs;
use std::fs::create_dir;
use std::fs::File;
use std::io::Write;

use nix::unistd::{Gid, Uid};
use std::ffi::CString;
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Deserialize, PartialEq)]
pub struct PublicKeys {
    #[serde(rename = "keyData")]
    key_data: String,
    #[serde(rename = "path")]
    path: String,
}

pub async fn create_ssh_directory(
    username: &str,
    home_path: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut file_path = home_path;
    file_path.push_str("/.ssh");

    create_dir(file_path.clone())?;

    set_ssh_keys(username.to_string(), file_path.clone()).await;

    let uid_username = CString::new(username.clone()).unwrap();
    let uid_passwd = unsafe { libc::getpwnam(uid_username.as_ptr()) };
    let uid = unsafe { (*uid_passwd).pw_uid };
    let new_uid = Uid::from_raw(uid);

    let gid_groupname = CString::new(username.clone()).unwrap();
    let gid_group = unsafe { libc::getgrnam(gid_groupname.as_ptr()) };
    let gid = unsafe { (*gid_group).gr_gid };
    let new_gid = Gid::from_raw(gid);

    let _set_ownership =
        nix::unistd::chown(file_path.as_str(), Some(new_uid), Some(new_gid));

    let metadata = fs::metadata(&file_path).unwrap();
    let permissions = metadata.permissions();
    let mut new_permissions = permissions.clone();
    new_permissions.set_mode(0o700);
    fs::set_permissions(&file_path, new_permissions).unwrap();

    Ok(())
}

pub async fn get_ssh_keys(
) -> Result<Vec<PublicKeys>, Box<dyn std::error::Error>> {
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

    let body = response.text().await?;

    let data: Value = serde_json::from_str(&body).unwrap();
    let content =
        Vec::<PublicKeys>::deserialize(&data["compute"]["publicKeys"]).unwrap();
    println!("{:?}", content);

    Ok(content)
}

pub async fn set_ssh_keys(username: String, file_path: String) {
    let keys = get_ssh_keys().await;
    match keys {
        Ok(keys) => {
            let mut authorized_keys_path = file_path;
            authorized_keys_path.push_str("/authorized_keys");
            let mut authorized_keys =
                File::create(authorized_keys_path.clone()).unwrap();
            for key in keys {
                writeln!(authorized_keys, "{}", key.key_data).unwrap();
            }
            let metadata = fs::metadata(&authorized_keys_path.clone()).unwrap();
            let permissions = metadata.permissions();
            let mut new_permissions = permissions.clone();
            new_permissions.set_mode(0o600);
            fs::set_permissions(&authorized_keys_path.clone(), new_permissions)
                .unwrap();

            let uid_username = CString::new(username.clone()).unwrap();
            let uid_passwd = unsafe { libc::getpwnam(uid_username.as_ptr()) };
            let uid = unsafe { (*uid_passwd).pw_uid };
            let new_uid = Uid::from_raw(uid);

            let gid_groupname = CString::new(username.clone()).unwrap();
            let gid_group = unsafe { libc::getgrnam(gid_groupname.as_ptr()) };
            let gid = unsafe { (*gid_group).gr_gid };
            let new_gid = Gid::from_raw(gid);

            let _set_ownership = nix::unistd::chown(
                authorized_keys_path.as_str(),
                Some(new_uid),
                Some(new_gid),
            );

            return;
        }
        Err(_error) => {
            // handle the error
            return;
        }
    }
}
