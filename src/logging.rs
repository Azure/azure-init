// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Azure Init's stderr, file and KVP subscriber configuration.

mod tracing_layer;

use std::fs::{OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use self::tracing_layer::DiagnosticsKvp;
use libazureinit::config::Config;
use libazureinit_kvp::{
    DiagnosticWriter, KvpError, KvpPool, KvpPoolStore, PoolMode,
    PROVISIONING_REPORT_KEY,
};
use tracing::{field::Visit, Event, Level, Subscriber};
use tracing_subscriber::{
    filter::Targets, fmt, fmt::format::FmtSpan, layer::Context,
    layer::SubscriberExt, EnvFilter, Layer, Registry,
};

const AZURE_INIT_KVP_FILTER: &str = "AZURE_INIT_KVP_FILTER";

/// Target of the `health_report` event emitted by `libazureinit::health`.
const HEALTH_REPORT_TARGET: &str = "libazureinit::health::report";
const HEALTH_REPORT_FIELD: &str = "health_report";

/// Stores `health_report` events as `PROVISIONING_REPORT`, as the legacy KVP
/// layer did. Temporary until the provisioning report follow-up replaces it.
struct ProvisioningReportKvp {
    store: KvpPoolStore,
}

impl<S: Subscriber> Layer<S> for ProvisioningReportKvp {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut report = HealthReport(None);
        event.record(&mut report);
        if let Some(report) = report.0 {
            if let Err(error) = write_provisioning_report(&self.store, &report)
            {
                eprintln!("Failed to write KVP provisioning report: {error}");
            }
        }
    }
}

struct HealthReport(Option<String>);

impl Visit for HealthReport {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == HEALTH_REPORT_FIELD {
            self.0 = Some(value.to_owned());
        }
    }

    fn record_debug(
        &mut self,
        field: &tracing::field::Field,
        value: &dyn std::fmt::Debug,
    ) {
        if field.name() == HEALTH_REPORT_FIELD {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

/// Replaces the pool's report, truncated to one value like the legacy
/// writer's first chunk. Appends when the pool has no room for a new key.
fn write_provisioning_report(
    store: &KvpPoolStore,
    report: &str,
) -> Result<(), KvpError> {
    let mut end = report.len().min(store.max_value_size());
    while !report.is_char_boundary(end) {
        end -= 1;
    }
    let report = &report[..end];
    match store.insert(PROVISIONING_REPORT_KEY, report) {
        Err(KvpError::MaxUniqueKeysExceeded { .. }) => {
            store.append(PROVISIONING_REPORT_KEY, report)
        }
        result => result,
    }
}

pub(crate) struct LoggingSetup {
    pub(crate) subscriber: Box<dyn Subscriber + Send + Sync>,
}

/// The console/file filter, honoring `AZURE_INIT_LOG` with the given default.
fn output_filter(default: &str) -> EnvFilter {
    EnvFilter::try_from_env("AZURE_INIT_LOG")
        .unwrap_or_else(|_| EnvFilter::new(default))
}

/// Default KVP filter with azure-init-specific target overrides.
fn default_kvp_filter() -> EnvFilter {
    EnvFilter::builder()
        .parse(
            [
                "WARN",
                "azure_init=INFO",
                "libazureinit=INFO",
                "libazureinit::config::success",
                "libazureinit::http::received",
                "libazureinit::http::success",
                "libazureinit::ssh::authorized_keys",
                "libazureinit::ssh::success",
                "libazureinit::user::add",
                "libazureinit::status::success",
                "libazureinit::status::retrieved_vm_id",
                "libazureinit::health::status",
                "libazureinit::health::report",
                "libazureinit::password::status",
            ]
            .join(","),
        )
        .expect("the default KVP filter is valid")
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
    setup_layers_in(vm_id, config, KvpPool::default_dir())
}

/// Like [`setup_layers`], but reads the KVP pool from `kvp_dir` instead of the
/// system default, allowing tests to exercise KVP setup against a temp dir.
fn setup_layers_in(
    vm_id: &str,
    config: &Config,
    kvp_dir: &Path,
) -> LoggingSetup {
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
                .with_filter(output_filter("info")),
        ),
        Err(error) => {
            eprintln!(
                "Could not prepare log file {}: {error}. Continuing without file logging.",
                config.azure_init_log_path.path.display()
            );
            None
        }
    };

    let kvp_store = if config.telemetry.kvp_diagnostics {
        match KvpPoolStore::new_in(KvpPool::Guest, kvp_dir, PoolMode::Safe)
            .and_then(|store| {
                store.clear_if_stale()?;
                Ok(store)
            }) {
            Ok(store) => Some(store),
            Err(error) => {
                eprintln!("Failed to prepare KVP storage: {error}. Continuing without KVP telemetry.");
                None
            }
        }
    } else {
        None
    };

    let kvp_layer = kvp_store.clone().and_then(|store| {
        match DiagnosticWriter::new(
            store,
            concat!("azure-init/", env!("CARGO_PKG_VERSION")),
            vm_id,
        ) {
            Ok(writer) => Some(DiagnosticsKvp::new(writer).with_filter(
                kvp_filter(config.telemetry.kvp_filter.as_deref()),
            )),
            Err(error) => {
                eprintln!("Failed to initialize KVP diagnostics: {error}. Continuing without KVP diagnostics.");
                None
            }
        }
    });

    // The report bypasses the configurable KVP filter, which only governs
    // diagnostic history.
    let report_layer = kvp_store.map(|store| {
        ProvisioningReportKvp { store }.with_filter(
            Targets::new().with_target(HEALTH_REPORT_TARGET, Level::INFO),
        )
    });

    let subscriber = Registry::default()
        .with(stderr_layer)
        .with(file_layer)
        .with(kvp_layer)
        .with(report_layer);

    LoggingSetup {
        subscriber: Box::new(subscriber),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libazureinit_kvp::{
        Diagnostic, DiagnosticPayload, DiagnosticReader, Entry, Outcome,
        PROVISIONING_REPORT_KEY,
    };
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
        let default = default_kvp_filter().to_string();
        let cases = [
            (None, None, default.as_str()),
            (Some("trace"), Some("warn"), "trace"),
            (Some("off"), Some("trace"), "off"),
            (Some(""), Some("warn"), "warn"),
            (Some(" \t "), Some("debug"), "debug"),
            (Some("target=invalid"), Some("error"), "error"),
            (None, Some(" debug "), "debug"),
            (None, Some(""), default.as_str()),
            (None, Some(" \t "), default.as_str()),
            (None, Some("target=invalid"), default.as_str()),
            (Some("target=invalid"), None, default.as_str()),
            (
                Some("target=invalid"),
                Some("target=invalid"),
                default.as_str(),
            ),
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
    fn default_logging_filters_preserve_main_behavior() {
        const CASE_ENV: &str = "AZURE_INIT_TEST_DEFAULT_LOGGING_FILTERS";
        if std::env::var_os(CASE_ENV).is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "logging::tests::default_logging_filters_preserve_main_behavior",
                    "--nocapture",
                ])
                .env(CASE_ENV, "1")
                .env_remove("AZURE_INIT_LOG")
                .env_remove(AZURE_INIT_KVP_FILTER)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            return;
        }

        let dir = TempDir::new().unwrap();
        let (config, kvp_dir) = kvp_config(&dir);
        let setup = setup_layers_in(VM_ID, &config, &kvp_dir);
        tracing::subscriber::with_default(setup.subscriber, || {
            tracing::info!(target: "azure_init", "default-agent-info");
            tracing::info!(target: "libazureinit", "default-library-info");
            tracing::debug!(target: "azure_init", "default-agent-debug");
            tracing::warn!(target: "dependency", "default-dependency-warn");
            tracing::info!(target: "dependency", "default-dependency-info");
            tracing::debug!(target: "libazureinit::http", "default-library-debug");
            tracing::trace!(target: "libazureinit::http::success", "default-target-trace");
        });

        let file =
            std::fs::read_to_string(&config.azure_init_log_path.path).unwrap();
        let store =
            KvpPoolStore::new_in(KvpPool::Guest, &kvp_dir, PoolMode::Safe)
                .unwrap();
        let kvp = store
            .dump()
            .unwrap()
            .into_iter()
            .map(|(_, value)| value)
            .collect::<Vec<_>>()
            .join("\n");
        for (message, in_file, in_kvp) in [
            ("default-agent-info", true, true),
            ("default-library-info", true, true),
            ("default-agent-debug", false, false),
            ("default-dependency-warn", true, true),
            ("default-dependency-info", true, false),
            ("default-library-debug", false, false),
            ("default-target-trace", false, true),
        ] {
            assert_eq!(file.contains(message), in_file, "{message}: file");
            assert_eq!(kvp.contains(message), in_kvp, "{message}: KVP");
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
            let kvp_dir = dir.path().join("kvp");
            std::fs::create_dir(&kvp_dir).unwrap();
            let store =
                KvpPoolStore::new_in(KvpPool::Guest, &kvp_dir, PoolMode::Safe)
                    .unwrap();
            store.append("sentinel", "untouched").unwrap();
            std::fs::File::open(store.path())
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH)
                .unwrap();
            let setup = setup_layers_in(VM_ID, &config, &kvp_dir);
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::read_to_string(path).unwrap(),
                if existing { "existing log\n" } else { "" }
            );
            tracing::subscriber::with_default(setup.subscriber, || {
                tracing::info!("KVP is disabled");
            });
            assert_eq!(
                store.dump().unwrap(),
                vec![("sentinel".to_owned(), "untouched".to_owned())]
            );
        }
    }

    #[test]
    fn unavailable_log_file_does_not_fail_local_only_setup() {
        let dir = TempDir::new().unwrap();
        let mut config = local_only_config(&dir);
        config.azure_init_log_path.path = dir.path().to_path_buf();
        let setup = setup_layers(VM_ID, &config);
        assert!(dir.path().is_dir());
        tracing::subscriber::with_default(setup.subscriber, || {
            tracing::error!(
                "[EXPECTED TEST ERROR] Verifying stderr logging when the log file is unavailable"
            );
        });
    }

    /// A local-only config with KVP diagnostics enabled, targeting `kvp_dir`.
    fn kvp_config(dir: &TempDir) -> (Config, std::path::PathBuf) {
        let mut config = local_only_config(dir);
        config.telemetry.kvp_diagnostics = true;
        let kvp_dir = dir.path().join("kvp");
        std::fs::create_dir(&kvp_dir).unwrap();
        (config, kvp_dir)
    }

    #[test]
    fn kvp_setup_writes_health_reports_as_provisioning_report() {
        let dir = TempDir::new().unwrap();
        let (mut config, kvp_dir) = kvp_config(&dir);
        // The report is written even when the diagnostic filter excludes it.
        config.telemetry.kvp_filter = Some("off".to_owned());
        let setup = setup_layers_in(VM_ID, &config, &kvp_dir);
        let store =
            KvpPoolStore::new_in(KvpPool::Guest, &kvp_dir, PoolMode::Safe)
                .unwrap();
        tracing::subscriber::with_default(setup.subscriber, || {
            // Covers string and `%` (Display) values, and events without one.
            tracing::info!(
                target: "libazureinit::health::report",
                health_report = "result=error|reason=first",
                "Provisioning report"
            );
            tracing::info!(
                target: "libazureinit::health::report",
                "No report field"
            );
            let report = "result=success".to_owned();
            tracing::info!(
                target: "libazureinit::health::report",
                health_report = %report,
                "Provisioning report: {report}"
            );
        });
        assert_eq!(
            store.dump().unwrap(),
            vec![(
                PROVISIONING_REPORT_KEY.to_owned(),
                "result=success".to_owned()
            )]
        );
    }

    #[test]
    fn provisioning_report_is_truncated_and_survives_a_full_pool() {
        let dir = TempDir::new().unwrap();
        let store =
            KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe)
                .unwrap();
        // A multibyte character straddles the value limit.
        let report =
            format!("{}\u{20ac}tail", "x".repeat(store.max_value_size() - 1));
        write_provisioning_report(&store, &report).unwrap();
        assert_eq!(
            store.read(PROVISIONING_REPORT_KEY).unwrap().unwrap(),
            "x".repeat(store.max_value_size() - 1)
        );

        store.clear().unwrap();
        store
            .append_multiple((0..1024).map(|i| (format!("key-{i}"), "v")))
            .unwrap();
        write_provisioning_report(&store, "result=success").unwrap();
        assert_eq!(
            store.read(PROVISIONING_REPORT_KEY).unwrap().as_deref(),
            Some("result=success")
        );
    }

    #[test]
    fn provisioning_report_write_failure_does_not_panic() {
        let dir = TempDir::new().unwrap();
        let (config, kvp_dir) = kvp_config(&dir);
        let setup = setup_layers_in(VM_ID, &config, &kvp_dir);
        let pool = kvp_dir.join(KvpPool::Guest.file_name());
        std::fs::create_dir(&pool).unwrap();
        tracing::subscriber::with_default(setup.subscriber, || {
            tracing::info!(
                target: "libazureinit::health::report",
                health_report = "result=success",
                "Provisioning report"
            );
        });
        assert!(pool.is_dir());
    }

    #[test]
    fn other_health_report_fields_stay_ordinary_diagnostics() {
        let dir = TempDir::new().unwrap();
        let (config, kvp_dir) = kvp_config(&dir);
        let setup = setup_layers_in(VM_ID, &config, &kvp_dir);
        let store =
            KvpPoolStore::new_in(KvpPool::Guest, &kvp_dir, PoolMode::Safe)
                .unwrap();
        tracing::subscriber::with_default(setup.subscriber, || {
            tracing::info_span!("operation").in_scope(|| {
                tracing::info!(
                    health_report = "result=success",
                    "An ordinary diagnostic event"
                );
            });
        });
        let entries = DiagnosticReader::new(store.clone()).entries().unwrap();
        assert!(matches!(
            entries.as_slice(),
            [Entry::Diagnostic(Diagnostic::Start(start)),
             Entry::Diagnostic(Diagnostic::Event(event)),
             Entry::Diagnostic(Diagnostic::Finish(finish))]
                if start.key.event_id == finish.key.event_id
                    && start.key.event_id != event.key.event_id
                    && finish.result == Outcome::Success
                    && matches!(&event.payload, DiagnosticPayload::Text(payload)
                        if payload.contains("\"health_report\":\"result=success\""))
        ));
        assert_eq!(store.read(PROVISIONING_REPORT_KEY).unwrap(), None);
    }

    #[test]
    fn kvp_setup_with_invalid_vm_id_preserves_local_logging() {
        let dir = TempDir::new().unwrap();
        let (config, kvp_dir) = kvp_config(&dir);
        let setup = setup_layers_in("not-a-uuid", &config, &kvp_dir);
        tracing::subscriber::with_default(setup.subscriber, || {
            tracing::info!("Local logging still works with an invalid VM ID");
        });
        let store =
            KvpPoolStore::new_in(KvpPool::Guest, &kvp_dir, PoolMode::Safe)
                .unwrap();
        assert!(store.dump().unwrap().is_empty());
        assert!(std::fs::read_to_string(&config.azure_init_log_path.path)
            .unwrap()
            .contains("Local logging still works with an invalid VM ID"));
    }

    #[test]
    fn kvp_setup_with_unusable_pool_preserves_local_logging() {
        let dir = TempDir::new().unwrap();
        let (config, kvp_dir) = kvp_config(&dir);
        let pool_path = kvp_dir.join(KvpPool::Guest.file_name());
        std::fs::create_dir(&pool_path).unwrap();
        let setup = setup_layers_in(VM_ID, &config, &kvp_dir);
        tracing::subscriber::with_default(setup.subscriber, || {
            tracing::info!("Local logging still works with an unusable pool");
        });
        assert!(pool_path.is_dir());
        assert!(std::fs::read_to_string(&config.azure_init_log_path.path)
            .unwrap()
            .contains("Local logging still works with an unusable pool"));
    }
}
