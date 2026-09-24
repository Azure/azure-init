// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Azure Init's stderr, file and KVP subscriber configuration.

use std::fs::{OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use libazureinit::config::Config;
use libazureinit_kvp::{
    DiagnosticWriter, DiagnosticsKvp, KvpPool, KvpPoolStore, PoolMode,
};
use tracing::{Dispatch, Subscriber};
use tracing_subscriber::{
    fmt, fmt::format::FmtSpan, layer::SubscriberExt, EnvFilter, Layer, Registry,
};

const AZURE_INIT_KVP_FILTER: &str = "AZURE_INIT_KVP_FILTER";

pub(crate) struct LoggingSetup {
    pub(crate) subscriber: Box<dyn Subscriber + Send + Sync>,
    /// Final-report store, present when KVP is enabled and cleanup succeeds.
    pub(crate) report_store: Option<KvpPoolStore>,
}

/// A minimal stderr subscriber for use before the configured layers exist.
pub(crate) fn bootstrap() -> Dispatch {
    Dispatch::new(
        Registry::default().with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(output_filter("info")),
        ),
    )
}

/// The console/file filter, honoring `AZURE_INIT_LOG` with the given default.
fn output_filter(default: &str) -> EnvFilter {
    EnvFilter::try_from_env("AZURE_INIT_LOG")
        .unwrap_or_else(|_| EnvFilter::new(default))
}

/// azure-init's default KVP filter: INFO and above (INFO, WARN, ERROR).
fn default_kvp_filter() -> EnvFilter {
    EnvFilter::new("info")
}

/// Tries environment, configuration, then default, skipping invalid values.
fn kvp_filter(config_filter: Option<&str>) -> EnvFilter {
    if let Some(filter) = std::env::var(AZURE_INIT_KVP_FILTER)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        match EnvFilter::builder().parse(&filter) {
            Ok(filter) => return filter,
            Err(error) => eprintln!(
                "Invalid {AZURE_INIT_KVP_FILTER} ({error}); trying config then default."
            ),
        }
    }
    if let Some(filter) = config_filter
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        match EnvFilter::builder().parse(filter) {
            Ok(filter) => return filter,
            Err(error) => eprintln!(
                "Invalid telemetry.kvp_filter ({error}); using the default KVP filter."
            ),
        }
    }
    default_kvp_filter()
}

pub(crate) fn setup_layers(vm_id: &str, config: &Config) -> LoggingSetup {
    let stderr_layer = fmt::layer()
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_writer(std::io::stderr)
        .with_filter(output_filter("error"));

    let file_layer = match OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&config.azure_init_log_path.path)
        .and_then(|file| {
            file.set_permissions(Permissions::from_mode(0o600))?;
            Ok(file)
        }) {
        Ok(file) => Some(
            fmt::layer()
                .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
                .with_writer(file)
                .with_filter(output_filter("debug")),
        ),
        Err(error) => {
            eprintln!(
                "Could not prepare log file {}: {error}. Continuing without file logging.",
                config.azure_init_log_path.path.display()
            );
            None
        }
    };

    // The report is written through the store directly, so it still publishes
    // when a non-UUID vm_id prevents the diagnostics writer from initializing.
    let mut report_store = None;
    let kvp_layer = if config.telemetry.kvp_diagnostics {
        match KvpPoolStore::new(KvpPool::Guest, PoolMode::Safe).and_then(
            |store| {
                store.clear_if_stale()?;
                Ok(store)
            },
        ) {
            Ok(store) => {
                report_store = Some(store.clone());
                match DiagnosticWriter::new(
                    store,
                    concat!("azure-init/", env!("CARGO_PKG_VERSION")),
                    vm_id,
                ) {
                    Ok(writer) => {
                        Some(DiagnosticsKvp::new(writer).with_filter(
                            kvp_filter(config.telemetry.kvp_filter.as_deref()),
                        ))
                    }
                    Err(error) => {
                        eprintln!("Failed to initialize KVP diagnostics: {error}. Provisioning reports remain enabled.");
                        None
                    }
                }
            }
            Err(error) => {
                eprintln!("Failed to prepare KVP storage: {error}. Continuing without KVP telemetry.");
                None
            }
        }
    } else {
        None
    };

    let subscriber = Registry::default()
        .with(stderr_layer)
        .with(file_layer)
        .with(kvp_layer);

    LoggingSetup {
        subscriber: Box::new(subscriber),
        report_store,
    }
}
