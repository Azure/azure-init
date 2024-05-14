// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use libazureinit::distro::{Distribution, Distributions};
use libazureinit::{
    goalstate, imds, media,
    reqwest::{header, Client},
    user,
};
use azurekvp::{initialize_tracing, TRACER};
use opentelemetry::global;
use tracing::{info, instrument};
use opentelemetry::trace::Tracer;
use opentelemetry::trace::Span;
use tracing::span;
use tracing::Level;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[instrument]
fn get_username(
    imds_body: String,
) -> Result<String, Box<dyn std::error::Error>> {
    if imds::is_password_authentication_disabled(&imds_body).map_err(|_| {
        "Failed to get disable password authentication".to_string()
    })? {
        // password authentication is disabled
        match imds::get_username(imds_body.clone()) {
            Ok(username) => Ok(username),
            Err(_err) => Err("Failed to get username".into()),
        }
    } else {
        // password authentication is enabled
        let ovf_body = media::read_ovf_env_to_string().unwrap();
        let environment = media::parse_ovf_env(ovf_body.as_str()).unwrap();

        if !environment
            .provisioning_section
            .linux_prov_conf_set
            .password
            .is_empty()
        {
            return Err("password is non-empty".into());
        }

        Ok(environment
            .provisioning_section
            .linux_prov_conf_set
            .username)
    }
}

#[instrument]
fn test () { 
    print!("Hello, World!"); 
}

#[tokio::main]
async fn main() {
    // Initialize the tracing subscriber
    initialize_tracing();
    test();

    let mut default_headers = header::HeaderMap::new();
    let user_agent = header::HeaderValue::from_str(
        format!("azure-init v{VERSION}").as_str(),
    )
    .unwrap();
    default_headers.insert(header::USER_AGENT, user_agent);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .default_headers(default_headers)
        .build()
        .unwrap();
    let query_result = imds::query_imds(&client).await;
    
    let mut query_span = span!(Level::TRACE, "query-imds").entered();

    let imds_body = match query_result {
        Ok(imds_body) => imds_body,
        Err(_err) => std::process::exit(exitcode::CONFIG),
    };
    let mut query_span = query_span.exit();

    let username = match get_username(imds_body.clone()) {
        Ok(res) => res,
        Err(_err) => std::process::exit(exitcode::CONFIG),
    };

    let mut file_path = "/home/".to_string();
    file_path.push_str(username.as_str());

    // always pass an empty password
    Distributions::from("ubuntu")
        .create_user(username.as_str(), "")
        .expect("Failed to create user");
    let _create_directory =
        user::create_ssh_directory(username.as_str(), &file_path).await;

    let get_ssh_key_result = imds::get_ssh_keys(imds_body.clone());
    let keys = match get_ssh_key_result {
        Ok(keys) => keys,
        Err(_err) => std::process::exit(exitcode::CONFIG),
    };

    file_path.push_str("/.ssh");

    user::set_ssh_keys(keys, username.to_string(), file_path.clone()).await;

    let get_hostname_result = imds::get_hostname(imds_body.clone());
    let hostname = match get_hostname_result {
        Ok(hostname) => hostname,
        Err(_err) => std::process::exit(exitcode::CONFIG),
    };

    Distributions::from("ubuntu")
        .set_hostname(hostname.as_str())
        .expect("Failed to set hostname");

    let get_goalstate_result = goalstate::get_goalstate(&client).await;
    let vm_goalstate = match get_goalstate_result {
        Ok(vm_goalstate) => vm_goalstate,
        Err(_err) => std::process::exit(exitcode::CONFIG),
    };

    let report_health_result =
        goalstate::report_health(&client, vm_goalstate).await;
    match report_health_result {
        Ok(report_health) => report_health,
        Err(_err) => std::process::exit(exitcode::CONFIG),
    };
}
