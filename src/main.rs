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


async fn get_goalstate_rest() -> Result<Goalstate, Box<dyn std::error::Error>>
{
    let url = "http://168.63.129.16/machine/?comp=goalstate";

    let client = Client::new();

    let mut headers = HeaderMap::new();

    headers.insert("x-ms-agent-name", HeaderValue::from_static("azure-provisioning-agent"));
    headers.insert("x-ms-version", HeaderValue::from_static("2012-11-30"));

    let request = client.get(url).headers(headers);
    let response = request.send().await?;

    if !response.status().is_success() {
        println!("Request failed with status code: {}", response.status());
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

    println!("{}", post_request);

    let response = client.post(url)
    .headers(headers)
    .body(post_request)
    .send()
    .await?;

    println!("{:?}", response);

    let response_text = response.text().await?;
    println!("");
    println!("{}", response_text);


    Ok(())
}

#[tokio::main]
async fn main() {
    let rest_call = get_goalstate_rest().await;
    
    if let Err(ref err) = rest_call {
        eprintln!("Get API call failed: {}", err);
    }

    let goalstate: Goalstate = rest_call.unwrap();

    println!("Get request completed successfully!");
    println!("");

    let post_call = post_goalstate(goalstate).await; //this condition will never be met
    if let Err(ref err) = post_call {
        eprintln!("Post API call failed: {}", err);
    }

    create_user("tom".to_string());
}

fn create_user(username: String) {
    let create_user = Command::new("useradd")
    .arg(username)
    .output()
    .expect("Failed to execute useradd command.");
}

fn set_password(password: String)
{
    let set_password = Command::new("passwd")
    .arg(password)
    .output()
    .expect("Failed to execute passwd command.");
}