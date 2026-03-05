// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use opentelemetry::{global, trace::TracerProvider};
use opentelemetry_sdk::trace::{self as sdktrace, Sampler, SdkTracerProvider};
use std::fs::{OpenOptions, Permissions};
use std::os::unix::fs::PermissionsExt;
use tracing::{event, Level, Subscriber};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{
    fmt, layer::SubscriberExt, EnvFilter, Layer, Registry,
};

use crate::config::Config;
pub use libazureinit_kvp::{Kvp, KvpOptions};

fn initialize_tracing() -> sdktrace::Tracer {
    let provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .build();

    let tracer = provider.tracer("azure-kvp");
    global::set_tracer_provider(provider);
    tracer
}

fn default_kvp_filter() -> Result<EnvFilter, anyhow::Error> {
    Ok(EnvFilter::builder().parse(
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
    )?)
}

fn get_kvp_filter(
    config_filter: Option<&str>,
) -> Result<EnvFilter, anyhow::Error> {
    match std::env::var("AZURE_INIT_KVP_FILTER") {
        Ok(filter) if !filter.is_empty() => {
            tracing::info!(
                "Using KVP filter override from environment variable '{}': '{}'",
                "AZURE_INIT_KVP_FILTER",
                filter
            );
            match EnvFilter::builder().parse(filter) {
                Ok(f) => Ok(f),
                Err(e) => {
                    tracing::warn!(
                        "Invalid '{}' value, falling back to {} filter: {}",
                        "AZURE_INIT_KVP_FILTER",
                        if config_filter.is_some() {
                            "config"
                        } else {
                            "default"
                        },
                        e
                    );
                    // Try config filter if provided; otherwise use default
                    if let Some(cfg) = config_filter {
                        if !cfg.trim().is_empty() {
                            return EnvFilter::builder()
                                .parse(cfg)
                                .map_err(anyhow::Error::from)
                                .or_else(|parse_err| {
                                    tracing::warn!(
                                        "Invalid config kvp_filter, falling back to default: {}",
                                        parse_err
                                    );
                                    default_kvp_filter()
                                });
                        }
                    }
                    default_kvp_filter()
                }
            }
        }
        _ => {
            // No env var set; try config if provided
            if let Some(cfg) = config_filter {
                let cfg = cfg.trim();
                if !cfg.is_empty() {
                    tracing::info!("Using KVP filter from config: '{}'", cfg);
                    return match EnvFilter::builder().parse(cfg) {
                        Ok(f) => Ok(f),
                        Err(e) => {
                            tracing::warn!(
                                "Invalid config kvp_filter, falling back to default: {}",
                                e
                            );
                            default_kvp_filter()
                        }
                    };
                }
            }
            tracing::info!("Using default KVP filter");
            default_kvp_filter()
        }
    }
}

/// Builds a `tracing` subscriber that can optionally write azure-init.log
/// to a specific location if `Some(&Config)` is provided.
///
/// This function follows a two-phase initialization:
/// - Minimal Setup (Pre-Config): When called initially, it sets up basic logging
///   to console (`stderr`), KVP (Hyper-V), and OpenTelemetry without file logging.
///
/// - Full Setup (Post-Config): After the configuration is loaded, it is called again
///   with `config`, adding file logging to `config.azure_init_log_path.path` or
///   falling back to `DEFAULT_AZURE_INIT_LOG_PATH` if unspecified.
pub fn setup_layers(
    vm_id: &str,
    config: &Config,
) -> Result<Box<dyn Subscriber + Send + Sync + 'static>, anyhow::Error> {
    let tracer = initialize_tracing();
    let otel_layer = OpenTelemetryLayer::new(tracer).with_filter(
        EnvFilter::try_from_env("AZURE_INIT_LOG")
            .unwrap_or_else(|_| EnvFilter::new("info")),
    );

    let kvp_filter = get_kvp_filter(config.telemetry.kvp_filter.as_deref())?;

    let emit_kvp_layer = if config.telemetry.kvp_diagnostics {
        let options = KvpOptions::default().vm_id(vm_id);
        match Kvp::with_options(options) {
            Ok(kvp) => Some(kvp.tracing_layer.with_filter(kvp_filter)),
            Err(e) => {
                event!(Level::ERROR, "Failed to initialize Kvp: {}. Continuing without KVP logging.", e);
                None
            }
        }
    } else {
        event!(
            Level::INFO,
            "Hyper-V KVP diagnostics are disabled via config.  It is recommended to be enabled for support purposes."
        );
        None
    };

    let stderr_layer = fmt::layer()
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_writer(std::io::stderr)
        .with_filter(
            EnvFilter::try_from_env("AZURE_INIT_LOG")
                .unwrap_or_else(|_| EnvFilter::new("error")),
        );

    let file_layer = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.azure_init_log_path.path)
    {
        Ok(file) => {
            if let Err(e) = file.set_permissions(Permissions::from_mode(0o600))
            {
                event!(
                    Level::WARN,
                    "Failed to set permissions on {}: {}.",
                    config.azure_init_log_path.path.display(),
                    e,
                );
            }

            Some(
                fmt::layer()
                    .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
                    .with_writer(file)
                    .with_filter(
                        EnvFilter::try_from_env("AZURE_INIT_LOG")
                            .unwrap_or_else(|_| EnvFilter::new("info")),
                    ),
            )
        }
        Err(e) => {
            event!(
                Level::ERROR,
                "Could not open configured log file {}: {}. Continuing without file logging.",
                config.azure_init_log_path.path.display(),
                e
            );

            None
        }
    };

    let subscriber = Registry::default()
        .with(stderr_layer)
        .with(otel_layer)
        .with(emit_kvp_layer)
        .with(file_layer);

    Ok(Box::new(subscriber))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gag::BufferRedirect;
    use serial_test::serial;
    use std::io::Read;
    use tempfile::NamedTempFile;

    #[test]
    #[serial]
    fn test_kvp_filter_default() {
        std::env::remove_var("AZURE_INIT_KVP_FILTER");

        let log_file = NamedTempFile::new().expect("create temp file");
        let log_path = log_file.path().to_path_buf();

        let kvp_filter = get_kvp_filter(None).expect("default filter parses");

        let writer_path = log_path.clone();
        let make_writer = move || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&writer_path)
                .expect("open writer")
        };

        let subscriber = Registry::default().with(
            fmt::layer()
                .with_writer(make_writer)
                .with_filter(kvp_filter),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("warn-default");
            tracing::debug!("debug-default-hidden");
        });

        std::thread::sleep(std::time::Duration::from_millis(50));

        let contents =
            std::fs::read_to_string(&log_path).expect("read default log");
        assert!(contents.contains("warn-default"));
        assert!(!contents.contains("debug-default-hidden"));
    }

    #[test]
    #[serial]
    fn test_kvp_filter_env_override() {
        std::env::set_var("AZURE_INIT_KVP_FILTER", "debug");

        let log_file = NamedTempFile::new().expect("create temp file");
        let log_path = log_file.path().to_path_buf();

        let kvp_filter = get_kvp_filter(None).expect("override filter parses");

        let writer_path = log_path.clone();
        let make_writer = move || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&writer_path)
                .expect("open writer")
        };

        let subscriber = Registry::default().with(
            fmt::layer()
                .with_writer(make_writer)
                .with_filter(kvp_filter),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("warn-override");
            tracing::debug!("debug-override-visible");
        });

        std::thread::sleep(std::time::Duration::from_millis(50));

        let contents =
            std::fs::read_to_string(&log_path).expect("read override log");
        assert!(contents.contains("warn-override"));
        assert!(contents.contains("debug-override-visible"));

        std::env::remove_var("AZURE_INIT_KVP_FILTER");
    }

    #[test]
    #[serial]
    fn test_kvp_filter_from_config() {
        std::env::remove_var("AZURE_INIT_KVP_FILTER");

        let log_file = NamedTempFile::new().expect("create temp file");
        let log_path = log_file.path().to_path_buf();

        let kvp_filter =
            get_kvp_filter(Some("debug")).expect("config filter parses");

        let writer_path = log_path.clone();
        let make_writer = move || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&writer_path)
                .expect("open writer")
        };

        let subscriber = Registry::default().with(
            fmt::layer()
                .with_writer(make_writer)
                .with_filter(kvp_filter),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("warn-cfg-only");
            tracing::debug!("debug-cfg-only-visible");
        });

        std::thread::sleep(std::time::Duration::from_millis(50));

        let contents =
            std::fs::read_to_string(&log_path).expect("read cfg-only log");
        assert!(contents.contains("warn-cfg-only"));
        assert!(contents.contains("debug-cfg-only-visible"));
    }

    #[test]
    #[serial]
    fn test_kvp_filter_env_overrides_config() {
        std::env::set_var("AZURE_INIT_KVP_FILTER", "warn");

        let log_file = NamedTempFile::new().expect("create temp file");
        let log_path = log_file.path().to_path_buf();

        let kvp_filter =
            get_kvp_filter(Some("debug")).expect("precedence filter parses");

        let writer_path = log_path.clone();
        let make_writer = move || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&writer_path)
                .expect("open writer")
        };

        let subscriber = Registry::default().with(
            fmt::layer()
                .with_writer(make_writer)
                .with_filter(kvp_filter),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("warn-precedence");
            tracing::debug!("debug-precedence-hidden");
        });

        std::thread::sleep(std::time::Duration::from_millis(50));

        let contents =
            std::fs::read_to_string(&log_path).expect("read precedence log");
        assert!(contents.contains("warn-precedence"));
        assert!(!contents.contains("debug-precedence-hidden"));

        std::env::remove_var("AZURE_INIT_KVP_FILTER");
    }

    #[test]
    #[serial]
    fn test_kvp_filter_invalid_env_falls_back_to_default() {
        std::env::set_var(
            "AZURE_INIT_KVP_FILTER",
            "app=not_a_valid_level", // This will cause a parse error
        );

        let log_file = NamedTempFile::new().expect("Failed to create tempfile");
        let log_path = log_file.path().to_path_buf();

        let kvp_filter = get_kvp_filter(None)
            .expect("filter should be available (fallback)");

        let writer_path = log_path.clone();
        let make_writer = move || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&writer_path)
                .expect("open writer")
        };

        let subscriber = Registry::default().with(
            fmt::layer()
                .with_writer(make_writer)
                .with_filter(kvp_filter),
        );

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("warn-fallback");
            tracing::debug!("debug-should-be-hidden");
        });

        std::thread::sleep(std::time::Duration::from_millis(50));

        let contents =
            std::fs::read_to_string(&log_path).expect("read log file");
        assert!(contents.contains("warn-fallback"));
        assert!(
            !contents.contains("debug-should-be-hidden"),
            "Invalid env should fall back to default filter (no DEBUG)"
        );

        std::env::remove_var("AZURE_INIT_KVP_FILTER");
    }

    #[test]
    #[serial]
    fn test_azure_init_log() {
        let _buf = BufferRedirect::stderr().unwrap();

        let log_file = NamedTempFile::new().expect("Failed to create tempfile");
        let log_path = log_file.path().to_path_buf();

        let mut config = Config::default();
        config.azure_init_log_path.path = log_path.clone();
        config.telemetry.kvp_diagnostics = false;

        let vm_id = "test-vm-id-for-logging";

        let subscriber =
            setup_layers(vm_id, &config).expect("Failed to setup layers");

        tracing::subscriber::with_default(subscriber, || {
            tracing::trace!(
                "This is a trace message and should NOT be logged."
            );
            tracing::debug!(
                "This is a debug message and should NOT be logged."
            );
            tracing::info!(
                "This is an info message and should be logged to the file."
            );
            tracing::warn!(
                "This is a warn message and should be logged to the file."
            );
            tracing::error!(
                "This is an error message and should be logged to the file."
            );
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let log_contents = std::fs::read_to_string(&log_path)
            .expect("Failed to read log file");

        println!(
            "--- Log file contents for test_azure_init_log ---\n{}\n",
            log_contents
        );

        let lines: Vec<&str> = log_contents.lines().collect();

        assert!(!lines.iter().any(|&line| line.contains("TRACE")));
        assert!(!lines.iter().any(|&line| line.contains("DEBUG")));
        assert!(lines.iter().any(|&line| line.contains("INFO")
            && line.contains("should be logged to the file")));
        assert!(lines.iter().any(|&line| line.contains("WARN")
            && line.contains("should be logged to the file")));
        assert!(lines.iter().any(|&line| line.contains("ERROR")
            && line.contains("should be logged to the file")));
    }

    #[test]
    #[serial]
    fn test_stderr_logger_defaults_to_error() {
        let mut config = Config::default();
        config.telemetry.kvp_diagnostics = false;

        let test_vm_id = "00000000-0000-0000-0000-000000000000";

        let mut buf = BufferRedirect::stderr().unwrap();

        let subscriber =
            setup_layers(test_vm_id, &config).expect("Failed to setup layers");

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                "This is an info message and should NOT be logged to stderr."
            );
            tracing::warn!(
                "This is a warn message and should NOT be logged to stderr."
            );
            tracing::error!(
                "This is an error message and should be logged to stderr."
            );
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut stderr_contents = String::new();
        buf.read_to_string(&mut stderr_contents)
            .expect("Failed to read from stderr buffer");

        drop(buf);

        assert!(!stderr_contents.contains("This is an info message"));
        assert!(!stderr_contents.contains("This is a warn message"));
        assert!(stderr_contents.contains("This is an error message"));
    }
}
