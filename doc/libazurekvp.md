# Azure-init Tracing System

## Overview

Azure-init implements a comprehensive tracing system that captures detailed information about the provisioning process.
This information is crucial for monitoring, debugging, and troubleshooting VM provisioning issues in Azure environments.
The tracing system is built on a multi-layered architecture that provides flexibility and robustness.

## Architecture

The tracing architecture consists of four specialized layers, each handling a specific aspect of the tracing process:

### 1. EmitKVPLayer

**Purpose**: Processes spans and events by capturing metadata, generating key-value pairs (KVPs), and writing to Hyper-V's data exchange file.

**Key Functions**:
- Captures span lifecycle events (creation, entry, exit, closing)
- Processes emitted events within spans
- Formats data as KVPs for Hyper-V consumption
- Writes encoded data to `/var/lib/hyperv/.kvp_pool_1`

Additionally, events emitted with a `health_report` field are written as special provisioning reports using the key `PROVISIONING_REPORT`.

**Integration with Azure**:
- The `/var/lib/hyperv/.kvp_pool_1` file is monitored by the Hyper-V `hv_kvp_daemon` service
- This enables key metrics and logs to be transferred from the VM to the Azure platform
- Administrators can access this data through the Azure portal or API

### 2. OpenTelemetryLayer

**Purpose**: Propagates tracing context and prepares span data for export.

**Key Functions**:
- Maintains distributed tracing context across service boundaries
- Exports standardized trace data to compatible backends
- Enables integration with broader monitoring ecosystems

### 3. StdErrLayer

**Purpose**: Formats and logs trace data to stderr.

**Key Functions**:
- Provides human-readable logging for immediate inspection
- Supports debugging during development
- Captures trace events even when other layers might fail

### 4. File Layer

**Purpose**: Writes formatted logs to a file (default path: `/var/log/azure-init.log`).

**Key Functions**:
- Provides a persistent log for post-provisioning inspection
- Uses file permissions `0600` when possible
- Log level controlled by `AZURE_INIT_LOG` (defaults to `info` for the file layer)

## How the Layers Work Together

Despite operating independently, these layers collaborate to provide comprehensive tracing:

1. **Independent Processing**: Each layer processes spans and events without dependencies on other layers
2. **Ordered Execution**: Layers are executed in the order they are registered in `setup_layers` (stderr, OpenTelemetry, KVP if enabled, file if available)
3. **Complementary Functions**: Each layer serves a specific purpose in the tracing ecosystem:
   - `EmitKVPLayer` focuses on Azure Hyper-V integration
   - `OpenTelemetryLayer` handles standardized tracing and exports
   - `stderr_layer` provides immediate visibility for debugging

### Configuration

The tracing system's behavior is controlled through configuration files and environment variables, allowing morecontrol over what data is captured and where it's sent:

- `telemetry.kvp_diagnostics` (config): Enables/disables KVP emission. Default: `true`.
- `telemetry.kvp_filter` (config): Optional `EnvFilter`-style directives to select which spans/events go to KVP.
- `azure_init_log_path.path` (config): Target path for the file layer. Default: `/var/log/azure-init.log`.
- `AZURE_INIT_KVP_FILTER` (env): Overrides `telemetry.kvp_filter`. Precedence: env > config > default.
- `AZURE_INIT_LOG` (env): Controls stderr and file fmt layers’ levels (defaults: stderr=`error`, file=`info`).

The KVP layer uses a conservative default filter aimed at essential provisioning signals; adjust that via the settings above as needed.
For more on how to use these configuration variables, see the [configuration documentation](./configuration.md#complete-configuration-example).

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

## Reference Documentation

For more details on how the Hyper-V Data Exchange Service works, refer to the official documentation:
[Hyper-V Data Exchange Service (KVP)](https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/reference/integration-services#hyper-v-data-exchange-service-kvp)

For OpenTelemetry integration details:
[OpenTelemetry for Rust](https://opentelemetry.io/docs/instrumentation/rust/)