use reqwest;
use reqwest::Client;
use reqwest::header::HeaderValue;
use reqwest::header::HeaderMap;

use serde::{Deserialize};
use serde_xml_rs::from_str;
use serde_json;

use std::process::Command;
use std::fs::File;
use std::io::Write;

//////////////////////////////////
//          XML STRUCTS
//////////////////////////////////

#[derive(Debug, Deserialize, PartialEq)]
struct Goalstate {
    #[serde(rename = "Container")]
    container: Container,
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "Incarnation")]
    incarnation: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Container {
    #[serde(rename = "ContainerId")]
    container_id: String,
    #[serde(rename = "RoleInstanceList")]
    role_instance_list: RoleInstanceList,
}

#[derive(Debug, Deserialize, PartialEq)]
struct RoleInstanceList {
    #[serde(rename = "RoleInstance")]
    role_instance: RoleInstance,
}

#[derive(Debug, Deserialize, PartialEq)]
struct RoleInstance {
    #[serde(rename = "InstanceId")]
    instance_id: String,
}

//////////////////////////////////
//          JSON STRUCTS
//////////////////////////////////

#[derive(Debug, Deserialize, PartialEq)]
struct Data {
    #[serde(rename = "compute")]
    compute: Compute
}

#[derive(Debug, Deserialize, PartialEq)]
struct Compute {
    #[serde(rename = "publicKeys")]
    public_keys: Vec<PublicKeys>
}

#[derive(Debug, Deserialize, PartialEq)]
struct PublicKeys {
    #[serde(rename = "keyData")]
    data: String,
    #[serde(rename = "path")]
    path: String,
}

async fn get_goalstate() -> Result<Goalstate, Box<dyn std::error::Error>>
{
    let url = "http://168.63.129.16/machine/?comp=goalstate";

    let client = Client::new();

    let mut headers = HeaderMap::new();

    headers.insert("x-ms-agent-name", HeaderValue::from_static("azure-provisioning-agent"));
    headers.insert("x-ms-version", HeaderValue::from_static("2012-11-30"));

    let request = client.get(url).headers(headers);
    let response = request.send().await?;

    if !response.status().is_success() {
        println!("Get request failed with status code: {}", response.status());
        return Err(Box::from("Failed Get Call"));
    }

    let body = response.text().await?;

    let goalstate: Goalstate = from_str(&body)?;
    Ok(goalstate)
}


async fn post_goalstate(goalstate: Goalstate) -> Result<(), Box<dyn std::error::Error>> {
    let url = "http://168.63.129.16/machine/?comp=health";

    let client = Client::new();

    let mut headers = HeaderMap::new();

    headers.insert("x-ms-agent-name", HeaderValue::from_static("azure-provisioning-agent"));
    headers.insert("x-ms-version", HeaderValue::from_static("2012-11-30"));
    headers.insert("Content-Type", HeaderValue::from_static("text/xml;charset=utf-8"));

    let post_request = 
    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
    <Health xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\">\n\
    <GoalStateIncarnation>$GOAL_STATE_INCARNATION</GoalStateIncarnation>\n\
    <Container>\n\
    <ContainerId>$CONTAINER_ID</ContainerId>\n\
    <RoleInstanceList>\n\
    <Role>\n\
    <InstanceId>$INSTANCE_ID</InstanceId>\n\
    <Health>\n\
    <State>Ready</State>\n\
    </Health>\n\
    </Role>\n\
    </RoleInstanceList>\n\
    </Container>\n\
    </Health>";

    let post_request = post_request.replace("$GOAL_STATE_INCARNATION", &goalstate.incarnation);
    let post_request = post_request.replace("$CONTAINER_ID", &goalstate.container.container_id);
    let post_request = post_request.replace("$INSTANCE_ID", &goalstate.container.role_instance_list.role_instance.instance_id);

    let response = client.post(url)
    .headers(headers)
    .body(post_request)
    .send()
    .await?;

    if !response.status().is_success() {
        println!("Post request failed with status code: {}", response.status());
        return Err(Box::from("Failed Post Call"));
    }

    Ok(())
}


async fn create_user(username: &str) {
    let mut home_path = "/home/".to_string();
    home_path.push_str(username);

    let _create_user = Command::new("useradd")
    .arg(username.to_string())
    .arg("--comment")
    .arg("Provisioning agent created this user based on username provided in IMDS")
    .arg("--groups")
    .arg("adm,audio,cdrom,dialout,dip,floppy,lxd,netdev,plugdev,sudo,video")
    .arg("-d")
    .arg(home_path.clone())
    .arg("-m")
    .status()
    .expect("Failed to execute useradd command.");

    let _set_password = Command::new("passwd")
    .arg("-d")
    .arg(username.to_string())
    .output()
    .expect("Failed to execute passwd command");

    create_ssh_directory(username, home_path).await;

    return;
}

async fn create_ssh_directory(username: &str, home_path: String){
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

    let _transfer_ssh_o = Command::new("chgrp")
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

    return;
}

async fn get_ssh_keys() -> Result<Vec<PublicKeys>, Box<dyn std::error::Error>>
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

    let data: Data = serde_json::from_str(&body).unwrap(); //could be cleaned if this works

    Ok(data.compute.public_keys)
}

async fn set_ssh_keys(file_path: String){
    let keys = get_ssh_keys().await;
    match keys {
        Ok(keys) => {
            let mut authorized_keys_path = file_path;
            authorized_keys_path.push_str("/authorized_keys");
            let mut authorized_keys = File::create(authorized_keys_path).unwrap();
            for key in keys{
                writeln!(authorized_keys, "{}", key.data).unwrap();
            }
            return;
        },
        Err(error) => {
            // handle the error
            return;
        }
    }
}


fn set_hostname(hostname: &str){
    let _set_hostname = Command::new("hostnamectl")
    .arg("set-hostname")
    .arg(hostname)
    .status()
    .expect("Failed to execute hostnamectl set-hostname");

    return;
}



#[tokio::main]
async fn main() {
    let rest_call = get_goalstate().await;
    
    if let Err(ref _err) = rest_call {
        return;
    }

    let goalstate: Goalstate = rest_call.unwrap();

    let post_call = post_goalstate(goalstate).await;
    if let Err(ref _err) = post_call {
        return;
    }

    create_user("test_user").await;  //add to deserializer

    set_hostname("cadetest-0003");  //this should be done elsewhere
}