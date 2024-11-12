// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use opentelemetry::{global, sdk::trace as sdktrace, trace::TracerProvider};

/// Initializes the tracing system by setting up a tracing provider and
/// registering it globally. This function returns a tracer instance
/// associated with the "azure-kvp" application.
///
/// # Returns
/// A sdktrace::Tracer object that can be used to create and manage spans.
pub fn initialize_tracing() -> sdktrace::Tracer {
    let provider = sdktrace::TracerProvider::builder().build();
    global::set_tracer_provider(provider.clone());
    provider.tracer("azure-kvp")
}
