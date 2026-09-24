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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    const VM_ID: &str = "00000000-0000-0000-0000-000000000001";

    fn local_only_config(dir: &TempDir) -> Config {
        let mut config = Config::default();
        config.telemetry.kvp_diagnostics = false;
        config.azure_init_log_path.path = dir.path().join("azure-init.log");
        config
    }

    #[test]
    fn kvp_filter_precedence_and_fallbacks() {
        const CASE_ENV: &str = "AZURE_INIT_TEST_KVP_FILTER_CASE";
        let cases = [
            (None, None, "info"),
            (Some("trace"), Some("warn"), "trace"),
            (Some("off"), Some("trace"), "off"),
            (Some(""), Some("warn"), "warn"),
            (Some(" \t "), Some("debug"), "debug"),
            (Some("target=invalid"), Some("error"), "error"),
            (None, Some(" debug "), "debug"),
            (None, Some(""), "info"),
            (None, Some(" \t "), "info"),
            (None, Some("target=invalid"), "info"),
            (Some("target=invalid"), None, "info"),
            (Some("target=invalid"), Some("target=invalid"), "info"),
            (
                None,
                Some("warn,libazureinit=debug"),
                "libazureinit=debug,warn",
            ),
        ];
        if let Ok(case) = std::env::var(CASE_ENV) {
            let (_, config, expected) = cases[case.parse::<usize>().unwrap()];
            assert_eq!(kvp_filter(config).to_string(), expected);
        } else {
            // Each case gets its own process environment, leaving parallel tests untouched.
            for (index, (env, _, _)) in cases.iter().enumerate() {
                let mut child = Command::new(std::env::current_exe().unwrap());
                child
                    .args([
                        "--exact",
                        "logging::tests::kvp_filter_precedence_and_fallbacks",
                        "--nocapture",
                    ])
                    .env(CASE_ENV, index.to_string())
                    .env_remove(AZURE_INIT_KVP_FILTER);
                if let Some(filter) = env {
                    child.env(AZURE_INIT_KVP_FILTER, filter);
                }
                let output = child.output().unwrap();
                assert!(output.status.success(), "case {index}: {output:?}");
            }
        }
    }

    #[test]
    fn disabled_kvp_setup_preserves_logs_and_enforces_private_permissions() {
        for existing in [false, true] {
            let dir = TempDir::new().unwrap();
            let config = local_only_config(&dir);
            let path = &config.azure_init_log_path.path;
            if existing {
                std::fs::write(path, "existing log\n").unwrap();
                std::fs::set_permissions(path, Permissions::from_mode(0o644))
                    .unwrap();
            }
            let setup = setup_layers(VM_ID, &config);
            assert!(setup.report_store.is_none());
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::read_to_string(path).unwrap(),
                if existing { "existing log\n" } else { "" }
            );
        }
    }

    #[test]
    fn unavailable_log_file_does_not_fail_local_only_setup() {
        let dir = TempDir::new().unwrap();
        let mut config = local_only_config(&dir);
        config.azure_init_log_path.path = dir.path().to_path_buf();
        let setup = setup_layers(VM_ID, &config);
        assert!(setup.report_store.is_none());
        assert!(dir.path().is_dir());
        tracing::subscriber::with_default(setup.subscriber, || {
            tracing::error!(
                "[EXPECTED TEST ERROR] Verifying stderr logging when the log file is unavailable"
            );
        });
    }
}
