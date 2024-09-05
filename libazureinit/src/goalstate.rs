// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::{Client, StatusCode};

use std::time::Duration;

use serde::Deserialize;
use serde_xml_rs::from_str;

use tokio::time::timeout;

use crate::error::Error;
use crate::http;

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

const DEFAULT_GOALSTATE_URL: &str =
    "http://168.63.129.16/machine/?comp=goalstate";

pub async fn get_goalstate(
    client: &Client,
    retry_interval: Duration,
    total_timeout: Duration,
    url: Option<&str>,
) -> Result<Goalstate, Error> {
    let url = url.unwrap_or(DEFAULT_GOALSTATE_URL);

    let mut headers = HeaderMap::new();
    headers.insert("x-ms-agent-name", HeaderValue::from_static("azure-init"));
    headers.insert("x-ms-version", HeaderValue::from_static("2012-11-30"));

    let response = timeout(total_timeout, async {
        let now = std::time::Instant::now();
        loop {
            if let Ok(response) = client
                .get(url)
                .headers(headers.clone())
                .timeout(Duration::from_secs(http::WIRESERVER_HTTP_TIMEOUT_SEC))
                .send()
                .await
            {
                let statuscode = response.status();

                if statuscode == StatusCode::OK {
                    tracing::info!(
                        "HTTP response succeeded with status {}",
                        statuscode
                    );
                    return Ok(response);
                }

                if !http::RETRY_CODES.contains(&statuscode) {
                    return response.error_for_status().map_err(|error| {
                        tracing::error!(
                            ?error,
                            "{}",
                            format!(
                                "HTTP call failed due to status {}",
                                statuscode
                            )
                        );
                        error
                    });
                }
            }

            tracing::info!("Retrying to get HTTP response in {} sec, remaining timeout {} sec.", retry_interval.as_secs(), total_timeout.saturating_sub(now.elapsed()).as_secs());

            tokio::time::sleep(retry_interval).await;
        }
    })
    .await?;

    let goalstate_body = response?.text().await?;

    let goalstate: Goalstate = from_str(&goalstate_body)?;

    Ok(goalstate)
}

const DEFAULT_HEALTH_URL: &str = "http://168.63.129.16/machine/?comp=health";

pub async fn report_health(
    client: &Client,
    goalstate: Goalstate,
    retry_interval: Duration,
    total_timeout: Duration,
    url: Option<&str>,
) -> Result<(), Error> {
    let url = url.unwrap_or(DEFAULT_HEALTH_URL);

    let mut headers = HeaderMap::new();
    headers.insert("x-ms-agent-name", HeaderValue::from_static("azure-init"));
    headers.insert("x-ms-version", HeaderValue::from_static("2012-11-30"));
    headers.insert(
        "Content-Type",
        HeaderValue::from_static("text/xml;charset=utf-8"),
    );

    let post_request = build_report_health_file(goalstate);

    _ = timeout(total_timeout, async {
        let now = std::time::Instant::now();
        loop {
            if let Ok(response) = client
                .post(url)
                .headers(headers.clone())
                .body(post_request.clone())
                .timeout(Duration::from_secs(http::WIRESERVER_HTTP_TIMEOUT_SEC))
                .send()
                .await
            {
                let statuscode = response.status();

                if statuscode == StatusCode::OK {
                    tracing::info!("HTTP response succeeded with status {}", statuscode);
                    return Ok(response);
                }

                if !http::RETRY_CODES.contains(&statuscode) {
                    return response.error_for_status().map_err(|error| {
                        tracing::error!(?error, "{}", format!("HTTP call failed due to status {}", statuscode));
                        error
                    });
                }
            }

            tracing::info!("Retrying to get HTTP response in {} sec, remaining timeout {} sec.", retry_interval.as_secs(), total_timeout.saturating_sub(now.elapsed()).as_secs());

            tokio::time::sleep(retry_interval).await;
        }
    })
    .await?;

    Ok(())
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
    use super::{
        build_report_health_file, get_goalstate, report_health, Goalstate,
    };

    use reqwest::{header, Client, StatusCode};
    use std::time::Duration;
    use tokio::net::TcpListener;

    use crate::{http, unittest};

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

    // Runs a test around sending via get_goalstate() with a given statuscode.
    async fn run_goalstate_retry(statuscode: &StatusCode) -> bool {
        const HTTP_TOTAL_TIMEOUT_SEC: u64 = 5;
        const HTTP_PERCLIENT_TIMEOUT_SEC: u64 = 5;
        const HTTP_RETRY_INTERVAL_SEC: u64 = 1;

        let mut default_headers = header::HeaderMap::new();
        let user_agent =
            header::HeaderValue::from_str("azure-init test").unwrap();

        // Run local test servers for goalstate and health that reply with simple test data.
        let gs_ok_payload =
            unittest::get_http_response_payload(statuscode, GOALSTATE_STR);
        let gs_serverlistener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gs_addr = gs_serverlistener.local_addr().unwrap();

        let health_ok_payload =
            unittest::get_http_response_payload(statuscode, HEALTH_STR);
        let health_serverlistener =
            TcpListener::bind("127.0.0.1:0").await.unwrap();
        let health_addr = health_serverlistener.local_addr().unwrap();

        let cancel_token = tokio_util::sync::CancellationToken::new();

        let gs_server = tokio::spawn(unittest::serve_requests(
            gs_serverlistener,
            gs_ok_payload,
            cancel_token.clone(),
        ));
        let health_server = tokio::spawn(unittest::serve_requests(
            health_serverlistener,
            health_ok_payload,
            cancel_token.clone(),
        ));

        default_headers.insert(header::USER_AGENT, user_agent);
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(HTTP_PERCLIENT_TIMEOUT_SEC))
            .default_headers(default_headers)
            .build()
            .unwrap();

        let vm_goalstate = get_goalstate(
            &client,
            Duration::from_secs(HTTP_RETRY_INTERVAL_SEC),
            Duration::from_secs(HTTP_TOTAL_TIMEOUT_SEC),
            Some(
                format!("http://{:}:{:}/", gs_addr.ip(), gs_addr.port())
                    .as_str(),
            ),
        )
        .await;

        if !vm_goalstate.is_ok() {
            cancel_token.cancel();

            let gs_requests = gs_server.await.unwrap();
            let health_requests = health_server.await.unwrap();

            if http::HARDFAIL_CODES.contains(statuscode) {
                assert_eq!(gs_requests, 1);
                assert_eq!(health_requests, 0);
            }

            if http::RETRY_CODES.contains(statuscode) {
                assert!(gs_requests >= 4);
                assert_eq!(health_requests, 0);
            }

            return false;
        }

        let res_health = report_health(
            &client,
            vm_goalstate.unwrap(),
            Duration::from_secs(HTTP_RETRY_INTERVAL_SEC),
            Duration::from_secs(HTTP_TOTAL_TIMEOUT_SEC),
            Some(
                format!(
                    "http://{:}:{:}/",
                    health_addr.ip(),
                    health_addr.port()
                )
                .as_str(),
            ),
        )
        .await;

        res_health.is_ok()
    }

    #[tokio::test]
    async fn goalstate_query_retry() {
        // status codes that should succeed.
        assert!(run_goalstate_retry(&StatusCode::OK).await);

        // status codes that should be retried up to 5 minutes.
        for rc in http::RETRY_CODES {
            assert!(!run_goalstate_retry(rc).await);
        }

        // status codes that should result into immediate failures.
        for rc in http::HARDFAIL_CODES {
            assert!(!run_goalstate_retry(rc).await);
        }
    }
}
