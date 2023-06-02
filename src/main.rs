use reqwest;
use reqwest::Client;
use reqwest::header::HeaderValue;
use reqwest::header::HeaderMap;

use serde::{Deserialize};
use serde_xml_rs::from_str;

use std::process::Command;

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


fn create_user(username: &str) {
    //check that useradd/echo/chpasswd exists before calling (like with FreeBSD)
    let _create_user = Command::new("useradd")
    .arg(username.to_string())
    .arg("--comment")
    .arg("Provisioning agent created this user based on username provided in IMDS")
    .arg("--groups")
    .arg("adm,audio,cdrom,dialout,dip,floppy,lxd,netdev,plugdev,sudo,video")
    .arg("-m")
    .output()
    .expect("Failed to execute useradd command.");

    let _set_password = Command::new("passwd")
    .arg("-d")
    .arg(username.to_string())
    .output()
    .expect("Failed to execute passwd command");

    create_ssh_directory(username);

    return;
}

fn create_ssh_directory(username: &str){
    let mut file_path = "/home/".to_string();
    file_path.push_str(username);
    file_path.push_str("/.ssh");

    let _create_ssh_directory = Command::new("mkdir")
    .arg(file_path)
    .output()
    .expect("Failed to execute mkdir command");

    // done last to ensure that everything in there is assigned to the user
    // chown -R username:<user> /home/username/.ssh

    return;
}

fn set_hostname(hostname: &str){
    let _set_hostname = Command::new("hostnamectl")
    .arg("set-hostname")
    .arg(hostname)
    .status()
    .expect("Failed to execute hostnamectl set-hostname");

    return;
}

async fn set_ssh_keys() -> Result<(), Box<dyn std::error::Error>>
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

    let public_key_start_index = match body.find("publicKeys") {
        Some(index) => index + "publicKeys".len() + 1,
        None => return Err(Box::from("Failed to find publicKeys")),
    };
    let public_key_end_index = match body[public_key_start_index..].find("]") {
        Some(index) => public_key_start_index + index,
        None => return Err(Box::from("Failed to find publicKeys")),
    };

    let key_text:&str = &body[public_key_start_index..public_key_end_index];

    let mut keys: Vec<String> = Vec::new();
    let mut start_index = 0;
    while let Some(key_index) = key_text[start_index..].find("ssh-rsa"){
        let key_index = start_index + key_index;
        let quote_index = match key_text[key_index..].find('"'){
            Some(index) => index + key_index,
            None => break,
        };
        let key = key_text[key_index..quote_index].to_owned();
        keys.push(key);
        start_index = quote_index;
    }

    println!("{:?}", keys);

    Ok(())
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

    create_user("test_user");

    set_hostname("cadetest-0003");

    set_ssh_keys().await;
}