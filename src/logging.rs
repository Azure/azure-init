// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use once_cell::sync::OnceCell;
use opentelemetry::{global, trace::TracerProvider};
use opentelemetry_sdk::trace::{self as sdktrace, Sampler, SdkTracerProvider};
use std::fs::{File, OpenOptions, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tracing::{event, Level};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::filter::Filtered;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{
    fmt::{self, writer::MakeWriter},
    layer::SubscriberExt,
    reload::{self, Handle},
    EnvFilter, Layer, Registry,
};

use crate::kvp::EmitKVPLayer;
use libazureinit::config::{Config, DEFAULT_AZURE_INIT_LOG_PATH};

/// A global static that holds reload handles & writer state
static LOGGING_STATE: OnceCell<LoggingState> = OnceCell::new();

/// This struct keeps any reload handles or shared state we need to reconfigure later.
pub struct LoggingState {
    kvp_reload_handle:
        Handle<Filtered<EmitKVPLayer, EnvFilter, Registry>, Registry>,
    reloadable_file: ReloadableFile,
}
/// A "reloadable writer" that can swap out the underlying file at runtime.
#[derive(Clone)]
pub struct ReloadableFile {
    inner: std::sync::Arc<std::sync::Mutex<File>>,
}

impl ReloadableFile {
    pub fn new(file: File) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(file)),
        }
    }

    /// Swap the underlying file handle at runtime.
    pub fn swap_file(&self, new_file: File) {
        let mut guard = self.inner.lock().unwrap();
        *guard = new_file;
    }
}

/// Implement the `MakeWriter` trait so that `FmtLayer` can write logs via this reloadable file.
impl<'a> MakeWriter<'a> for ReloadableFile {
    type Writer = std::io::BufWriter<File>;

    fn make_writer(&'a self) -> Self::Writer {
        let file = self.inner.lock().unwrap();
        // Clone the file handle for concurrent writes
        let cloned = file
            .try_clone()
            .expect("Failed to clone underlying file handle");
        std::io::BufWriter::new(cloned)
    }
}

pub fn initialize_tracing() -> sdktrace::Tracer {
    let provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .build();

    global::set_tracer_provider(provider.clone());
    provider.tracer("azure-kvp")
}

/// Builds a `tracing` subscriber that can optionally write azure-init.log
/// to a specific location if `Some(&Config)` is provided.
///
/// Phase 1 (Pre-config):
///   If no logging state exists yet (LOGGING_STATE is empty), install the subscriber
///   using fallback default values (for example, using DEFAULT_AZURE_INIT_LOG_PATH and
///   enabling KVP by default).
///
/// Phase 2 (Post-config):
///   Once a configuration has been loaded, call this function again with `Some(&Config)`.
///   In that case, the function will call `reload_layers(config)` to update the logging
///   layers (for example, swap the file path or change the KVP filter). If `config` is `None`,
///   it falls back to defaults.
pub fn setup_layers(
    tracer: sdktrace::Tracer,
    vm_id: &str,
    config: Option<&Config>,
) -> Result<(), Box<dyn std::error::Error>> {
    if LOGGING_STATE.get().is_none() {
        // Phase 1: No logging state yet, so install the subscriber with fallback defaults.
        install_global_subscriber(tracer, vm_id)?;
    } else {
        // Phase 2: Global logger is already set.
        reload_layers(config)?;
    }
    Ok(())
}

/// Install the global tracing subscriber exactly once,
/// with fallback paths for file logging and with KVP layer enabled by default.
fn install_global_subscriber(
    tracer: sdktrace::Tracer,
    vm_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let otel_layer = OpenTelemetryLayer::new(tracer)
        .with_filter(EnvFilter::from_env("AZURE_INIT_LOG"));

    let stderr_layer = fmt::layer()
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::from_env("AZURE_INIT_LOG"));

    let kvp_filter = EnvFilter::builder().parse(
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
        ]
        .join(","),
    )?;

    let raw_kvp_layer = match EmitKVPLayer::new(
        std::path::PathBuf::from("/var/lib/hyperv/.kvp_pool_1"),
        vm_id,
    ) {
        Ok(real_layer) => real_layer,
        Err(e) => {
            event!(
                Level::ERROR,
                "Failed to initialize EmitKVPLayer: {}. No KVP logging.",
                e
            );
            EmitKVPLayer::noop()
        }
    };

    let emit_kvp_layer = raw_kvp_layer.with_filter(kvp_filter);
    let (reloadable_kvp_layer, kvp_reload_handle) =
        reload::Layer::new(emit_kvp_layer);

    let fallback_path = PathBuf::from(DEFAULT_AZURE_INIT_LOG_PATH);

    let file = open_or_log_error(&fallback_path);
    let reloadable_file = ReloadableFile::new(file);

    let file_filter = EnvFilter::from_env("AZURE_INIT_LOG");
    let file_layer = fmt::layer()
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_writer(reloadable_file.clone())
        .with_filter(file_filter);

    let subscriber = Registry::default()
        .with(reloadable_kvp_layer)
        .with(stderr_layer)
        .with(otel_layer)
        .with(file_layer);

    tracing::subscriber::set_global_default(subscriber)?;

    // Save reload handles (KVP handle + reloadable_file) in a OnceCell
    let logging_state = LoggingState {
        kvp_reload_handle,
        reloadable_file,
    };
    LOGGING_STATE
        .set(logging_state)
        .map_err(|_| "Global logger already set")?;

    Ok(())
}

fn reload_layers(
    config: Option<&Config>,
) -> Result<(), Box<dyn std::error::Error>> {
    let logging_state =
        LOGGING_STATE.get().expect("Logger not yet initialized");

    let kvp_enabled = config
        .map(|cfg| cfg.telemetry.kvp_diagnostics)
        .unwrap_or(true);

    let new_kvp_filter = if kvp_enabled {
        EnvFilter::builder().parse(
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
            ]
            .join(","),
        )?
    } else {
        EnvFilter::new("off")
    };

    // Overwrite the internal filter of the KVP layer
    logging_state.kvp_reload_handle.modify(|filtered_kvp| {
        *filtered_kvp.filter_mut() = new_kvp_filter;
    })?;

    let new_path = config
        .map(|cfg| cfg.azure_init_log_path.path.clone())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_AZURE_INIT_LOG_PATH));

    let new_file = open_or_log_error(&new_path);
    logging_state.reloadable_file.swap_file(new_file);

    Ok(())
}

/// A small helper to open or create the given path, setting permissions to 0600.
/// If it fails, we log an error and open `/dev/null` instead.
fn open_or_log_error(path: &Path) -> File {
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => {
            if let Err(e) = file.set_permissions(Permissions::from_mode(0o600))
            {
                event!(
                    Level::WARN,
                    "Failed to set permissions on {}: {}.",
                    path.display(),
                    e
                );
            }
            file
        }
        Err(e) => {
            event!(
                Level::ERROR,
                "Could not open configured log file {}: {}. Logging to /dev/null.",
                path.display(),
                e
            );
            OpenOptions::new()
                .write(true)
                .open("/dev/null")
                .expect("Failed to open /dev/null")
        }
    }
}
