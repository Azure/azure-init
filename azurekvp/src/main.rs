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


fn main() -> io::Result<()> {
    // Specify the log file path
    let log_file_path = "spans.log";

    // Create a new file for writing
    let file = File::create(log_file_path)?;

    // Duplicate file descriptor to stdout
    let stdout_fd = file.as_raw_fd();
    dup2(stdout_fd, libc::STDOUT_FILENO)?;

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

    // Trace executed code
    tracing::subscriber::with_default(subscriber, || {
        // Spans will be sent to the configured OpenTelemetry exporter
        let root = span!(tracing::Level::TRACE, "app_start", work_units = 2);
        let _enter = root.enter();

        // Error events will be logged in the root span
        error!("This event will be logged in the root span.");

        // Redirect stdout to the file after tracing setup
        let file = File::create(log_file_path)?;
        let stdout_fd = file.as_raw_fd();
        dup2(stdout_fd, libc::STDOUT_FILENO)?;

        Ok(())
    })
}