// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use opentelemetry::{global, trace::TracerProvider};
use opentelemetry_sdk::trace::{self as sdktrace, Sampler, SdkTracerProvider};
use std::fs::{OpenOptions, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tracing::{event, Level};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{
    fmt, layer::SubscriberExt, EnvFilter, Layer, Registry,
};

use crate::kvp::EmitKVPLayer;
use libazureinit::config::{Config, DEFAULT_AZURE_INIT_LOG_PATH};

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
/// This function follows a two-phase initialization:
/// - Minimal Setup (Pre-Config): When called initially, it sets up basic logging
///   to console (`stderr`), KVP (Hyper-V), and OpenTelemetry without file logging.
///
/// - Full Setup (Post-Config): After the configuration is loaded, it is called again
///   with `config`, adding file logging to `config.azure_init_log_path.path` or
///   falling back to `DEFAULT_AZURE_INIT_LOG_PATH` if unspecified.
pub fn setup_layers(
    tracer: sdktrace::Tracer,
    vm_id: &str,
    config: Option<&Config>,
) -> Result<(), Box<dyn std::error::Error>> {
    let otel_layer = OpenTelemetryLayer::new(tracer)
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

    let emit_kvp_layer = match EmitKVPLayer::new(
        std::path::PathBuf::from("/var/lib/hyperv/.kvp_pool_1"),
        vm_id,
    ) {
        Ok(layer) => Some(layer.with_filter(kvp_filter)),
        Err(e) => {
            event!(Level::ERROR, "Failed to initialize EmitKVPLayer: {}. Continuing without KVP logging.", e);
            None
        }
    };

    let stderr_layer = fmt::layer()
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::from_env("AZURE_INIT_LOG"));

    let log_path = config
        .map(|cfg| cfg.azure_init_log_path.path.clone())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_AZURE_INIT_LOG_PATH));

    let file_layer = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(file) => {
            if let Err(e) = file.set_permissions(Permissions::from_mode(0o600))
            {
                event!(
                    Level::WARN,
                    "Failed to set permissions on {}: {}.",
                    log_path.display(),
                    e,
                );
            }

            Some(
                fmt::layer()
                    .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
                    .with_writer(file)
                    .with_filter(EnvFilter::from_env("AZURE_INIT_LOG")),
            )
        }
        Err(e) => {
            event!(
                Level::ERROR,
                "Could not open configured log file {}: {}. Continuing without file logging.",
                log_path.display(),
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

    tracing::subscriber::set_global_default(subscriber)?;

    Ok(())
}
