// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use libazureinit::imds::PublicKeys;
use libazureinit::{
    goalstate, provision,
    reqwest::{header, Client},
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

    provision::Provision::new("my-hostname".to_string(), username.to_string())
        .hostname_provisioners([provision::hostname::Provisioner::Hostnamectl])
        .user_provisioners([provision::user::Provisioner::Useradd])
        .ssh_keys(keys)
        .password("".to_string())
        .password_provisioners([provision::password::Provisioner::Passwd])
        .provision()
        .expect("Failed to provision host");

    println!("VM successfully provisioned");
    println!();

    println!("**********************************");
    println!("* Functional testing completed successfully!");
    println!("**********************************");
    println!();
}
