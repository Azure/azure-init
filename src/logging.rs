// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use opentelemetry::{global, trace::TracerProvider};
use opentelemetry_sdk::trace::{self as sdktrace, Sampler, SdkTracerProvider};
use tracing::{event, Level};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{
    fmt, layer::SubscriberExt, EnvFilter, Layer, Registry,
};

use crate::kvp::EmitKVPLayer;

pub fn initialize_tracing() -> sdktrace::Tracer {
    let provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .build();

    global::set_tracer_provider(provider.clone());
    provider.tracer("azure-kvp")
}

pub fn setup_layers(
    tracer: sdktrace::Tracer,
    vm_id: &str,
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

    let subscriber = Registry::default()
        .with(stderr_layer)
        .with(otel_layer)
        .with(emit_kvp_layer);

    tracing::subscriber::set_global_default(subscriber)?;

    Ok(())
}
