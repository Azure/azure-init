// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::TracerProvider;
use opentelemetry_sdk::export::trace::SpanExporter;
use opentelemetry_stdout as stdout;
use tracing::{error, span};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;
use std::fs::File;
use std::io::{self, Write};
use nix::unistd::dup2;
use nix::libc;
use std::os::fd::AsRawFd;
use tracing::instrument;


#[instrument]
fn my_function() {
    // Code inside my_function
    error!("This is an error logged inside my_function.");
}

fn main() -> io::Result<()> {
    // Specify the log file path
    let log_file_path = "spans.log";

    // Create a new file for writing
    let file = File::create(log_file_path)?;

    // Create a new OpenTelemetry trace pipeline that prints to stdout
    let provider = TracerProvider::builder()
        .with_simple_exporter(stdout::SpanExporter::default())
        .build();
    let tracer = provider.tracer("readme_example");

    // Create a tracing layer with the configured tracer
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

    // Use the tracing subscriber `Registry`, or any other subscriber
    // that impls `LookupSpan`
    let subscriber = Registry::default().with(telemetry);

    // Initialize the tracing subscriber
    tracing::subscriber::with_default(subscriber, || {
        // Call the instrumented function
        my_function();

        // Redirect stdout to the file after tracing setup
        let stdout_fd = file.as_raw_fd();
        dup2(stdout_fd, libc::STDOUT_FILENO)?;

        Ok(())
    })
}