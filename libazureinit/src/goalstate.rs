// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::Client;
use tracing::instrument;

use std::time::Duration;

use serde::Deserialize;
use serde_xml_rs::from_str;

use crate::error::Error;
use crate::http;

/// Azure goalstate of the virtual machine. Metadata is written in XML format.
///
/// Required fields are Container, Version, Incarnation.
///
/// # Example
///
/// ```
/// # use libazureinit::goalstate::Goalstate;
///
/// static GOALSTATE_STR: &str = "<Goalstate>
///         <Container>
///             <ContainerId>2</ContainerId>
///             <RoleInstanceList>
///                 <RoleInstance>
///                     <InstanceId>test_user_instance_id</InstanceId>
///                 </RoleInstance>
///             </RoleInstanceList>
///         </Container>
///         <Version>example_version</Version>
///         <Incarnation>test_goal_incarnation</Incarnation>
///     </Goalstate>";
///
/// let goalstate: Goalstate = serde_xml_rs::from_str(GOALSTATE_STR)
///     .expect("Failed to parse the goalstate XML.");
/// ```
#[derive(Debug, Deserialize, PartialEq)]
pub struct Goalstate {
    #[serde(rename = "Container")]
    container: Container,
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "Incarnation")]
    incarnation: String,
}

/// Container of [`Goalstate`] of the virtual machine. Metadata is written in XML format.
#[derive(Debug, Deserialize, PartialEq)]
pub struct Container {
    #[serde(rename = "ContainerId")]
    container_id: String,
    #[serde(rename = "RoleInstanceList")]
    role_instance_list: RoleInstanceList,
}

/// List of role instances of goalstate. Metadata is written in XML format.
#[derive(Debug, Deserialize, PartialEq)]
pub struct RoleInstanceList {
    #[serde(rename = "RoleInstance")]
    role_instance: RoleInstance,
}

/// Role instance of goalstate. Metadata is written in XML format.
#[derive(Debug, Deserialize, PartialEq)]
pub struct RoleInstance {
    #[serde(rename = "InstanceId")]
    instance_id: String,
}

const DEFAULT_GOALSTATE_URL: &str =
    "http://168.63.129.16/machine/?comp=goalstate";

/// Fetch Azure goalstate of Azure wireserver.
///
/// Caller needs to pass 3 required parameters, client, retry_interval,
/// total_timeout. It is therefore required to create a reqwest::Client
/// variable with possible options, to pass it as parameter.
///
/// Parameter url is optional. If None is passed, it defaults to
/// DEFAULT_GOALSTATE_URL, an internal goalstate URL available in the Azure VM.
///
/// # Example
///
/// ```
/// # use std::time::Duration;
/// use libazureinit::reqwest::Client;
///
/// let client = Client::builder()
///     .timeout(std::time::Duration::from_secs(5))
///     .build()
///     .unwrap();
///
/// let res = libazureinit::goalstate::get_goalstate(
///     &client, Duration::from_secs(1), Duration::from_secs(5),
///     Some("http://127.0.0.1:8000/"),
/// );
/// ```
#[instrument(err, skip_all)]
pub async fn get_goalstate(
    client: &Client,
    retry_interval: Duration,
    mut total_timeout: Duration,
    url: Option<&str>,
) -> Result<Goalstate, Error> {
    let mut headers = HeaderMap::new();
    headers.insert("x-ms-agent-name", HeaderValue::from_static("azure-init"));
    headers.insert("x-ms-version", HeaderValue::from_static("2012-11-30"));
    let url = url.unwrap_or(DEFAULT_GOALSTATE_URL);
    let request_timeout =
        Duration::from_secs(http::WIRESERVER_HTTP_TIMEOUT_SEC);

    while !total_timeout.is_zero() {
        let (response, remaining_timeout) = http::get(
            client,
            headers.clone(),
            request_timeout,
            retry_interval,
            total_timeout,
            url,
        )
        .await?;
        match response.text().await {
            Ok(body) => {
                let goalstate = from_str(&body).map_err(|error| {
                    tracing::warn!(
                        ?error,
                        "The response body was invalid and could not be deserialized"
                    );
                    error.into()
                });
                if goalstate.is_ok() {
                    tracing::info!(
                        operation_status = "success",
                        "Successfully retrieved and parsed goalstate."
                    );
                    return goalstate;
                }
            }
            Err(error) => {
                tracing::warn!(?error, "Failed to read the full response body")
            }
        }

        total_timeout = remaining_timeout;
    }

    Err(Error::Timeout)
}

const DEFAULT_HEALTH_URL: &str = "http://168.63.129.16/machine/?comp=health";

/// Report health stateus to Azure wireserver.
///
/// Caller needs to pass 4 required parameters, client, retry_interval,
/// total_timeout, goalstate. It is therefore required to create a reqwest::Client
/// variable with possible options, to pass it as parameter. Also caller must
/// first run get_goalstate to get GoalState variable to pass it as parameter of
/// report_health.
///
/// Parameter url optional. If None is passed, it defaults to DEFAULT_HEALTH_URL,
/// an internal health report URL available in the Azure VM.
///
/// # Example
///
/// ```rust,no_run
/// # use std::time::Duration;
/// use libazureinit::reqwest::Client;
///
/// #[tokio::main]
/// async fn main() {
///     let client = Client::builder()
///         .timeout(std::time::Duration::from_secs(5))
///         .build()
///         .unwrap();
///
///     let vm_goalstate = libazureinit::goalstate::get_goalstate(
///         &client, Duration::from_secs(1), Duration::from_secs(5),
///         Some("http://127.0.0.1:8000/"),
///     ).await.unwrap();
///
///     let res = libazureinit::goalstate::report_health(
///         &client, vm_goalstate, Duration::from_secs(1), Duration::from_secs(5),
///         Some("http://127.0.0.1:8000/"),
///     );
/// }
/// ```
#[instrument(err, skip_all)]
pub async fn report_health(
    client: &Client,
    goalstate: Goalstate,
    retry_interval: Duration,
    total_timeout: Duration,
    url: Option<&str>,
) -> Result<(), Error> {
    let mut headers = HeaderMap::new();
    headers.insert("x-ms-agent-name", HeaderValue::from_static("azure-init"));
    headers.insert("x-ms-version", HeaderValue::from_static("2012-11-30"));
    headers.insert(
        "Content-Type",
        HeaderValue::from_static("text/xml;charset=utf-8"),
    );
    let request_timeout =
        Duration::from_secs(http::WIRESERVER_HTTP_TIMEOUT_SEC);
    let url = url.unwrap_or(DEFAULT_HEALTH_URL);

    let post_request = build_report_health_file(goalstate);

    _ = http::post(
        client,
        headers,
        post_request,
        request_timeout,
        retry_interval,
        total_timeout,
        url,
    )
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

    // Assert malformed responses are retried.
    //
    // In this case the server doesn't return XML at all.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn malformed_response() {
        let body = "You thought this was XML, but you were wrong";
        let payload = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n\r\n{}",
             StatusCode::OK.as_u16(),
             StatusCode::OK.to_string(),
             body.len(),
             body
        );

        let serverlistener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = serverlistener.local_addr().unwrap();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let server = tokio::spawn(unittest::serve_requests(
            serverlistener,
            payload,
            cancel_token.clone(),
        ));

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();

        let res = get_goalstate(
            &client,
            Duration::from_millis(10),
            Duration::from_millis(50),
            Some(format!("http://{:}:{:}/", addr.ip(), addr.port()).as_str()),
        )
        .await;

        cancel_token.cancel();

        let requests = server.await.unwrap();
        assert!(requests >= 2);
        assert!(logs_contain(
            "The response body was invalid and could not be deserialized"
        ));
        match res {
            Err(crate::error::Error::Timeout) => {}
            _ => panic!("Response should have timed out"),
        };
    }
}
