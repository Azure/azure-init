use opentelemetry::{ trace::{self, TracerProvider}, trace::Tracer as _};
use opentelemetry::sdk::trace as sdktrace;
use opentelemetry::sdk::export::trace::stdout;
use opentelemetry::global;
use tracing_opentelemetry::OpenTelemetryLayer;
use once_cell::sync::Lazy;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use libc::dup2;
use tracing_subscriber::{Registry, layer::SubscriberExt};


pub static TRACER: Lazy<()> = Lazy::new(|| {   
    // Redirect stdout to a file
   let log_file_path = "spans.log";
   let file = File::create(log_file_path).expect("Failed to create log file");
   let stdout_fd = file.as_raw_fd();
   unsafe {
       dup2(stdout_fd, libc::STDOUT_FILENO);
   }

    // Set up the stdout exporter correctly
    let exporter = stdout::Exporter::new(std::io::stdout(), true);

    // Set up the TracerProvider with the stdout exporter
    let provider = sdktrace::TracerProvider::builder()
        .with_simple_exporter(exporter)
        .build();

   global::set_tracer_provider(provider.clone());

   let tracer = provider.tracer("azure-kvp");
   // Create the OpenTelemetry layer using the SDK tracer
   let otel_layer = OpenTelemetryLayer::new(tracer);
    
    // Create a `tracing` subscriber
    let subscriber = Registry::default()
        .with(otel_layer);
    // Set the subscriber as the global default
    tracing::subscriber::set_global_default(subscriber)
        .expect("Setting default subscriber failed");
    });

pub fn initialize_tracing() {
    Lazy::force(&TRACER);
}