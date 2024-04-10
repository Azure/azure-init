// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
use opentelemetry::trace::{TracerProvider, Tracer};
use opentelemetry_sdk::export::trace::SpanExporter;
use tracing::{error, span};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;
use std::fs::File;
use std::io::Write;
use std::pin::Pin;
use std::future::Future;
use opentelemetry::trace::TraceError;


#[derive(Debug)]
struct FileExporter {
   file: File,
}
impl FileExporter {
   fn new(file: File) -> Self {
       FileExporter { file }
   }
}
impl SpanExporter for FileExporter {
    fn export(&mut self, batch: Vec<opentelemetry_sdk::export::trace::SpanData>) -> Pin<Box<dyn Future<Output = Result<(), TraceError>> + Send + 'static>> {
        Box::pin(async move {
            for span_data in batch {
                let span_json = match serde_json::to_string(&span_data) {
                    Ok(json) => json,
                    Err(e) => {
                        error!("Failed to serialize span data: {:?}", e);
                        String::default()
                    }
                };
                if let Err(e) = writeln!(self.file, "{}", span_json) {
                    error!("Failed to write span data to file: {:?}", e);
                }
            }
            Ok(())
        })
    }
    fn shutdown(&mut self) {}
 }
fn main() {
   // Open a file for writing spans
   let log_file_path = "spans.log";
   let file = File::create(log_file_path).expect("Failed to create file");
   let exporter = FileExporter::new(file);

   // Create a new OpenTelemetry trace pipeline with custom file exporter
   let provider = opentelemetry_sdk::trace::TracerProvider::builder()
       .with_simple_exporter(exporter)
       .build();

   let tracer = provider.tracer("readme_example");
   // Create a tracing layer with the configured tracer
   
   let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

   // Use the tracing subscriber `Registry`, or any other subscriber
   // that impls `LookupSpan`
   let subscriber = Registry::default().with(telemetry);

   // Trace executed code
   tracing::subscriber::with_default(subscriber, || {
       // Spans will be sent to the configured OpenTelemetry exporter
       let root = span!(tracing::Level::TRACE, "app_start", work_units = 2);
       let _enter = root.enter();
       error!("This event will be logged in the root span.");
   });
}