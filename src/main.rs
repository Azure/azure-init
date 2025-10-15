// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use libazureinit::{
    config::Config,
    error::Error as LibError,
    get_vm_id,
    health::{report_failure, report_ready},
    imds::{query, InstanceMetadata},
    is_provisioning_complete,
    logging::setup_layers,
    mark_provisioning_complete,
    media::{get_mount_device, mount_parse_ovf_env, Environment},
    reqwest::{header, Client},
    Provision, User,
};
use std::process::ExitCode;
use std::time::Duration;
use sysinfo::System;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use tracing_subscriber::{prelude::*, Layer};

const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

fn version_string() -> String {
    if let Some(v) = option_env!("AZURE_INIT_VERSION") {
        return v.to_string();
    }

    if let Some(desc) = option_env!("VERGEN_GIT_DESCRIBE") {
        format!("{PKG_VERSION}-{desc}")
    } else if let Some(sha) = option_env!("VERGEN_GIT_SHA") {
        let short = &sha[..std::cmp::min(7, sha.len())];
        format!("{PKG_VERSION}-{short}")
    } else {
        PKG_VERSION.to_string()
    }
}

/// Minimal provisioning agent for Azure
///
/// Create a user, add SSH public keys, and set the hostname.
/// By default, if no subcommand is specified, this will provision the host.
///
/// Arguments provided via command-line arguments override any arguments provided
/// via environment variables.
#[derive(Parser, Debug)]
#[command(disable_version_flag = true)]
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

    /// Print version information and exit
    #[arg(long = "version", short = 'V', action = clap::ArgAction::SetTrue)]
    show_version: bool,

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

/// Attempts to find and parse provisioning data from an OVF environment.
///
/// This function iterates through available block devices,
/// searching for an OVF environment file (`ovf-env.xml`). This file contains
/// provisioning parameters such as username, SSH keys, and hostname.
///
/// This is one of two primary sources for provisioning data, the other being
/// the Azure Instance Metadata Service (IMDS). The agent prioritizes IMDS
/// when available for most data, but can use OVF as a fallback for the username.
#[instrument]
fn get_environment() -> Result<Environment, anyhow::Error> {
    tracing::debug!("Searching for OVF environment on local block devices.");
    let ovf_devices = get_mount_device(None)?;
    let mut environment: Option<Environment> = None;

    // loop until it finds a correct device.
    for dev in ovf_devices {
        environment = match mount_parse_ovf_env(dev) {
            Ok(env) => {
                tracing::info!(
                    target = "libazureinit::media::success",
                    "Successfully parsed OVF environment."
                );
                Some(env)
            }
            Err(_) => continue,
        }
    }

    environment.ok_or_else(|| {
        tracing::warn!("Failed to find valid OVF provisioning data on any block device. Falling back to IMDS.");
        anyhow::anyhow!("Unable to get list of block devices")
    })
}

#[instrument(skip_all)]
fn get_username(
    instance_metadata: Option<&InstanceMetadata>,
    environment: Option<&Environment>,
) -> Result<String, anyhow::Error> {
    if let Some(metadata) = instance_metadata {
        tracing::debug!("Using username from IMDS.");
        return Ok(metadata.compute.os_profile.admin_username.clone());
    }

    // Read username from OVF environment via mounted local device.
    tracing::debug!("IMDS metadata not available, attempting to get username from OVF environment.");
    environment
        .map(|env| {
            tracing::debug!("Using username from OVF environment.");
            env.clone()
                .provisioning_section
                .linux_prov_conf_set
                .username
        })
        .ok_or_else(|| {
            tracing::error!("Username Failure: Could not determine username from IMDS or OVF environment.");
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
    let vm_id: String = get_vm_id()
        .unwrap_or_else(|| "00000000-0000-0000-0000-000000000000".to_string());

    let opts = Cli::parse();
    if opts.show_version {
        println!("azure-init {}", version_string());
        return ExitCode::SUCCESS;
    }

    let graceful_shutdown = CancellationToken::new();

    let temp_layer = tracing_subscriber::fmt::layer()
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NONE)
        .with_writer(std::io::stderr)
        .with_filter(tracing_subscriber::EnvFilter::new(
            "libazureinit::config=info",
        ));

    let temp_subscriber =
        tracing_subscriber::Registry::default().with(temp_layer);

    let setup_result =
        tracing::subscriber::with_default(temp_subscriber, || {
            let config = Config::load(opts.config.clone())?;
            let (subscriber, rx) =
                setup_layers(&vm_id, &config, graceful_shutdown.clone())?;
            if let Err(e) = tracing::subscriber::set_global_default(subscriber)
            {
                eprintln!("Failed to set global default subscriber: {e}");
            }
            Ok::<_, anyhow::Error>((config, rx))
        });

    let (config, kvp_completion_rx) = match setup_result {
        Ok((config, rx)) => (config, rx),
        Err(error) => {
            eprintln!("Failed to load configuration: {error:?}");
            eprintln!("Example configuration:\n\n{}", Config::default());

            // Build temporary config to pass in wireserver defaults for report_failure
            let cfg = Config::default();

            let err = LibError::LoadSshdConfig {
                details: format!("{error:?}"),
            };

            // Report the failure to the health endpoint
            let report_str = err.as_encoded_report(&vm_id);
            let report_result = report_failure(report_str, &cfg).await;

            if let Err(report_error) = report_result {
                tracing::warn!(
                    "Failed to send provisioning failure report: {:?}",
                    report_error
                );
            }

            return ExitCode::FAILURE;
        }
    };

    let azure_init_run_span = tracing::info_span!("azure_init_run");
    let _enter_azure_init_run = azure_init_run_span.enter();

    tracing::info!(
        target = "libazureinit::config::success",
        "Final configuration: {:#?}",
        config
    );

    let exit_code = if let Some(Command::Clean { logs }) = opts.command {
        if clean_provisioning_status(&config).is_err()
            || (logs && clean_log_file(&config).is_err())
        {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    } else if is_provisioning_complete(Some(&config), &vm_id) {
        tracing::info!(
            "Provisioning already completed earlier. Skipping provisioning."
        );
        ExitCode::SUCCESS
    } else {
        let clone_config = config.clone();
        match provision(config, &vm_id, opts).await {
            Ok(_) => {
                let report_result =
                    report_ready(&clone_config, &vm_id, None).await;

                if let Err(report_error) = report_result {
                    tracing::warn!(
                        "Failed to send provisioning success report: {:?}",
                        report_error
                    );
                }

                tracing::info!("Provisioning completed successfully");

                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e:?}");

                let report_str = e
                    .downcast_ref::<LibError>()
                    .map(|lib_error| lib_error.as_encoded_report(&vm_id))
                    .unwrap_or_else(|| {
                        LibError::UnhandledError {
                            details: format!("{e:?}"),
                        }
                        .as_encoded_report(&vm_id)
                    });
                let report_result =
                    report_failure(report_str, &clone_config).await;

                if let Err(report_error) = report_result {
                    tracing::warn!(
                        "Failed to send provisioning failure report: {:?}",
                        report_error
                    );
                }

                tracing::error!("Provisioning failed with error: {e:?}");

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
    };

    if let Some(handle) = kvp_completion_rx {
        graceful_shutdown.cancel();

        match handle.await {
            Ok(Ok(_)) => {
                tracing::info!("KVP writer task finished successfully.");
            }
            Ok(Err(io_err)) => {
                tracing::warn!(
                    "KVP writer task finished with an IO error: {:?}",
                    io_err
                );
            }
            Err(join_err) => {
                tracing::warn!("KVP writer task panicked: {:?}", join_err);
            }
        }
    }
    exit_code
}

#[instrument(skip_all, err)]
async fn provision(
    config: Config,
    vm_id: &str,
    opts: Cli,
) -> Result<(), anyhow::Error> {
    let kernel_version = System::kernel_version()
        .unwrap_or("Unknown Kernel Version".to_string());
    let os_version =
        System::os_version().unwrap_or("Unknown OS Version".to_string());

    let build_version = version_string();

    tracing::info!(
        "Kernel Version: {}, OS Version: {}, Azure-Init Version: {}",
        kernel_version,
        os_version,
        build_version
    );

    let clone_config = config.clone();

    let mut default_headers = header::HeaderMap::new();
    let user_agent = format!("azure-init v{build_version}");
    let user_agent = header::HeaderValue::from_str(user_agent.as_str())?;
    default_headers.insert(header::USER_AGENT, user_agent);
    let client = Client::builder()
        .connect_timeout(Duration::from_secs_f64(
            config.imds.connection_timeout_secs,
        ))
        .default_headers(default_headers)
        .build()?;

    // Username can be obtained either via fetching instance metadata from IMDS
    // or mounting a local device for OVF environment file. It should not fail
    // immediately in a single failure, instead it should fall back to the other
    // mechanism. So it is not a good idea to use `?` for query() or
    // get_environment().
    let instance_metadata = query(
        &client,
        Some(&clone_config),
        None, // default IMDS URL
    )
    .await
    .ok();

    let environment = get_environment().ok();

    // The username is required for provisioning. This attempts to get the username
    // first from the IMDS metadata, falling back to the OVF environment if
    // IMDS is unavailable. If neither source can provide a username, provisioning fails.
    let username =
        get_username(instance_metadata.as_ref(), environment.as_ref())?;

    // It is necessary to get the actual instance metadata after getting username,
    // as it is not desirable to immediately return error before get_username.
    let im = instance_metadata
        .clone()
        .ok_or::<LibError>(LibError::InstanceMetadataFailure)?;

    // Create the user with the public SSH keys provided in the IMDS metadata.
    // The `disable_password_authentication` flag from IMDS controls whether
    // password-based SSH authentication is disabled in the sshd_config.
    let user =
        User::new(username, im.compute.public_keys).with_groups(opts.groups);

    Provision::new(
        im.compute.os_profile.computer_name,
        user,
        config,
        im.compute.os_profile.disable_password_authentication, // from IMDS: controls PasswordAuthentication
    )
    .provision()?;

    mark_provisioning_complete(Some(&clone_config), vm_id).with_context(
        || {
            tracing::error!("Failed to mark provisioning complete.");
            "Failed to mark provisioning complete."
        },
    )?;

    Ok(())
}
