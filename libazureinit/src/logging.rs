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
use crate::kvp::{EmitKVPLayer, Kvp as KvpInternal};

pub type LoggingSetup = (
    Box<dyn Subscriber + Send + Sync + 'static>,
    Option<JoinHandle<std::io::Result<()>>>,
);

fn initialize_tracing() -> sdktrace::Tracer {
    let provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .build();

    global::set_tracer_provider(provider.clone());
    provider.tracer("azure-kvp")
}

fn kvp_env_filter() -> Result<EnvFilter, anyhow::Error> {
    Ok(EnvFilter::builder().parse(
        [
            "WARN",
            "azure_init=INFO",
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
        ]
        .join(","),
    )?)
}

// Public KVP wrapper API for library consumers
struct KvpLayer<S: Subscriber>(Filtered<EmitKVPLayer, EnvFilter, S>);

/// Emit tracing data to the Hyper-V KVP.
///
/// # Example
///
/// ```no_run
/// # use libazureinit::logging::Kvp;
/// use tracing_subscriber::layer::SubscriberExt;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let mut kvp = Kvp::new("a-unique-id")?;
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
    /// Refer to [`libazureinit::get_vm_id`] to retrieve the VM's unique identifier.
    pub fn new<T: AsRef<str>>(vm_id: T) -> Result<Self, anyhow::Error> {
        let shutdown = CancellationToken::new();
        let inner = KvpInternal::new(
            std::path::PathBuf::from("/var/lib/hyperv/.kvp_pool_1"),
            vm_id.as_ref(),
            shutdown.clone(),
        )?;

        let kvp_filter = kvp_env_filter()?;
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

    let kvp_filter = kvp_env_filter()?;

    let (emit_kvp_layer, kvp_writer_handle) = if config
        .telemetry
        .kvp_diagnostics
    {
        match KvpInternal::new(
            std::path::PathBuf::from("/var/lib/hyperv/.kvp_pool_1"),
            vm_id,
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
    use std::io::Read;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_azure_init_log() {
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
