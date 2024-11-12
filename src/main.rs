// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
use anyhow::Context;
use clap::Parser;
use libazureinit::imds::InstanceMetadata;
use libazureinit::tracing::{initialize_tracing, setup_layers};
use libazureinit::User;
use libazureinit::{
    error::Error as LibError,
    goalstate, imds, media,
    media::{get_mount_device, Environment},
    reqwest::{header, Client},
    HostnameProvisioner, PasswordProvisioner, Provision, UserProvisioner,
};
use std::process::ExitCode;
use std::time::Duration;
use sysinfo::{System, SystemExt};
use tracing::instrument;
use tracing::{event, Level};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Minimal provisioning agent for Azure
///
/// Create a user, add SSH public keys, and set the hostname.
///
/// Arguments provided via command-line arguments override any arguments provided
/// via environment variables.
#[derive(Parser, Debug)]
struct Cli {
    /// List of supplementary groups of the provisioned user account.
    ///
    /// Values can be comma-separated and the argument can be provided multiple times.
    #[arg(
        long,
        short,
        env = "AZURE_INIT_USER_GROUPS",
        value_delimiter = ',',
        default_value = ""
    )]
    groups: Vec<String>,
}

#[instrument]
fn get_environment() -> Result<Environment, anyhow::Error> {
    let ovf_devices = get_mount_device(None)?;
    let mut environment: Option<Environment> = None;

    // loop until it finds a correct device.
    for dev in ovf_devices {
        environment = match media::mount_parse_ovf_env(dev) {
            Ok(env) => Some(env),
            Err(_) => continue,
        }
    }

    environment.ok_or_else(|| {
        event!(Level::ERROR, "Unable to get list of block devices");
        anyhow::anyhow!("Unable to get list of block devices")
    })
}

#[instrument(skip_all)]
fn get_username(
    instance_metadata: Option<&InstanceMetadata>,
    environment: Option<&Environment>,
) -> Result<String, anyhow::Error> {
    if let Some(metadata) = instance_metadata {
        if metadata.compute.os_profile.disable_password_authentication {
            // If password authentication is disabled,
            // simply read from IMDS metadata if available.
            return Ok(metadata.compute.os_profile.admin_username.clone());
        }
        // If password authentication is enabled,
        // fall back to reading from OVF environment file.
    }

    // Read username from OVF environment via mounted local device.
    environment
        .map(|env| {
            env.clone()
                .provisioning_section
                .linux_prov_conf_set
                .username
        })
        .ok_or_else(|| {
            event!(Level::ERROR, "Username Failure");
            LibError::UsernameFailure.into()
        })
}

#[tokio::main]
async fn main() -> ExitCode {
    let tracer = initialize_tracing();

    if let Err(e) = setup_layers(tracer) {
        eprintln!("Warning: Failed to set up tracing layers: {:?}", e);
    }

    let result = provision().await;

    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            event!(Level::ERROR, "Provisioning failed with error: {:?}", e);
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

#[instrument(name = "root")]
async fn provision() -> Result<(), anyhow::Error> {
    let system = System::new();
    let kernel_version = system
        .kernel_version()
        .unwrap_or("Unknown Kernel Version".to_string());
    let os_version = system
        .os_version()
        .unwrap_or("Unknown OS Version".to_string());
    let azure_init_version = env!("CARGO_PKG_VERSION");

    event!(
        Level::INFO,
        msg = format!(
            "Kernel Version: {}, OS Version: {}, Azure-Init Version: {}",
            kernel_version, os_version, azure_init_version
        )
    );

    let opts = Cli::parse();

    let mut default_headers = header::HeaderMap::new();
    let user_agent = header::HeaderValue::from_str(
        format!("azure-init v{VERSION}").as_str(),
    )?;
    default_headers.insert(header::USER_AGENT, user_agent);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .default_headers(default_headers)
        .build()?;

    let imds_http_timeout_sec: u64 = 5 * 60;
    let imds_http_retry_interval_sec: u64 = 2;

    // Username can be obtained either via fetching instance metadata from IMDS
    // or mounting a local device for OVF environment file. It should not fail
    // immediately in a single failure, instead it should fall back to the other
    // mechanism. So it is not a good idea to use `?` for query() or
    // get_environment().
    let instance_metadata = imds::query(
        &client,
        Duration::from_secs(imds_http_retry_interval_sec),
        Duration::from_secs(imds_http_timeout_sec),
        None, // default IMDS URL
    )
    .await
    .ok();

    let environment = get_environment().ok();

    let username =
        get_username(instance_metadata.as_ref(), environment.as_ref())?;

    // It is necessary to get the actual instance metadata after getting username,
    // as it is not desirable to immediately return error before get_username.
    let im = instance_metadata
        .clone()
        .ok_or::<LibError>(LibError::InstanceMetadataFailure)?;

    let user =
        User::new(username, im.compute.public_keys).with_groups(opts.groups);

    Provision::new(im.compute.os_profile.computer_name, user)
        .hostname_provisioners([
            #[cfg(feature = "hostnamectl")]
            HostnameProvisioner::Hostnamectl,
        ])
        .user_provisioners([
            #[cfg(feature = "useradd")]
            UserProvisioner::Useradd,
        ])
        .password_provisioners([
            #[cfg(feature = "passwd")]
            PasswordProvisioner::Passwd,
        ])
        .provision()?;

    let vm_goalstate = goalstate::get_goalstate(
        &client,
        Duration::from_secs(imds_http_retry_interval_sec),
        Duration::from_secs(imds_http_timeout_sec),
        None, // default wireserver goalstate URL
    )
    .await
    .with_context(|| {
        event!(Level::ERROR, "Failed to get the desired goalstate.");
        "Failed to get desired goalstate."
    })?;

    goalstate::report_health(
        &client,
        vm_goalstate,
        Duration::from_secs(imds_http_retry_interval_sec),
        Duration::from_secs(imds_http_timeout_sec),
        None, // default wireserver health URL
    )
    .await
    .with_context(|| {
        event!(Level::ERROR, "Failed to report VM health.");
        "Failed to report VM health."
    })?;

    Ok(())
}
