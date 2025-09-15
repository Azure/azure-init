# Azure-init Tracing System

## Overview

Azure-init implements a comprehensive tracing system that captures detailed information about the provisioning process.
This information is crucial for monitoring, debugging, and troubleshooting VM provisioning issues in Azure environments.
The tracing system is built on a multi-layered architecture that provides flexibility and robustness.

## Architecture

The tracing architecture consists of three specialized layers, each handling a specific aspect of the tracing process:

### 1. EmitKVPLayer

**Purpose**: Processes spans and events by capturing metadata, generating key-value pairs (KVPs), and writing to Hyper-V's data exchange file.

**Key Functions**:
- Captures span lifecycle events (creation, entry, exit, closing)
- Processes emitted events within spans
- Formats data as KVPs for Hyper-V consumption
- Writes encoded data to `/var/lib/hyperv/.kvp_pool_1`

**Integration with Azure**:
- The `/var/lib/hyperv/.kvp_pool_1` file is monitored by the Hyper-V `hv_kvp_daemon` service
- This enables key metrics and logs to be transferred from the VM to the Azure platform
- Administrators can access this data through the Azure portal or API

### 2. OpenTelemetryLayer

**Purpose**: Manages context propagation and exports span data to external tracing systems.

**Key Functions**:
- Maintains distributed tracing context across service boundaries
- Exports standardized trace data to compatible backends
- Enables integration with broader monitoring ecosystems

### 3. stderr_layer

**Purpose**: Formats and logs trace data to stderr or specified log files.

**Key Functions**:
- Provides human-readable logging for immediate inspection
- Supports debugging during development
- Captures trace events even when other layers might fail

## How the Layers Work Together

Despite operating independently, these layers collaborate to provide comprehensive tracing:

1. **Independent Processing**: Each layer processes spans and events without dependencies on other layers
2. **Ordered Execution**: Layers are executed in the order specified in the `initialize_tracing` function
3. **Complementary Functions**: Each layer serves a specific purpose in the tracing ecosystem:
   - `EmitKVPLayer` focuses on Azure Hyper-V integration
   - `OpenTelemetryLayer` handles standardized tracing and exports
   - `stderr_layer` provides immediate visibility for debugging

## Practical Usage

### Instrumenting Functions

To instrument code with tracing, use the `#[instrument]` attribute on functions:

```rust
use tracing::{instrument, Level, event};

#[instrument(name = "provision_user", fields(user_id = ?user.id))]
async fn provision_user(user: User) -> Result<(), Error> {
    event!(Level::INFO, "Starting user provisioning");
    
    // Function logic
    
    event!(Level::INFO, "User provisioning completed successfully");
    Ok(())
}
```

### Emitting Events

To record specific points within a span:

```rust
use tracing::{event, Level};

fn configure_ssh_keys(user: &str, keys: &[String]) {
    event!(Level::INFO, user = user, key_count = keys.len(), "Configuring SSH keys");
    
    for (i, key) in keys.iter().enumerate() {
        event!(Level::DEBUG, user = user, key_index = i, "Processing SSH key");
        // Process each key
    }
    
    event!(Level::INFO, user = user, "SSH keys configured successfully");
}
```

### Initializing the Tracing System

The tracing system must be initialized at application startup:

```rust
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

fn main() {
    // Initialize the tracing system with all layers
    let subscriber = Registry::default()
        .with(EmitKVPLayer::new())
        .with(OpenTelemetryLayer::new())
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set global default subscriber");
    
    // Application code
}
```

## Key Benefits

- **Comprehensive Visibility**: Capture the entire flow of execution with detailed context
- **Structured Data**: Events include rich metadata for filtering and analysis
- **Azure Integration**: Direct integration with Azure monitoring via Hyper-V KVP
- **Standard Compatibility**: OpenTelemetry support enables integration with industry-standard tools
- **Debugging Support**: Immediate output to stderr assists in troubleshooting

## Reference Documentation

For more details on how the Hyper-V Data Exchange Service works, refer to the official documentation:
[Hyper-V Data Exchange Service (KVP)](https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/reference/integration-services#hyper-v-data-exchange-service-kvp)

For OpenTelemetry integration details:
[OpenTelemetry for Rust](https://opentelemetry.io/docs/instrumentation/rust/)