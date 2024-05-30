// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::ExitCode;

use anyhow::Context;

use libazureinit::distro::{Distribution, Distributions};
use libazureinit::imds::InstanceMetadata;
use libazureinit::{
    error::Error as LibError,
    goalstate, imds, media,
    media::Environment,
    reqwest::{header, Client},
    user,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn get_environment() -> Result<Environment, anyhow::Error> {
    let ovf_devices = media::get_mount_device()?;
    let mut environment: Option<Environment> = None;

    // loop until it finds a correct device.
    for dev in ovf_devices {
        environment = match media::mount_parse_ovf_env(dev) {
            Ok(env) => Some(env),
            Err(_) => continue,
        }
    }

    environment
        .ok_or_else(|| anyhow::anyhow!("Unable to get list of block devices"))
}

fn get_username(
    instance_metadata: &InstanceMetadata,
    environment: &Environment,
) -> Result<String, anyhow::Error> {
    if instance_metadata
        .compute
        .os_profile
        .disable_password_authentication
    {
        // password authentication is disabled
        Ok(instance_metadata.compute.os_profile.admin_username.clone())
    } else {
        // password authentication is enabled

        Ok(environment
            .clone()
            .provisioning_section
            .linux_prov_conf_set
            .username)
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match provision().await {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{:?}", e);
            let config: u8 = exitcode::CONFIG
                .try_into()
                .expect("Error code must be less than 256");
            match e.root_cause().downcast_ref::<LibError>() {
                Some(LibError::UserMissing { user: _ }) => {
                    ExitCode::from(config)
                }
                Some(LibError::NonEmptyPassword) => ExitCode::from(config),
                Some(_) | None => ExitCode::FAILURE,
            }
        }
    }
}

async fn provision() -> Result<(), anyhow::Error> {
    let mut default_headers = header::HeaderMap::new();
    let user_agent = header::HeaderValue::from_str(
        format!("azure-init v{VERSION}").as_str(),
    )?;
    default_headers.insert(header::USER_AGENT, user_agent);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .default_headers(default_headers)
        .build()?;

    let instance_metadata = imds::query(&client).await?;
    let username = get_username(&instance_metadata, &get_environment()?)?;

    let mut file_path = "/home/".to_string();
    file_path.push_str(username.as_str());

    // always pass an empty password
    Distributions::from("ubuntu")
        .create_user(username.as_str(), "")
        .with_context(|| format!("Unabled to create user '{username}'"))?;

    user::create_ssh_directory(username.as_str(), &file_path)
        .await
        .with_context(|| "Failed to create ssh directory.")?;

    file_path.push_str("/.ssh");

    user::set_ssh_keys(
        instance_metadata.compute.public_keys,
        username.to_string(),
        file_path.clone(),
    )
    .await
    .with_context(|| "Failed to write ssh public keys.")?;

    Distributions::from("ubuntu")
        .set_hostname(
            instance_metadata.compute.os_profile.computer_name.as_str(),
        )
        .with_context(|| "Failed to set hostname.")?;

    let vm_goalstate = goalstate::get_goalstate(&client)
        .await
        .with_context(|| "Failed to get desired goalstate.")?;
    goalstate::report_health(&client, vm_goalstate)
        .await
        .with_context(|| "Failed to report VM health.")?;

    Ok(())
}
