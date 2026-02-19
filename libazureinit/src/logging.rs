// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use opentelemetry::{global, trace::TracerProvider};
use opentelemetry_sdk::trace::{self as sdktrace, Sampler, SdkTracerProvider};
use std::fs::{OpenOptions, Permissions};
use std::os::unix::fs::PermissionsExt;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{event, Level, Subscriber};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{
    filter::Filtered, fmt, layer::SubscriberExt, registry::LookupSpan,
    EnvFilter, Layer, Registry,
};

use crate::config::Config;
pub use crate::kvp::EVENT_PREFIX;
use crate::kvp::{EmitKVPLayer, Kvp as KvpInternal};

pub type LoggingSetup = (
    Box<dyn Subscriber + Send + Sync + 'static>,
    Option<JoinHandle<std::io::Result<()>>>,
);

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

// Public KVP wrapper API for library consumers
struct KvpLayer<S: Subscriber>(Filtered<EmitKVPLayer, EnvFilter, S>);

/// Emit tracing data to the Hyper-V KVP.
///
/// ## KVP Tracing Configuration
///
/// The KVP tracing layer's filter can be configured at runtime by setting the
/// `AZURE_INIT_KVP_FILTER` environment variable. This allows any application
/// using this library to override the default filter and control which traces
/// are sent to the KVP pool.
///
/// The value of the variable must be a string that follows the syntax for
/// `tracing_subscriber::EnvFilter`, which is a comma-separated list of
/// logging directives. For example: `warn,my_crate=debug` or `info,my_crate::api=trace`.
/// See `config.rs` for more details.
///
/// The filter can also be configured via the `kvp_filter` field in the `Config` struct.
/// **Precedence**: Environment variable > Config field > Default filter.
/// If neither is set, a default filter tailored for `azure-init` (WARN level + specific modules) is used.
///
/// If an invalid filter string is provided (via env or config), a warning is logged
/// and the default filter is used instead.
///
/// # Example
///
/// ```no_run
/// # use libazureinit::logging::Kvp;
/// use tracing_subscriber::layer::SubscriberExt;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let mut kvp = Kvp::new("a-unique-id", None)?;
/// let registry = tracing_subscriber::Registry::default().with(kvp.layer());
///
/// // When it's time to shut down, doing this ensures all writes are flushed
/// kvp.halt().await?;
/// # Ok(())
/// # }
/// ```
pub struct Kvp<S: Subscriber> {
    layer: Option<KvpLayer<S>>,
    /// The `JoinHandle` for the background task responsible for writing
    /// KVP data to the file. The caller can use this handle to wait for
    /// the writer to finish.
    writer: JoinHandle<std::io::Result<()>>,
    shutdown: CancellationToken,
}

impl<S: Subscriber + for<'lookup> LookupSpan<'lookup>> Kvp<S> {
    /// Create a new tracing layer for KVP.
    ///
    /// When `event_prefix` is `None`, the default [`EVENT_PREFIX`] is used.
    /// Pass `Some("my-library-1.0")` to identify a different emitting library.
    ///
    /// Refer to [`libazureinit::get_vm_id`] to retrieve the VM's unique identifier.
    pub fn new<T: AsRef<str>>(
        vm_id: T,
        event_prefix: Option<&str>,
    ) -> Result<Self, anyhow::Error> {
        let event_prefix = event_prefix.unwrap_or(EVENT_PREFIX);
        let shutdown = CancellationToken::new();
        let inner = KvpInternal::new(
            std::path::PathBuf::from("/var/lib/hyperv/.kvp_pool_1"),
            vm_id.as_ref(),
            event_prefix,
            shutdown.clone(),
        )?;

        let kvp_filter = get_kvp_filter(None)?;
        let layer = Some(KvpLayer(inner.tracing_layer.with_filter(kvp_filter)));

        Ok(Self {
            layer,
            writer: inner.writer,
            shutdown,
        })
    }

    /// Get a tracing [`Layer`] to use with a [`Registry`].
    ///
    /// # Panics if this function is called more than once.
    pub fn layer(&mut self) -> Filtered<EmitKVPLayer, EnvFilter, S> {
        assert!(
            self.layer.is_some(),
            "Kvp::layer cannot be called multiple times!"
        );
        self.layer.take().unwrap().0
    }

    /// Gracefully shut down the KVP writer.
    ///
    /// This will stop new KVP logs from being queued and wait for all pending writes to the KVP
    /// pool to complete.  After this returns, no further logs will be written to KVP.
    pub async fn halt(self) -> Result<(), anyhow::Error> {
        self.shutdown.cancel();
        self.writer.await??;
        Ok(())
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
    graceful_shutdown: CancellationToken,
) -> Result<LoggingSetup, anyhow::Error> {
    let tracer = initialize_tracing();
    let otel_layer = OpenTelemetryLayer::new(tracer).with_filter(
        EnvFilter::try_from_env("AZURE_INIT_LOG")
            .unwrap_or_else(|_| EnvFilter::new("info")),
    );

    let kvp_filter = get_kvp_filter(config.telemetry.kvp_filter.as_deref())?;

    let (emit_kvp_layer, kvp_writer_handle) = if config
        .telemetry
        .kvp_diagnostics
    {
        match KvpInternal::new(
            std::path::PathBuf::from("/var/lib/hyperv/.kvp_pool_1"),
            vm_id,
            EVENT_PREFIX,
            graceful_shutdown,
        ) {
            Ok(kvp) => {
                let layer = kvp.tracing_layer.with_filter(kvp_filter);
                (Some(layer), Some(kvp.writer))
            }
            Err(e) => {
                event!(Level::ERROR, "Failed to initialize Kvp: {}. Continuing without KVP logging.", e);
                (None, None)
            }
        }
    } else {
        event!(
            Level::INFO,
            "Hyper-V KVP diagnostics are disabled via config.  It is recommended to be enabled for support purposes."
        );
        (None, None)
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

    Ok((Box::new(subscriber), kvp_writer_handle))
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

    #[tokio::test]
    #[serial]
    async fn test_azure_init_log() {
        // Redirect stderr to a buffer to keep the main test output clean from ERROR logs.
        let _buf = BufferRedirect::stderr().unwrap();

        let log_file = NamedTempFile::new().expect("Failed to create tempfile");
        let log_path = log_file.path().to_path_buf();

        let mut config = Config::default();
        config.azure_init_log_path.path = log_path.clone();
        config.telemetry.kvp_diagnostics = false;

        let vm_id = "test-vm-id-for-logging";
        let graceful_shutdown = CancellationToken::new();

        let (subscriber, _kvp_handle) =
            setup_layers(vm_id, &config, graceful_shutdown.clone())
                .expect("Failed to setup layers");

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

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        graceful_shutdown.cancel();

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

    #[tokio::test]
    #[serial]
    async fn test_stderr_logger_defaults_to_error() {
        let mut config = Config::default();
        config.telemetry.kvp_diagnostics = false;

        let test_vm_id = "00000000-0000-0000-0000-000000000000";
        let graceful_shutdown = CancellationToken::new();

        // Redirect stderr to a buffer
        let mut buf = BufferRedirect::stderr().unwrap();

        let (subscriber, _kvp_handle) =
            setup_layers(test_vm_id, &config, graceful_shutdown.clone())
                .expect("Failed to setup layers");

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

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        graceful_shutdown.cancel();

        let mut stderr_contents = String::new();
        buf.read_to_string(&mut stderr_contents)
            .expect("Failed to read from stderr buffer");

        drop(buf); // release stderr

        assert!(!stderr_contents.contains("This is an info message"));
        assert!(!stderr_contents.contains("This is a warn message"));
        assert!(stderr_contents.contains("This is an error message"));
    }
}
