// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::Client;

use serde::Deserialize;
use serde_xml_rs::from_str;

use crate::error::Error;

#[derive(Debug, Deserialize, PartialEq)]
pub struct Goalstate {
    #[serde(rename = "Container")]
    container: Container,
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "Incarnation")]
    incarnation: String,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct Container {
    #[serde(rename = "ContainerId")]
    container_id: String,
    #[serde(rename = "RoleInstanceList")]
    role_instance_list: RoleInstanceList,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct RoleInstanceList {
    #[serde(rename = "RoleInstance")]
    role_instance: RoleInstance,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct RoleInstance {
    #[serde(rename = "InstanceId")]
    instance_id: String,
}

pub async fn get_goalstate(client: &Client) -> Result<Goalstate, Error> {
    let url = "http://168.63.129.16/machine/?comp=goalstate";

    let mut headers = HeaderMap::new();
    headers.insert("x-ms-agent-name", HeaderValue::from_static("azure-init"));
    headers.insert("x-ms-version", HeaderValue::from_static("2012-11-30"));

    let request = client.get(url).headers(headers);
    let response = request.send().await?;

    if response.status().is_success() {
        let body = response.text().await?;

        let goalstate: Goalstate = from_str(&body)?;
        Ok(goalstate)
    } else {
        Err(Error::HttpStatus {
            endpoint: url.to_owned(),
            status: response.status(),
        })
    }
}

pub async fn report_health(
    client: &Client,
    goalstate: Goalstate,
) -> Result<(), Error> {
    let url = "http://168.63.129.16/machine/?comp=health";

    let mut headers = HeaderMap::new();
    headers.insert("x-ms-agent-name", HeaderValue::from_static("azure-init"));
    headers.insert("x-ms-version", HeaderValue::from_static("2012-11-30"));
    headers.insert(
        "Content-Type",
        HeaderValue::from_static("text/xml;charset=utf-8"),
    );

    let post_request = build_report_health_file(goalstate);

    let response = client
        .post(url)
        .headers(headers)
        .body(post_request)
        .send()
        .await?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(Error::HttpStatus {
            endpoint: url.to_owned(),
            status: response.status(),
        })
    }
}

fn build_report_health_file(goalstate: Goalstate) -> String {
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

    let post_request =
        post_request.replace("$GOAL_STATE_INCARNATION", &goalstate.incarnation);
    let post_request = post_request
        .replace("$CONTAINER_ID", &goalstate.container.container_id);
    post_request.replace(
        "$INSTANCE_ID",
        &goalstate
            .container
            .role_instance_list
            .role_instance
            .instance_id,
    )
}

#[cfg(test)]
mod tests {
    use super::{build_report_health_file, Goalstate};

    static GOALSTATE_STR: &str = "<Goalstate>
            <Container>
                <ContainerId>2</ContainerId>
                <RoleInstanceList>
                    <RoleInstance>
                        <InstanceId>test_user_instance_id</InstanceId>
                    </RoleInstance>
                </RoleInstanceList>
            </Container>
            <Version>example_version</Version>
            <Incarnation>test_goal_incarnation</Incarnation>
        </Goalstate>";

    static HEALTH_STR: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <Health xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\">\n\
            <GoalStateIncarnation>test_goal_incarnation</GoalStateIncarnation>\n\
            <Container>\n\
                <ContainerId>2</ContainerId>\n\
                <RoleInstanceList>\n\
                    <Role>\n\
                        <InstanceId>test_user_instance_id</InstanceId>\n\
                        <Health>\n\
                            <State>Ready</State>\n\
                        </Health>\n\
                    </Role>\n\
                </RoleInstanceList>\n\
            </Container>\n\
        </Health>";
    #[test]
    fn test_parsing_goalstate() {
        let goalstate: Goalstate = serde_xml_rs::from_str(GOALSTATE_STR)
            .expect("Failed to parse the goalstate XML.");
        assert_eq!(goalstate.container.container_id, "2".to_owned());
        assert_eq!(
            goalstate
                .container
                .role_instance_list
                .role_instance
                .instance_id,
            "test_user_instance_id".to_owned()
        );
        assert_eq!(goalstate.version, "example_version".to_owned());
        assert_eq!(goalstate.incarnation, "test_goal_incarnation".to_owned());
    }

    #[tokio::test]
    async fn test_build_report_health_file() {
        let goalstate: Goalstate = serde_xml_rs::from_str(GOALSTATE_STR)
            .expect("Failed to parse the goalstate XML.");

        let actual_output = build_report_health_file(goalstate);
        assert_eq!(actual_output, HEALTH_STR);
    }
}
