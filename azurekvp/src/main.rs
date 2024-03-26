use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::TracerProvider;
use opentelemetry_stdout as stdout;
use std::fs::File;
use std::io::{self, Write};
use tracing::{error, span};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

fn main() {
   // Create a new OpenTelemetry trace pipeline that prints to stdout
   let provider = TracerProvider::builder()
       .with_simple_exporter(stdout::SpanExporter::default())
       .build();
   let tracer = provider.tracer("readme_example");

   // Specify the log file path
   let log_file_path = "azurekvp/src/spans.log";
   let file_name_prefix = "spans.log";

   // Check if the log file exists, and create it if not
   if !std::path::Path::new(log_file_path).exists() {
       let _ = File::create(log_file_path).expect("Failed to create log file");
   }
   // Create a file appender using tracing-appender
   let appender = RollingFileAppender::new(Rotation:: NEVER, log_file_path, file_name_prefix);

   // Create a tracing layer with the configured tracer and file appender
   let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
   let subscriber = Registry::default()
        .with(telemetry)
        .with(Layer::new().with_writer(appender));
       //.with(FmtSpan::default().with_writer(appender));
    
   // Use the tracing subscriber `Registry`
   tracing::subscriber::with_default(subscriber, || {
       // Spans will be sent to the configured OpenTelemetry exporter
       let root = span!(tracing::Level::TRACE, "app_start", work_units = 2);
       let _enter = root.enter();
       error!("This event will be logged in the root span.");
   });
}