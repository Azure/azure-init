// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use libazureinit::distro::{Distribution, Distributions};
use libazureinit::imds::PublicKeys;
use libazureinit::{
    goalstate,
    reqwest::{header, Client},
    user,
};

use std::env;

#[tokio::main]
async fn main() {
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

    let get_goalstate_result = goalstate::get_goalstate(&client).await;
    let vm_goalstate = match get_goalstate_result {
        Ok(vm_goalstate) => vm_goalstate,
        Err(_err) => return,
    };

    println!("Goalstate successfully received");
    println!();
    println!("Reporting VM Health to wireserver");

    let report_health_result =
        goalstate::report_health(&client, vm_goalstate).await;
    match report_health_result {
        Ok(report_health) => report_health,
        Err(_err) => return,
    };

    println!("VM Health successfully reported");

    let username = &cli_args[1];

    let mut file_path = "/home/".to_string();
    file_path.push_str(username.as_str());

    println!();
    println!(
        "Attempting to create user {} without password",
        username.as_str()
    );

    Distributions::from("ubuntu")
        .create_user(username.as_str(), "")
        .expect("Failed to create user");

    println!("User {} was successfully created", username.as_str());

    println!();
    println!("Attempting to create user's SSH directory");

    let _create_directory =
        user::create_ssh_directory(username.as_str(), &file_path).await;
    match _create_directory {
        Ok(create_directory) => create_directory,
        Err(_err) => return,
    };
    println!("User's SSH directory was successfully created");

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

    file_path.push_str("/.ssh");

    user::set_ssh_keys(keys, username.to_string(), file_path.clone()).await;

    println!();
    println!("Attempting to set the VM hostname");

    Distributions::from("ubuntu")
        .set_hostname("test-hostname-set")
        .expect("Failed to set hostname");
    println!("VM hostname successfully set");
    println!();

    println!("**********************************");
    println!("* Functional testing completed successfully!");
    println!("**********************************");
    println!();
}
