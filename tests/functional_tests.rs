// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use libazureinit::config::Config;
use libazureinit::imds::PublicKeys;
use libazureinit::User;
use libazureinit::{
    goalstate, imds,
    reqwest::{header, Client},
    Provision,
};
use std::env;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let config = Config::default();

    let cli_args: Vec<String> = env::args().collect();
    let mut default_headers = header::HeaderMap::new();
    let user_agent = header::HeaderValue::from_str("azure-init").unwrap();
    default_headers.insert(header::USER_AGENT, user_agent);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .default_headers(default_headers)
        .build()
        .unwrap();

    println!();
    println!("**********************************");
    println!("* Beginning functional testing");
    println!("**********************************");
    println!();

    println!("Querying wireserver for Goalstate");

    let http_timeout_sec: u64 = 5 * 60;
    let http_retry_interval_sec: u64 = 2;

    let get_goalstate_result = goalstate::get_goalstate(
        &client,
        Duration::from_secs(http_retry_interval_sec),
        Duration::from_secs(http_timeout_sec),
        None, // default wireserver goalstate URL
    )
    .await;
    let vm_goalstate = match get_goalstate_result {
        Ok(vm_goalstate) => vm_goalstate,
        Err(_err) => return,
    };

    println!("Goalstate successfully received");
    println!();
    println!("Reporting VM Health to wireserver");

    let report_health_result = goalstate::report_health(
        &client,
        vm_goalstate,
        Duration::from_secs(http_retry_interval_sec),
        Duration::from_secs(http_timeout_sec),
        None, // default wireserver health URL
    )
    .await;
    match report_health_result {
        Ok(report_health) => report_health,
        Err(_err) => return,
    };

    println!("VM Health successfully reported");

    let imds_http_timeout_sec: u64 = 5 * 60;
    let imds_http_retry_interval_sec: u64 = 2;

    // Simplified version of calling imds::query. Since username is directly
    // given by cli_args below, it is not needed to get instance metadata like
    // how it is done in provision() in main.
    let _ = imds::query(
        &client,
        Duration::from_secs(imds_http_retry_interval_sec),
        Duration::from_secs(imds_http_timeout_sec),
        None, // default IMDS URL
    )
    .await
    .expect("Failed to query IMDS");

    let username = &cli_args[1];

    let keys: Vec<PublicKeys> = vec![
        PublicKeys {
            path: "/path/to/.ssh/keys/".to_owned(),
            key_data: "ssh-rsa test_key_1".to_owned(),
        },
        PublicKeys {
            path: "/path/to/.ssh/keys/".to_owned(),
            key_data: "ssh-rsa test_key_2".to_owned(),
        },
        PublicKeys {
            path: "/path/to/.ssh/keys/".to_owned(),
            key_data: "ssh-rsa test_key_3".to_owned(),
        },
    ];

    Provision::new(
        "my-hostname".to_string(),
        User::new(username, keys),
        config,
        false,
    )
    .provision()
    .expect("Failed to provision host");

    println!("VM successfully provisioned");
    println!();

    println!("**********************************");
    println!("* Functional testing completed successfully!");
    println!("**********************************");
    println!();
}
