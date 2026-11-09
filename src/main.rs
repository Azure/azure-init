// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
mod logging;
mod tracing_layer;

use std::{future::Future, path::PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand};
use libazureinit::{
    config::{Config, Wireserver},
    error::Error as LibError,
    get_vm_id,
    imds::{query, InstanceMetadata},
    is_provisioning_complete, mark_provisioning_complete,
    media::{get_mount_device, mount_parse_ovf_env, Environment},
    reqwest::{header, Client},
    wireserver, Provision, User,
};
use libazureinit_kvp::{
    write_report, KvpPoolStore, ProvisioningReport, ReportPpsType,
};
use std::process::ExitCode;
use std::time::Duration;
use sysinfo::System;
use tracing::instrument;
use tracing_subscriber::{prelude::*, Layer};

const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPORT_AGENT: &str = concat!("Azure-Init/", env!("CARGO_PKG_VERSION"));

fn version_string() -> String {
    if let Some(v) = option_env!("AZURE_INIT_VERSION") {
        return v.to_string();
    }

    if let Some(build) = option_env!("AZURE_INIT_BUILD_VERSION") {
        return format!("{PKG_VERSION}-{build}");
    }
    if let Some(sha) = option_env!("AZURE_INIT_BUILD_SHA") {
        let short = &sha[..std::cmp::min(7, sha.len())];
        return format!("{PKG_VERSION}-{short}");
    }

    PKG_VERSION.to_string()
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
///
/// If cleanup fails while reading the provisioning directory, its path and
/// the original I/O error are logged before returning the error.
#[instrument]
fn clean_provisioning_status(config: &Config) -> Result<(), std::io::Error> {
    let data_dir = &config.azure_init_data_dir.path;
    let mut found = false;

    let entries = match std::fs::read_dir(data_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(
                path = %data_dir.display(),
                "No provisioning state directory found; nothing to clean"
            );
            return Ok(());
        }
        Err(error) => {
            tracing::error!(
                path = %data_dir.display(),
                %error,
                "Failed to read provisioning state directory"
            );
            return Err(error);
        }
    };

    for entry in entries {
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

fn failure_report(error: &anyhow::Error, vm_id: &str) -> ProvisioningReport {
    error
        .downcast_ref::<LibError>()
        .map(|error| error.as_provisioning_report(vm_id))
        .unwrap_or_else(|| {
            LibError::UnhandledError {
                details: format!("{error:?}"),
            }
            .as_provisioning_report(vm_id)
        })
}

async fn write_kvp_report(
    store: &KvpPoolStore,
    report: &ProvisioningReport,
) -> Result<(), anyhow::Error> {
    let store = store.clone();
    let report = report.clone();
    tokio::task::spawn_blocking(move || write_report(&store, &report))
        .await??;
    Ok(())
}

/// Reporting failures never replace the actual provisioning result.
async fn provision_with_reporting(
    config: &Wireserver,
    store: Option<&KvpPoolStore>,
    vm_id: &str,
    provisioning: impl Future<Output = Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    let result = provisioning.await;
    let report = match &result {
        Ok(()) => ProvisioningReport::success(
            REPORT_AGENT,
            vm_id,
            ReportPpsType::None,
        ),
        Err(error) => {
            eprintln!("{error:?}");
            failure_report(error, vm_id)
        }
    };
    let kvp = async {
        if let Some(store) = store {
            if let Err(error) = write_kvp_report(store, &report).await {
                // Avoid sending a write failure back into the failing KVP sink.
                eprintln!(
                    "Failed to write provisioning report to KVP: {error:#}"
                );
            }
        }
    };
    let http = async {
        let sent = if result.is_ok() {
            wireserver::report_ready(config).await
        } else {
            wireserver::report_failure(&report, config).await
        };
        if let Err(error) = sent {
            tracing::warn!(
                "Failed to send provisioning report to wireserver: {error:?}"
            );
        }
    };
    tokio::join!(kvp, http);
    result
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
            let logging::LoggingSetup {
                subscriber,
                report_store,
            } = logging::setup_layers(&vm_id, &config);
            if let Err(e) = tracing::subscriber::set_global_default(subscriber)
            {
                eprintln!("Failed to set global default subscriber: {e}");
            }
            Ok::<_, anyhow::Error>((config, report_store))
        });

    let (config, report_store) = match setup_result {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Failed to load configuration: {error:?}");
            eprintln!("Example configuration:\n\n{}", Config::default());

            let cfg = Wireserver::default();

            let err = LibError::LoadSshdConfig {
                details: format!("{error:?}"),
            };

            // Report the failure to the health endpoint
            let report = err.as_provisioning_report(&vm_id);
            let report_result = wireserver::report_failure(&report, &cfg).await;

            if let Err(report_error) = report_result {
                eprintln!(
                    "Failed to send provisioning failure report: {:?}",
                    report_error
                );
            }

            return ExitCode::FAILURE;
        }
    };

    tracing::info!("Final configuration: {:#?}", config);

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
        let wireserver = config.wireserver.clone();
        match provision_with_reporting(
            &wireserver,
            report_store.as_ref(),
            &vm_id,
            provision(config, &vm_id, opts),
        )
        .await
        {
            Ok(_) => {
                tracing::info!("Provisioning completed successfully");

                ExitCode::SUCCESS
            }
            Err(e) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use libazureinit::reqwest::StatusCode;
    use libazureinit_kvp::{
        DiagnosticReader, DiagnosticWriter, Entry, KvpPool, Outcome, PoolMode,
        PROVISIONING_REPORT_KEY,
    };
    use rstest::rstest;
    use serde_json::{json, Value};
    use std::io;
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tokio::time::timeout;

    const VM_ID: &str = "00000000-0000-0000-0000-000000000001";

    #[test]
    fn clean_reports_subscriber_conflict() {
        const CHILD: &str = "AZURE_INIT_TEST_MAIN_SUBSCRIBER_CONFLICT";
        if std::env::var_os(CHILD).is_some() {
            tracing::subscriber::set_global_default(
                tracing_subscriber::registry(),
            )
            .unwrap();
            assert_eq!(
                format!("{:?}", main()),
                format!("{:?}", ExitCode::SUCCESS)
            );
            return;
        }

        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.telemetry.kvp_diagnostics = false;
        config.azure_init_log_path.path = dir.path().join("azure-init.log");
        config.azure_init_data_dir.path = dir.path().join("state");
        std::fs::create_dir(&config.azure_init_data_dir.path).unwrap();
        let marker = config.azure_init_data_dir.path.join("test.provisioned");
        std::fs::write(&marker, "").unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, config.to_string()).unwrap();

        let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        proxy.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", proxy.local_addr().unwrap());
        // "clean" is both a libtest filter and a valid argument to the real CLI.
        assert_cmd::Command::new(std::env::current_exe().unwrap())
            .arg("clean")
            .env(CHILD, "1")
            .env("AZURE_INIT_CONFIG", &config_path)
            .env("AZURE_INIT_LOG", "off")
            .env("RUST_TEST_NOCAPTURE", "1")
            .env("RUST_TEST_THREADS", "1")
            .env("HTTP_PROXY", &endpoint)
            .env("http_proxy", &endpoint)
            .env("ALL_PROXY", &endpoint)
            .env("all_proxy", &endpoint)
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .timeout(Duration::from_secs(10))
            .assert()
            .success()
            .stderr(predicates::str::contains(
                "Failed to set global default subscriber:",
            ));
        assert!(!marker.exists(), "subscriber failure must not skip cleanup");
        assert_eq!(
            proxy.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock,
            "clean must not send a health report"
        );
    }

    struct CapturedRequest {
        request_line: String,
        headers: header::HeaderMap,
        body: Value,
    }

    async fn capture_request(
        stream: &mut TcpStream,
    ) -> io::Result<CapturedRequest> {
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await?;
        let mut headers = header::HeaderMap::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete HTTP headers",
                ));
            }
            if line == "\r\n" {
                break;
            }
            let (name, value) = line.split_once(':').ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid HTTP header",
                )
            })?;
            headers.append(
                header::HeaderName::from_bytes(name.as_bytes()).map_err(
                    |error| io::Error::new(io::ErrorKind::InvalidData, error),
                )?,
                header::HeaderValue::from_str(value.trim()).map_err(
                    |error| io::Error::new(io::ErrorKind::InvalidData, error),
                )?,
            );
        }
        let length = headers
            .get(header::CONTENT_LENGTH)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "missing content length",
                )
            })?
            .to_str()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .parse::<usize>()
            .map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, error)
            })?;
        let mut body = vec![0; length];
        reader.read_exact(&mut body).await?;
        Ok(CapturedRequest {
            request_line: request_line.trim_end().into(),
            headers,
            body: serde_json::from_slice(&body).map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, error)
            })?,
        })
    }

    #[rstest]
    #[case::incomplete_headers(
        b"POST / HTTP/1.1\r\nHost: test\r\n",
        io::ErrorKind::UnexpectedEof,
        |error: &io::Error| error.to_string() == "incomplete HTTP headers"
    )]
    #[case::malformed_header(
        b"POST / HTTP/1.1\r\nnot-a-header\r\n\r\n",
        io::ErrorKind::InvalidData,
        |error: &io::Error| error.to_string() == "invalid HTTP header"
    )]
    #[case::invalid_header_name(
        b"POST / HTTP/1.1\r\nbad name: value\r\n\r\n",
        io::ErrorKind::InvalidData,
        |error: &io::Error| error.get_ref().unwrap().is::<header::InvalidHeaderName>()
    )]
    #[case::invalid_header_value(
        b"POST / HTTP/1.1\r\nX-Test: bad\0value\r\n\r\n",
        io::ErrorKind::InvalidData,
        |error: &io::Error| error.get_ref().unwrap().is::<header::InvalidHeaderValue>()
    )]
    #[case::missing_length(
        b"POST / HTTP/1.1\r\nHost: test\r\n\r\n",
        io::ErrorKind::InvalidData,
        |error: &io::Error| error.to_string() == "missing content length"
    )]
    #[case::non_ascii_length(
        b"POST / HTTP/1.1\r\nContent-Length: \xc3\xa9\r\n\r\n",
        io::ErrorKind::InvalidData,
        |error: &io::Error| error.get_ref().unwrap().is::<header::ToStrError>()
    )]
    #[case::invalid_length(
        b"POST / HTTP/1.1\r\nContent-Length: nope\r\n\r\n",
        io::ErrorKind::InvalidData,
        |error: &io::Error| error.get_ref().unwrap().is::<std::num::ParseIntError>()
    )]
    #[case::invalid_json(
        b"POST / HTTP/1.1\r\nContent-Length: 1\r\n\r\n{",
        io::ErrorKind::InvalidData,
        |error: &io::Error| error.get_ref().unwrap().is::<serde_json::Error>()
    )]
    #[tokio::test]
    async fn capture_request_rejects_invalid_requests(
        #[case] request: &[u8],
        #[case] kind: io::ErrorKind,
        #[case] matches_error: fn(&io::Error) -> bool,
    ) {
        let error = timeout(Duration::from_secs(5), async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut client = TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (mut server, _) = listener.accept().await.unwrap();
            client.write_all(request).await.unwrap();
            client.shutdown().await.unwrap();
            capture_request(&mut server).await
        })
        .await
        .expect("request capture hung")
        .err()
        .expect("invalid request was accepted");
        assert_eq!(error.kind(), kind);
        assert!(matches_error(&error), "unexpected error: {error:?}");
    }

    fn wireserver_config(listener: &TcpListener) -> Wireserver {
        Wireserver {
            connection_timeout_secs: 0.1,
            read_timeout_secs: 1.0,
            total_retry_timeout_secs: 2.0,
            health_endpoint: format!(
                "http://{}/provisioning/health",
                listener.local_addr().unwrap()
            ),
        }
    }

    async fn respond(stream: &mut TcpStream, status: StatusCode) {
        stream.write_all(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
        ).await.unwrap();
    }

    async fn mock_wireserver(
        statuses: Vec<StatusCode>,
    ) -> (Wireserver, JoinHandle<Vec<CapturedRequest>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = wireserver_config(&listener);
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for status in statuses {
                let request = timeout(Duration::from_secs(5), async {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let request = capture_request(&mut stream).await.unwrap();
                    respond(&mut stream, status).await;
                    request
                })
                .await
                .expect("health request did not complete");
                requests.push(request);
            }
            requests
        });
        (config, server)
    }

    fn store_at(dir: &TempDir) -> KvpPoolStore {
        KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe)
            .unwrap()
    }

    fn stored_report(store: &KvpPoolStore) -> ProvisioningReport {
        store
            .read(PROVISIONING_REPORT_KEY)
            .unwrap()
            .unwrap()
            .parse()
            .unwrap()
    }

    fn assert_request(request: &CapturedRequest, body: Value) {
        assert_eq!(request.request_line, "POST /provisioning/health HTTP/1.1");
        let agent = format!("azure-init v{}", libazureinit::VERSION);
        assert_eq!(request.headers[header::USER_AGENT], agent);
        assert_eq!(request.headers["x-ms-guest-agent-name"], agent);
        assert_eq!(request.headers[header::CONTENT_TYPE], "application/json");
        assert_eq!(request.body, body);
    }

    #[rstest]
    #[case::success(false)]
    #[case::failure(true)]
    #[tokio::test]
    async fn health_lifecycle_publishes_actual_result(#[case] failed: bool) {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        let previous = LibError::UnhandledError {
            details: "old context".into(),
        }
        .as_provisioning_report(VM_ID);
        write_report(&store, &previous).unwrap();
        let (config, server) = mock_wireserver(vec![StatusCode::OK]).await;
        let result =
            provision_with_reporting(&config, Some(&store), VM_ID, async {
                if failed {
                    Err(LibError::UserMissing {
                        user: "new|\"user\"".into(),
                    })
                    .context("provisioning context")
                } else {
                    Ok(())
                }
            })
            .await;
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        let report = stored_report(&store);
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(store.len().unwrap(), 1);
        assert_eq!(value["agent"], REPORT_AGENT);
        assert_eq!(value["vm_id"], VM_ID);
        assert_eq!(value["pps_type"], "None");
        if failed {
            let error = result.unwrap_err();
            assert!(matches!(
                error.downcast_ref::<LibError>(),
                Some(LibError::UserMissing { .. })
            ));
            assert_eq!(error.to_string(), "provisioning context");
            assert_eq!(value["result"], "error");
            assert_eq!(value["extra"], json!([["user", "new|\"user\""]]));
            assert_request(
                &requests[0],
                json!({
                    "state": "NotReady",
                    "details": {
                        "subStatus": "ProvisioningFailed",
                        "description": report.encode(),
                    },
                }),
            );
        } else {
            result.unwrap();
            assert_eq!(value["result"], "success");
            assert!(value.get("extra").is_none());
            assert_request(&requests[0], json!({"state": "Ready"}));
        }
    }

    #[rstest]
    #[case::http_failure(true, false, StatusCode::FORBIDDEN)]
    #[case::kvp_failure(true, true, StatusCode::OK)]
    #[case::both_failed(true, true, StatusCode::FORBIDDEN)]
    #[case::kvp_disabled(false, false, StatusCode::OK)]
    #[tokio::test]
    async fn health_delivery_failures_preserve_provisioning_result(
        #[case] enabled: bool,
        #[case] broken: bool,
        #[case] status: StatusCode,
        #[values(false, true)] failed: bool,
    ) {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        if broken {
            std::fs::create_dir(store.path()).unwrap();
        }
        let (config, server) = mock_wireserver(vec![status]).await;
        let result = provision_with_reporting(
            &config,
            enabled.then_some(&store),
            VM_ID,
            async {
                if failed {
                    Err(LibError::NonEmptyPassword.into())
                } else {
                    Ok(())
                }
            },
        )
        .await;
        let requests = server.await.unwrap();
        assert_eq!(
            requests.len(),
            1,
            "HTTP must be attempted despite KVP failure/disablement"
        );
        assert_eq!(
            requests[0].body["state"],
            if failed { "NotReady" } else { "Ready" }
        );
        if failed {
            assert!(matches!(
                result.unwrap_err().root_cause().downcast_ref::<LibError>(),
                Some(LibError::NonEmptyPassword)
            ));
        } else {
            result.unwrap();
        }
        if enabled && !broken {
            let report = serde_json::to_value(stored_report(&store)).unwrap();
            assert_eq!(
                report["result"],
                if failed { "error" } else { "success" }
            );
        } else if broken {
            assert!(store.path().is_dir());
        } else {
            assert!(!store.path().exists());
        }
    }

    #[tokio::test]
    async fn health_report_survives_diagnostic_errors() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        let (config, server) = mock_wireserver(vec![StatusCode::OK]).await;
        provision_with_reporting(&config, Some(&store), VM_ID, async {
            let writer = DiagnosticWriter::new(store.clone(), "agent", VM_ID).unwrap();
            let subscriber = tracing_subscriber::registry()
                .with(crate::tracing_layer::DiagnosticsKvp::new(writer));
            tracing::subscriber::with_default(subscriber, || {
                tracing::info_span!("recovered_attempt").in_scope(|| {
                    tracing::error!("A recovered diagnostic failure is not the final outcome");
                });
            });
            Ok(())
        }).await.unwrap();
        server.await.unwrap();
        let records = store.dump().unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(
            serde_json::to_value(stored_report(&store)).unwrap()["result"],
            "success"
        );
        assert!(DiagnosticReader::new(store).entries().unwrap().iter().any(|entry| {
            matches!(entry, Entry::Diagnostic(libazureinit_kvp::Diagnostic::Finish(finish))
                if finish.result == Outcome::Error)
        }));
    }

    #[tokio::test]
    async fn full_pool_does_not_prevent_provisioning_or_http() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        store
            .append_multiple((0..1024).map(|i| (format!("key-{i}"), "keep")))
            .unwrap();
        let before = store.dump().unwrap();
        let (config, server) = mock_wireserver(vec![StatusCode::OK]).await;
        provision_with_reporting(&config, Some(&store), VM_ID, async {
            assert_eq!(store.read(PROVISIONING_REPORT_KEY).unwrap(), None);
            Ok(())
        })
        .await
        .unwrap();
        let requests = server.await.unwrap();
        assert_request(&requests[0], json!({"state": "Ready"}));
        assert_eq!(store.dump().unwrap(), before);
    }

    #[tokio::test]
    async fn oversized_failure_reaches_http_without_truncating_kvp() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        let details = "large | \"quoted\"\u{20ac}".repeat(100);
        let (config, server) = mock_wireserver(vec![StatusCode::OK]).await;
        let result =
            provision_with_reporting(&config, Some(&store), VM_ID, async {
                Err(LibError::UnhandledError {
                    details: details.clone(),
                }
                .into())
            })
            .await;
        assert!(result.is_err());
        let requests = server.await.unwrap();
        let description =
            requests[0].body["details"]["description"].as_str().unwrap();
        assert!(description.len() > store.max_value_size());
        let report: ProvisioningReport = description.parse().unwrap();
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["result"], "error");
        assert_eq!(
            value["extra"],
            json!([["error", format!("unhandled error: {details}")]])
        );
        assert_eq!(store.read(PROVISIONING_REPORT_KEY).unwrap(), None);
        assert_eq!(store.len().unwrap(), 0);
    }

    #[tokio::test]
    async fn http_retries_reuse_the_final_kvp_report() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        let (config, server) = mock_wireserver(vec![
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::CREATED,
        ])
        .await;
        let result =
            provision_with_reporting(&config, Some(&store), VM_ID, async {
                Err(LibError::UnhandledError {
                    details: "retry | \"context\"".into(),
                }
                .into())
            })
            .await;
        assert!(result.is_err());
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        let encoded = stored_report(&store).encode();
        for request in &requests {
            assert_request(
                request,
                json!({
                    "state": "NotReady",
                    "details": {"subStatus": "ProvisioningFailed", "description": encoded},
                }),
            );
        }
    }

    #[rstest]
    #[case::missing_report(false)]
    #[case::previous_failure(true)]
    #[tokio::test]
    async fn final_kvp_report_does_not_wait_for_http_response(
        #[case] previous_failure: bool,
    ) {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        if previous_failure {
            write_report(
                &store,
                &ProvisioningReport::failure(
                    REPORT_AGENT,
                    VM_ID,
                    "previous failure",
                    ReportPpsType::None,
                ),
            )
            .unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = Wireserver {
            read_timeout_secs: 10.0,
            total_retry_timeout_secs: 10.0,
            ..wireserver_config(&listener)
        };
        let mut waiting = std::pin::pin!(async {
            loop {
                if let Some(value) =
                    store.read(PROVISIONING_REPORT_KEY).unwrap()
                {
                    let report: ProvisioningReport = value.parse().unwrap();
                    if serde_json::to_value(report).unwrap()["result"]
                        == "success"
                    {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        std::future::poll_fn(|context| {
            assert!(waiting.as_mut().poll(context).is_pending());
            std::task::Poll::Ready(())
        })
        .await;

        let report_store = store.clone();
        let reporting = tokio::spawn(async move {
            provision_with_reporting(
                &config,
                Some(&report_store),
                VM_ID,
                async { Ok(()) },
            )
            .await
        });
        let (mut stream, _) =
            timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
        let request = capture_request(&mut stream).await.unwrap();
        timeout(Duration::from_secs(5), waiting)
            .await
            .expect("KVP publication waited for the held HTTP response");
        assert!(!reporting.is_finished());
        assert_request(&request, json!({"state": "Ready"}));
        respond(&mut stream, StatusCode::OK).await;
        reporting.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn progress_http_preserves_its_contract() {
        let (config, server) = mock_wireserver(vec![StatusCode::OK]).await;
        wireserver::report_in_progress(&config, VM_ID)
            .await
            .unwrap();
        let requests = server.await.unwrap();
        assert_request(
            &requests[0],
            json!({
                "state": "NotReady",
                "details": {
                    "subStatus": "Provisioning",
                    "description": format!("Provisioning is still in progress for vm_id={VM_ID}."),
                },
            }),
        );
    }

    #[test]
    fn failure_report_preserves_typed_errors_and_falls_back_to_unhandled() {
        for (error, reason) in [
            (
                anyhow::Error::from(LibError::UserMissing {
                    user: "user".into(),
                })
                .context("outer context"),
                "user not found",
            ),
            (
                anyhow::anyhow!("unexpected failure").context("outer context"),
                "unhandled error",
            ),
        ] {
            let report = failure_report(&error, VM_ID);
            assert_eq!(
                report.encode().parse::<ProvisioningReport>().unwrap(),
                report
            );
            let json = serde_json::to_value(report).unwrap();
            assert_eq!(json["reason"], reason);
            if reason == "unhandled error" {
                assert_eq!(
                    json["extra"],
                    json!([["error", format!("unhandled error: {error:?}")]])
                );
            }
        }
    }

    #[test]
    fn kvp_errors_are_visible_and_do_not_stop_reporting() {
        const CHILD: &str = "AZURE_INIT_TEST_REPORT_ERRORS";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(
                std::env::current_exe().unwrap(),
            )
            .args([
                "--exact",
                "tests::kvp_errors_are_visible_and_do_not_stop_reporting",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
            assert!(output.status.success(), "{output:?}");
            let stderr = String::from_utf8(output.stderr).unwrap();
            for message in [
                "Failed to write provisioning report to KVP:",
                "Failed to send provisioning report to wireserver:",
            ] {
                assert_eq!(stderr.matches(message).count(), 1, "{stderr}");
            }
            return;
        }
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(std::io::stderr)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let dir = TempDir::new().unwrap();
                let store = store_at(&dir);
                std::fs::create_dir(store.path()).unwrap();
                let (config, server) =
                    mock_wireserver(vec![StatusCode::FORBIDDEN]).await;
                provision_with_reporting(&config, Some(&store), VM_ID, async {
                    Ok(())
                })
                .await
                .unwrap();
                assert_eq!(server.await.unwrap().len(), 1);
            });
    }
}
