# libazureinit-kvp: Layered KVP Architecture

## Overview

`libazureinit-kvp` is a standalone workspace crate that provides a layered
library for Hyper-V KVP (Key-Value Pair) storage. It replaces the former
`kvp.rs` module in `libazureinit` with independently testable
layers and synchronous, flock-based I/O.

The crate is consumed by `libazureinit` (via `logging.rs`) and is also
available to external callers who want to emit KVP diagnostics or
provisioning reports.

## Architecture

The library is organized into four layers, stacked from low-level storage
up to tracing integration:

```
┌─────────────────────────────────────────────────┐
│  Kvp<S>  (top-level client, wires layers)       │
├─────────────────────────────────────────────────┤
│  Layer 3: ProvisioningReport                    │
│           Typed accessor for PROVISIONING_REPORT│
├─────────────────────────────────────────────────┤
│  Layer 2: TracingKvpLayer<S>                    │
│           tracing_subscriber::Layer impl        │
├─────────────────────────────────────────────────┤
│  Layer 1: DiagnosticsKvp<S>                     │
│           Typed diagnostic events, splitting    │
├─────────────────────────────────────────────────┤
│  Layer 0: KvpStore trait                        │
│  ┌──────────────────┐  ┌─────────────────────┐  │
│  │ HyperVKvpStore   │  │ InMemoryKvpStore    │  │
│  │ (production)     │  │ (test double)       │  │
│  └──────────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────┘
```

### Layer 0: `KvpStore` trait

The fundamental storage abstraction. All higher layers are generic over
`S: KvpStore`, making them testable without the filesystem.

```rust
pub trait KvpStore: Send + Sync {
    fn write(&self, key: &str, value: &str) -> io::Result<()>;
    fn read(&self, key: &str) -> io::Result<Option<String>>;
    fn entries(&self) -> io::Result<Vec<(String, String)>>;
    fn delete(&self, key: &str) -> io::Result<bool>;
}
```

`HyperVKvpStore` — Production implementation that reads and writes the
binary Hyper-V pool file (`/var/lib/hyperv/.kvp_pool_1`). Each record is
2,560 bytes (512-byte key + 2,048-byte value). Concurrency is handled via
`flock` (shared locks for reads, exclusive locks for writes).

`InMemoryKvpStore` — `HashMap`-backed test double. Implements
`Clone` (via `Arc<Mutex<HashMap>>`), so clones share state, matching the
semantics expected by higher layers.

### Layer 1: `DiagnosticsKvp<S>`

Provides typed access to diagnostic events. Handles:

- Key generation using the format
  `{event_prefix}|{vm_id}|{level}|{name}|{span_id}`
- Value splitting at the 1,022-byte Azure platform read limit
- Parsing diagnostic keys back into `DiagnosticEvent` structs

```rust
kvp.diagnostics.emit(&DiagnosticEvent {
    level: "INFO".into(),
    name: "provision:user".into(),
    span_id: "abc-123".into(),
    message: "User created".into(),
    timestamp: Utc::now(),
})?;
```

### Layer 2: `TracingKvpLayer<S>`

A `tracing_subscriber::Layer` implementation that automatically converts
`tracing` spans and events into KVP diagnostic entries. It:

- Detects events with a `health_report` field and writes them directly
  to the store under the `PROVISIONING_REPORT` key
- Converts all other events into `DiagnosticEvent` structs and calls
  `DiagnosticsKvp::emit`
- Tracks span start/end times and emits timing entries on span close

This layer is registered alongside the other tracing layers (stderr,
file, OpenTelemetry) in `setup_layers`.

### Layer 3: `ProvisioningReport`

A typed accessor for the `PROVISIONING_REPORT` KVP key used by the
Azure platform. Supports:

- `ProvisioningReport::success(vm_id)` — builds a success report
- `ProvisioningReport::error(vm_id, reason)` — builds
  an error report
- `report.write_to(&store)` / `ProvisioningReport::read_from(&store)` —
  serialization via the pipe-delimited wire format

The wire format is fully compatible with the existing `health.rs`
`encode_report` output.

### `Kvp<S>` Client

The top-level struct that wires the layers together:

```rust
pub struct Kvp<S: KvpStore + 'static> {
    pub store: S,
    pub diagnostics: DiagnosticsKvp<S>,
    pub tracing_layer: TracingKvpLayer<S>,
}
```

Constructors:

- `Kvp::with_options(KvpOptions)` — production path, creates a
  `Kvp<HyperVKvpStore>`, requires `vm_id` to be set
- `Kvp::from_store(store, vm_id, event_prefix)` — generic constructor
  for any `KvpStore` implementation (useful for testing)

## What the Crate Provides vs. What azure-init Adds

### libazureinit-kvp (standalone)

External callers depend on `libazureinit-kvp` directly and get:

- `KvpStore` trait + `HyperVKvpStore` + `InMemoryKvpStore`
- `DiagnosticsKvp<S>` for typed diagnostic events with value splitting
- `TracingKvpLayer<S>` for automatic tracing-to-KVP bridging
- `ProvisioningReport` for reading/writing provisioning reports
- `Kvp<S>` client that wires the layers together
- `KvpOptions` builder for production construction

The crate has **no filtering, no config system, and no awareness of
azure-init's log levels or environment variables**. It emits every
span/event that reaches the `TracingKvpLayer`. Callers are responsible
for applying their own `tracing_subscriber::EnvFilter` (or other filter)
via `.with_filter(...)` on the `TracingKvpLayer` if they want selective
emission.

Example for an external caller:

```rust
use libazureinit_kvp::{Kvp, KvpOptions};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Registry};

let kvp = Kvp::with_options(
    KvpOptions::default().vm_id("my-vm-id"),
)?;

let subscriber = Registry::default().with(
    kvp.tracing_layer.with_filter(EnvFilter::new("info")),
);
```

### azure-init / libazureinit (via `logging.rs`)

azure-init adds orchestration and policy on top of the raw kvp crate:

- `setup_layers()`: wires `TracingKvpLayer` alongside stderr, file,
  and OpenTelemetry layers into a single tracing subscriber.
- KVP filter resolution with three-tier precedence:
  `AZURE_INIT_KVP_FILTER` env var > `telemetry.kvp_filter` config >
  hardcoded default filter (conservative, provisioning-signal-only).
- vm_id resolution via `get_vm_id()` (reads DMI/SMBIOS data) before
  constructing the `Kvp` client — the kvp crate itself does not perform
  platform-specific ID lookups.
- config-driven enable/disable (`telemetry.kvp_diagnostics`): when
  `false`, the KVP layer is not registered at all.

This separation means the kvp crate stays dependency-light and
platform-agnostic (beyond the Hyper-V pool file format), while
azure-init owns the policy decisions about what gets logged where.

## Integration with azure-init

### `logging.rs`

`setup_layers` creates the `Kvp` client and registers
`kvp.tracing_layer` as one of the subscriber layers:

```rust
pub fn setup_layers(
    vm_id: &str,
    config: &Config,
) -> Result<Box<dyn Subscriber + Send + Sync + 'static>, anyhow::Error>
```

The function no longer requires a `CancellationToken` or returns a
`JoinHandle` — all KVP I/O is synchronous.

### `main.rs`

The KVP shutdown block (`graceful_shutdown.cancel()`, `handle.await`)
has been removed. The `main` function simply calls `setup_layers` and
uses the returned subscriber directly.

### `health.rs` / `error.rs`

These files are unchanged. The `encode_report` function in `health.rs`
continues to format the pipe-delimited report string that flows through
the tracing layer to KVP via the `health_report` field detection.

## Configuration

- `telemetry.kvp_diagnostics` (config): Enables/disables KVP emission.
  Default: `true`.
- `telemetry.kvp_filter` (config): Optional `EnvFilter`-style directives
  to select which spans/events go to KVP.
- `AZURE_INIT_KVP_FILTER` (env): Overrides `telemetry.kvp_filter`.
  Precedence: env > config > default.
- `AZURE_INIT_LOG` (env): Controls stderr and file layer log levels
  (defaults: stderr=`error`, file=`info`).

The KVP layer uses a conservative default filter aimed at essential
provisioning signals. See the
[configuration documentation](./configuration.md#complete-configuration-example)
for details.

## Truncation and Locking

On startup, `Kvp::with_options` calls `HyperVKvpStore::truncate_if_stale`,
which checks the pool file's mtime against the system uptime. If the file
predates the current boot, it is truncated to discard stale data from a
previous session. This operation uses an exclusive flock; if the lock
cannot be acquired, initialization continues without truncation.

All subsequent writes use per-operation exclusive flocks to ensure
safe concurrent access from multiple threads or processes.

## Usage Examples

### Using the KVP Client API

```rust
use libazureinit_kvp::{Kvp, KvpOptions, DiagnosticEvent, ProvisioningReport};
use chrono::Utc;

fn main() -> std::io::Result<()> {
    let vm_id = "00000000-0000-0000-0000-000000000001";
    let kvp = Kvp::with_options(
        KvpOptions::default().vm_id(vm_id),
    )?;

    // Emit a diagnostic event
    kvp.diagnostics.emit(&DiagnosticEvent {
        level: "INFO".into(),
        name: "provision:start".into(),
        span_id: "span-1".into(),
        message: "Provisioning started".into(),
        timestamp: Utc::now(),
    })?;

    // Write a provisioning report
    let report = ProvisioningReport::success(vm_id);
    report.write_to(&kvp.store)?;

    Ok(())
}
```

### Full Provisioning Flow Example

A more realistic example showing how to emit diagnostics and
provisioning reports through a provision-then-report workflow:

```rust
use libazureinit_kvp::{Kvp, KvpOptions, DiagnosticEvent, ProvisioningReport};
use chrono::Utc;

fn provision_vm(vm_id: &str) -> Result<(), String> {
    // ... actual provisioning logic ...
    // Return Ok(()) on success, Err("reason") on failure
    Ok(())
}

fn main() {
    let vm_id = "00000000-0000-0000-0000-000000000001";

    let kvp = match Kvp::with_options(KvpOptions::default().vm_id(vm_id)) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("KVP init failed (non-fatal): {e}");
            return;
        }
    };

    // Signal that provisioning is in progress
    let _ = kvp.diagnostics.emit(&DiagnosticEvent {
        level: "INFO".into(),
        name: "provision:start".into(),
        span_id: "main".into(),
        message: format!("Provisioning in progress for vm_id={vm_id}"),
        timestamp: Utc::now(),
    });

    match provision_vm(vm_id) {
        Ok(()) => {
            let _ = kvp.diagnostics.emit(&DiagnosticEvent {
                level: "INFO".into(),
                name: "provision:complete".into(),
                span_id: "main".into(),
                message: "Provisioning completed successfully".into(),
                timestamp: Utc::now(),
            });
            let report = ProvisioningReport::success(vm_id);
            let _ = report.write_to(&kvp.store);
        }
        Err(reason) => {
            let _ = kvp.diagnostics.emit(&DiagnosticEvent {
                level: "ERROR".into(),
                name: "provision:failed".into(),
                span_id: "main".into(),
                message: format!("Provisioning failed: {reason}"),
                timestamp: Utc::now(),
            });
            let report = ProvisioningReport::error(vm_id, &reason);
            let _ = report.write_to(&kvp.store);
        }
    }
}
```

Note the use of `let _ =` for all KVP operations -- KVP errors are
non-fatal and should never block provisioning. This matches the
principle used throughout azure-init.

### Using Tracing Instrumentation

`azure-init` uses `setup_layers` to register the KVP tracing layer.
Code instrumented with `#[instrument]` and `event!` automatically
emits KVP entries:

```rust
use tracing::{event, instrument, Level};

#[instrument(fields(user_id = ?user.id))]
fn provision_user(user: &User) -> Result<(), Error> {
    event!(Level::INFO, "Starting user provisioning");
    // ... provisioning logic ...
    event!(Level::INFO, "User provisioning completed");
    Ok(())
}
```

### Testing with InMemoryKvpStore

```rust
use libazureinit_kvp::{Kvp, InMemoryKvpStore, ProvisioningReport};

let store = InMemoryKvpStore::default();
let kvp = Kvp::from_store(store.clone(), "test-vm", "test-prefix");

let report = ProvisioningReport::success("test-vm");
report.write_to(&kvp.store).unwrap();

let read_back = ProvisioningReport::read_from(&kvp.store).unwrap();
assert!(read_back.is_some());
```

## Reference Documentation

- [Hyper-V Data Exchange Service (KVP)](https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/reference/integration-services#hyper-v-data-exchange-service-kvp)
- [OpenTelemetry for Rust](https://opentelemetry.io/docs/instrumentation/rust/)
