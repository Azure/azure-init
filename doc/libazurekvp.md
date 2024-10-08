## Tracing Logic Overview

### How Tracing is Set Up

The tracing setup in this project is built around three key layers, each with its own responsibility:

1. **EmitKVPLayer**: Custom Layer for Span Processing
2. **OpenTelemetryLayer**: Context Propagation and Span Export
3. **stderr_layer**: Formatting and Logging to stderr

These layers work together, yet independently, to process span data as it flows through the program.

### Layer Overview

#### 1. EmitKVPLayer

- **Purpose**: This custom layer is responsible for processing spans and events by capturing their metadata, generating key-value pairs (KVPs), encoding them into a specific format, and writing the encoded data to the VM's Hyper-V file for consumption by the `hv_kvp_daemon` service.

- **How It Works**:
  - **Span Processing**: When a span is created, `EmitKVPLayer` processes the span's metadata, generating a unique key for the span and encoding the span data into a binary format that can be consumed by Hyper-V.
  - **Event Processing**: When an event is emitted using the `event!` macro, the `on_event` method in `EmitKVPLayer` processes the event, capturing its message and linking it to the current span.  Events are useful for tracking specific points in time within a span, such as errors, warnings, retries, or important state changes. Events are recorded independently of spans but they are be tied to the span they occur within by using the same span metadata. 
  - Both span and event data are written to the `/var/lib/hyperv/.kvp_pool_1` file, which is typically monitored by the Hyper-V `hv_kvp_daemon` service.
  - The `hv_kvp_daemon` uses this file to exchange key-value pair (KVP) data between the virtual machine and the Hyper-V host. This mechanism is crucial for telemetry and data synchronization.

- **Reference**: For more details on how the Hyper-V Data Exchange Service works, refer to the official documentation here: [Hyper-V Data Exchange Service (KVP)](https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/reference/integration-services#hyper-v-data-exchange-service-kvp).

#### 2. OpenTelemetryLayer

- **Purpose**: This layer integrates with the OpenTelemetry framework to handle context propagation and export span data to an external tracing backend (e.g., Jaeger, Prometheus) or to stdout.
- **How It Works**:
  - As spans are created and processed, the `OpenTelemetryLayer` ensures that context is propagated correctly across different parts of the program, which is crucial in distributed systems for tracing requests across service boundaries.
  - The span data is then exported to a configured backend or stdout, where it can be visualized and analyzed using OpenTelemetry-compatible tools.

#### 3. stderr_layer

- **Purpose**: This layer formats and logs span and event data to stderr or a specified log file, providing a human-readable output for immediate inspection.
- **How It Works**:
  - Each span's lifecycle events, as well as individual emitted events, are logged in a structured format, making it easy to see the flow of execution in the console or log files.
  - This layer is particularly useful for debugging and monitoring during development.

### How the Layers Work Together

- **Independent Processing**: Each of these layers processes spans and events independently. When a span is created, it triggers the `on_new_span` method in each layer, and when an event is emitted, it triggers the `on_event` method. As the span progresses through its lifecycle (`on_enter`, `on_close`), each layer performs its respective tasks.
- **Order of Execution**: The layers are executed in the order they are added in the `initialize_tracing` function. For instance, `EmitKVPLayer` might process a span before `OpenTelemetryLayer`, but this order only affects the sequence of operations, not the functionality or output of each layer.
- **No Cross-Layer Dependencies**: Each layer operates independently of the others. For example, the `EmitKVPLayer` encodes and logs span and event data without affecting how `OpenTelemetryLayer` exports span data to a backend. This modular design allows each layer to be modified, replaced, or removed without impacting the others.

In the `main.rs` file, the tracing logic is initialized. Spans are instrumented using the `#[instrument]` attribute and events can be created with the `event!` macro to monitor the execution of the function. Here's an example:

```rust
#[instrument(name = "root")]
async fn provision() -> Result<(), anyhow::Error> {
    event!(Level::INFO, msg = "Starting the provision process...");
    // Other logic...
}
```

1. **Initialization**:  
   The `initialize_tracing` function is called at the start of the program to set up the tracing subscriber with the configured layers (`EmitKVPLayer`, `OpenTelemetryLayer`, and `stderr_layer`).

2. **Instrumenting the `provision()` Function**:  
   The `#[instrument]` attribute is used to automatically create a span for the `provision()` function.  
   - The `name = "root"` part of the `#[instrument]` attribute specifies the name of the span.
   - This span will trace the entire execution of the `provision()` function, capturing any relevant metadata (e.g., function parameters, return values).

3. **Span Processing**:  
   As the `provision()` function is called and spans are created, entered, exited, and closed, they are processed by the layers configured in `initialize_tracing`:  
   - **EmitKVPLayer** processes the span, generates key-value pairs, encodes them, and writes them directly to `/var/lib/hyperv/.kvp_pool_1`.  
   - **OpenTelemetryLayer** handles context propagation and exports span data to a tracing backend or stdout.  
   - **stderr_layer** logs span information to stderr or another specified output for immediate visibility.

