// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
use std::path::PathBuf;
mod kvp;
mod logging;
pub use logging::{initialize_tracing, setup_layers};

use anyhow::Context;
use clap::{Parser, Subcommand};
use libazureinit::config::Config;
use libazureinit::imds::InstanceMetadata;
use libazureinit::User;
use libazureinit::{
    error::Error as LibError,
    goalstate, imds, media,
    media::{get_mount_device, Environment},
    reqwest::{header, Client},
    Provision,
};
use libazureinit::{
    get_vm_id, is_provisioning_complete, mark_provisioning_complete,
};
use std::process::ExitCode;
use std::time::Duration;
use sysinfo::System;
use tracing::instrument;
use tracing_subscriber::{prelude::*, Layer};

// These should be set during the build process
const VERSION: &str = env!("CARGO_PKG_VERSION");
const COMMIT_HASH: &str = env!("GIT_COMMIT_HASH");

/// Minimal provisioning agent for Azure
///
/// Create a user, add SSH public keys, and set the hostname.
/// By default, if no subcommand is specified, this will provision the host.
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

    #[arg(
        long,
        help = "Path to the configuration file",
        env = "AZURE_INIT_CONFIG"
    )]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// By default, this removes provisioning state data. Optional flags can be used
    /// to clean logs or additional generated files.
    Clean {
        /// Cleans the log files as defined in the configuration file
        #[arg(long)]
        logs: bool,
    },
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
        tracing::error!("Unable to get list of block devices");
        anyhow::anyhow!("Unable to get list of block devices")
    })
}

#[instrument(skip_all)]
fn get_username(
    instance_metadata: Option<&InstanceMetadata>,
    environment: Option<&Environment>,
) -> Result<String, anyhow::Error> {
    if let Some(metadata) = instance_metadata {
        return Ok(metadata.compute.os_profile.admin_username.clone());
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
            tracing::error!("Username Failure");
            LibError::UsernameFailure.into()
        })
}

/// Cleans all provisioning state marker files from the azure-init data directory.
///
/// This removes all files ending in `.provisioned` from the directory specified
/// by `azure_init_data_dir` (typically `/var/lib/azure-init`). These marker files
/// indicate that provisioning has completed. Removing them allows azure-init to
/// re-run provisioning logic on the next boot.
#[instrument]
fn clean_provisioning_status(config: &Config) -> Result<(), std::io::Error> {
    let data_dir = &config.azure_init_data_dir.path;
    let mut found = false;

    for entry in std::fs::read_dir(data_dir)? {
        let path = match entry {
            Ok(e) => e.path(),
            Err(e) => {
                tracing::error!(
                    "Failed to read directory entry in {:?}: {:?}",
                    data_dir,
                    e
                );
                return Err(e);
            }
        };

        if path.extension().is_some_and(|ext| ext == "provisioned") {
            found = true;

            match std::fs::remove_file(&path) {
                Ok(_) => {
                    tracing::info!(
                        "Successfully removed provisioning state at: {:?}",
                        path
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tracing::info!(
                        "No provisioning marker found at: {:?}",
                        path
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to clean provisioning marker {:?}: {:?}",
                        path,
                        e
                    );
                    return Err(e);
                }
            }
        }
    }

    if !found {
        tracing::info!(
            "No provisioning marker files (*.provisioned) found in {:?}",
            data_dir
        );
    }

    Ok(())
}

/// Cleans the azure-init log file defined in the configuration.
///
/// This removes the log file at the path configured by `azure_init_log_path`,
/// which defaults to `/var/log/azure-init.log`. If the file does not exist,
/// a message is logged but no error is returned.
#[instrument]
fn clean_log_file(config: &Config) -> Result<(), std::io::Error> {
    let log_path = &config.azure_init_log_path.path;

    match std::fs::remove_file(log_path) {
        Ok(_) => {
            tracing::info!("Successfully removed log file at: {:?}", log_path);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("No log file found at: {:?}", log_path);
        }
        Err(e) => {
            tracing::error!("Failed to clean log file {:?}: {:?}", log_path, e);
            return Err(e);
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let tracer = initialize_tracing();
    let vm_id: String = get_vm_id()
        .unwrap_or_else(|| "00000000-0000-0000-0000-000000000000".to_string());
    let opts = Cli::parse();

    let temp_layer = tracing_subscriber::fmt::layer()
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NONE)
        .with_writer(std::io::stderr)
        .with_filter(tracing_subscriber::EnvFilter::new(
            "libazureinit::config=info",
        ));

    let temp_subscriber =
        tracing_subscriber::Registry::default().with(temp_layer);

    let config =
        match tracing::subscriber::with_default(temp_subscriber, || {
            Config::load(opts.config.clone())
        }) {
            Ok(cfg) => cfg,
            Err(error) => {
                eprintln!("Failed to load configuration: {error:?}");
                eprintln!("Example configuration:\n\n{}", Config::default());
                return ExitCode::FAILURE;
            }
        };

    if let Err(e) = setup_layers(tracer, &vm_id, &config) {
        tracing::error!("Failed to set final logging subscriber: {e:?}");
    }

    tracing::info!(
        target = "libazureinit::config::success",
        "Final configuration: {:#?}",
        config
    );

    if let Some(Command::Clean { logs }) = opts.command {
        if clean_provisioning_status(&config).is_err() {
            return ExitCode::FAILURE;
        }

        if logs && clean_log_file(&config).is_err() {
            return ExitCode::FAILURE;
        }

        return ExitCode::SUCCESS;
    }

    if is_provisioning_complete(Some(&config), &vm_id) {
        tracing::info!(
            "Provisioning already completed earlier. Skipping provisioning."
        );
        return ExitCode::SUCCESS;
    }

    match provision(config, &vm_id, opts).await {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("Provisioning failed with error: {:?}", e);
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

#[instrument(name = "root", skip_all)]
async fn provision(
    config: Config,
    vm_id: &str,
    opts: Cli,
) -> Result<(), anyhow::Error> {
    let kernel_version = System::kernel_version()
        .unwrap_or("Unknown Kernel Version".to_string());
    let os_version =
        System::os_version().unwrap_or("Unknown OS Version".to_string());

    tracing::info!(
        "Kernel Version: {}, OS Version: {}, Azure-Init Version: {}",
        kernel_version,
        os_version,
        VERSION
    );

    let clone_config = config.clone();

    let mut default_headers = header::HeaderMap::new();
    let user_agent = if cfg!(debug_assertions) {
        format!("azure-init v{}-{}", VERSION, COMMIT_HASH)
    } else {
        format!("azure-init v{}", VERSION)
    };
    let user_agent = header::HeaderValue::from_str(user_agent.as_str())?;
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

    Provision::new(im.compute.os_profile.computer_name, user, config)
        .provision()?;

    let vm_goalstate = goalstate::get_goalstate(
        &client,
        Duration::from_secs(imds_http_retry_interval_sec),
        Duration::from_secs(imds_http_timeout_sec),
        None, // default wireserver goalstate URL
    )
    .await
    .with_context(|| {
        tracing::error!("Failed to get the desired goalstate.");
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
        tracing::error!("Failed to report VM health.");
        "Failed to report VM health."
    })?;

    mark_provisioning_complete(Some(&clone_config), vm_id).with_context(
        || {
            tracing::error!("Failed to mark provisioning complete.");
            "Failed to mark provisioning complete."
        },
    )?;

    Ok(())
}
