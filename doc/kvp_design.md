# Design: azure-init KVP APIs

## Motivation

**Need to External Use.** Other Linux distros
that want to emit Hyper-V KVP data had to pull in the full `libazureinit`
crate -- with all its dependencies on tokio, OpenTelemetry, IMDS, and
configuration -- just to write a key-value pair. There was no way to use
KVP storage without also setting up the tracing subscriber, the async
runtime, and the graceful shutdown machinery.

**Unnecessarily complex lifecycle.** The async channel + background writer
pattern required callers to manage a `CancellationToken`, await a
`JoinHandle`, and call `close().await` to drain buffered writes.

**Unclear boundaries.** Encoding, splitting, tracing, provisioning reports,
and file I/O were interleaved in the same structs and methods. Someone
looking at how diagnostics are structured had to also understand binary
record encoding. Someone looking at provisioning reports had to understand
the tracing layer's `health_report` detection logic.

### Why the new architecture is better

The rearchitecture extracts KVP into its own crate (`libazureinit-kvp`)
with a layered design where each layer has a single, clearly scoped
responsibility:

- Simple external usage, as other distros depend on `libazureinit-kvp`.

- Synchronous I/O, due to replacing the async channel + background writer with
  direct `flock` + write + unlock per operation. No `close()`, no
  `CancellationToken`, no `JoinHandle`. Writes go to disk immediately.

- Testable at every layer. Any layer can be tested against
  `InMemoryKvpStore` (a `HashMap`-backed test double) without touching the
  filesystem. Binary encoding tests are isolated in `HyperVKvpStore`.

- There is a clear separation of concerns. Storage knows nothing about
  diagnostics, diagnostics knows nothing about tracing, and provisioning
  reports know nothing about either. Each layer can be understood, modified,
  and tested independently.

## Implementation Approach

The new API is designed around two goals:

1. **Keep external usage simple and explicit.** External callers get a
   small, dependency-light crate with straightforward synchronous APIs.
2. **Preserve azure-init's internal tracing-based emission path.**
   `setup_layers` continues to wire a `TracingKvpLayer` into the tracing
   subscriber stack, so `#[instrument]` and `event!` macros emit KVP data
   automatically.

## Crate Structure

KVP is extracted into its own workspace crate with a clear dependency
graph:

```
azure-init  ──►  libazureinit  ──►  libazureinit-kvp
```

`libazureinit` depends on `libazureinit-kvp` via a workspace path
dependency. `azure-init` (the binary) depends only on `libazureinit`.
External callers can depend on `libazureinit-kvp` directly without
pulling in the rest of the azure-init stack.

### Workspace layout

```
azure-init/                       # workspace root
├── Cargo.toml                    # [workspace] members
├── src/main.rs                   # azure-init binary
├── libazureinit/
│   ├── Cargo.toml                # depends on libazureinit-kvp
│   └── src/
│       ├── lib.rs
│       ├── logging.rs            # wires TracingKvpLayer into subscriber
│       ├── health.rs             # wireserver reporting (unchanged)
│       ├── error.rs              # error types with report encoding
│       └── ...
└── libazureinit-kvp/
    ├── Cargo.toml
    └── src/
        ├── lib.rs                # KvpStore trait, KvpOptions, Kvp<S>
        ├── hyperv.rs             # HyperVKvpStore, binary encode/decode
        ├── memory.rs             # InMemoryKvpStore (test double)
        ├── diagnostics.rs        # DiagnosticEvent, DiagnosticsKvp<S>
        ├── tracing_layer.rs      # TracingKvpLayer<S>, StringVisitor
        └── provisioning.rs       # ProvisioningReport
```

`libazureinit-kvp` has a minimal dependency footprint: `chrono`, `csv`,
`fs2`, `sysinfo`, `tracing`, `tracing-subscriber`, `uuid`. No tokio, no
OpenTelemetry, no configuration system.

## Layered Architecture

The library is organized into four layers, each with a single
responsibility. Higher layers depend only on the `KvpStore` trait, never
on a concrete implementation:

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

For detailed API signatures, struct definitions, and code examples for
each layer, see the
[Architecture](./libazurekvp.md#architecture) section in `libazurekvp.md`.

### Layer 0 -- `KvpStore` trait

Core storage abstraction (`write`, `read`, `entries`, `delete`). Two
implementations:

- **`HyperVKvpStore`** -- production. Reads and writes the binary Hyper-V
  pool-file format: fixed-size 2,560-byte records (512-byte key +
  2,048-byte value, zero-padded). Concurrency via `flock` (exclusive for
  writes, shared for reads).
- **`InMemoryKvpStore`** -- test double. `HashMap`-backed, no filesystem.
  Implements `Clone` via `Arc<Mutex<...>>` so clones share state.

`write()` at this layer writes exactly one record per call. Value
splitting across multiple records is **not** handled here -- that is the
responsibility of higher layers that understand their data semantics.

### Layer 1 -- `DiagnosticsKvp<S>`

Typed diagnostic event emission. Handles key generation using the format
`{event_prefix}|{vm_id}|{level}|{name}|{span_id}`
and value splitting at the 1,022-byte Azure platform read limit (UTF-16:
511 characters + null terminator). This keeps `HyperVKvpStore` simple
while preserving the chunking semantics the host expects.

### Layer 2 -- `TracingKvpLayer<S>`

A `tracing_subscriber::Layer` that translates `#[instrument]` spans and
`event!` calls into `DiagnosticsKvp::emit()` calls. Detects
`health_report` fields and writes them as `PROVISIONING_REPORT` entries
directly to the store.

### Layer 3 -- `ProvisioningReport`

Typed accessor for the `PROVISIONING_REPORT` KVP key with
`write_to()` / `read_from()` and `success()` / `error()` constructors.
The wire format (pipe-delimited `key=value` segments) is fully compatible
with the existing `health.rs` `encode_report` output.

### `Kvp<S>` client

Wires all layers together with public fields (`store`, `diagnostics`,
`tracing_layer`). Constructed via `Kvp::with_options(KvpOptions)`
(production) or `Kvp::from_store(store, vm_id, event_prefix)` (testing).

## Design Principles

### Non-fatal error handling

KVP is a telemetry side-channel. It must never block or fail
provisioning. All KVP operations return `io::Result`, but callers
(including azure-init itself) use `let _ =` to discard errors after
logging. If KVP initialization fails entirely, `setup_layers` logs an
error and continues without the KVP layer.

### Two-consumer model

The library serves two distinct consumers with different needs:

1. **External callers** (other distros, provisioning agents) -- use the
   `Kvp` client API directly. They get synchronous, explicit calls with
   no filtering, no config, no tracing subscriber setup. They own their
   own lifecycle.

2. **azure-init** -- uses the `TracingKvpLayer` wired into the tracing
   subscriber via `setup_layers()`. KVP emission is automatic through
   `#[instrument]` and `event!`. Filtering, config, and subscriber
   orchestration are handled by `logging.rs`.

This separation is documented in detail in
[What the Crate Provides vs. What azure-init Adds](./libazurekvp.md#what-the-crate-provides-vs-what-azure-init-adds)
in `libazurekvp.md`.

### Testability via trait generics

Every layer above Layer 0 is generic over `S: KvpStore`. This means
diagnostics, tracing, and provisioning report logic can all be tested
against `InMemoryKvpStore` without touching the filesystem. Only
`HyperVKvpStore` tests need temp files. The test suite has 41 tests
covering all layers.

### `KvpOptions`

Configures the production `Kvp<HyperVKvpStore>` client:

| Field | Type | Default |
|-------|------|---------|
| `vm_id` | `Option<String>` | `None` (required -- caller must set) |
| `event_prefix` | `Option<String>` | `None` (falls back to `EVENT_PREFIX`) |
| `file_path` | `PathBuf` | `/var/lib/hyperv/.kvp_pool_1` |
| `truncate_on_start` | `bool` | `true` |

## Initialization Flow

`Kvp::with_options(options)` performs:

1. Validate that `vm_id` is present (return error if `None`).
2. Resolve `event_prefix` (use provided value or fall back to
   `EVENT_PREFIX`, which is `"azure-init-{version}"`).
3. Create `HyperVKvpStore` pointing at `options.file_path`.
4. If `truncate_on_start` is `true`, call `store.truncate_if_stale()` to
   clear records from previous boots.
5. Call `Kvp::from_store(store, vm_id, event_prefix)` to wire layers.
6. Return the initialized client.

No background task is spawned. No channel is created. No shutdown token
is needed.

For details on the truncation and flock semantics, see
[Truncation and Locking](./libazurekvp.md#truncation-and-locking) in
`libazurekvp.md`.

## External Caller Model

For external callers (other distros, provisioning agents), the API is
intentionally simple -- no async runtime, no tracing setup, no
configuration system:

1. Construct a client (`Kvp::with_options`)
2. Emit diagnostics (`kvp.diagnostics.emit(...)`) and/or write
   provisioning reports (`ProvisioningReport::success(...).write_to(...)`)
3. Done. No `close()`, no `await`, no shutdown.

### Example 1: Minimal external caller

```rust
use libazureinit_kvp::{Kvp, KvpOptions, ProvisioningReport};

fn main() -> std::io::Result<()> {
    let vm_id = "00000000-0000-0000-0000-000000000001";
    let kvp = Kvp::with_options(
        KvpOptions::default().vm_id(vm_id),
    )?;

    ProvisioningReport::success(vm_id).write_to(&kvp.store)?;
    Ok(())
}
```

### Example 2: Full provisioning flow with diagnostics

```rust
use libazureinit_kvp::{Kvp, KvpOptions, DiagnosticEvent, ProvisioningReport};
use chrono::Utc;

fn provision_vm(vm_id: &str) -> Result<(), String> {
    // ... actual provisioning logic ...
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
            let _ = ProvisioningReport::success(vm_id)
                .write_to(&kvp.store);
        }
        Err(reason) => {
            let _ = kvp.diagnostics.emit(&DiagnosticEvent {
                level: "ERROR".into(),
                name: "provision:failed".into(),
                span_id: "main".into(),
                message: format!("Provisioning failed: {reason}"),
                timestamp: Utc::now(),
            });
            let _ = ProvisioningReport::error(vm_id, &reason)
                .write_to(&kvp.store);
        }
    }
}
```

Note: all KVP operations use `let _ =` because KVP errors are non-fatal
and must never block provisioning.

### Example 3: Custom identity and file path

```rust
use libazureinit_kvp::{Kvp, KvpOptions, DiagnosticEvent};
use chrono::Utc;

fn main() -> std::io::Result<()> {
    let kvp = Kvp::with_options(
        KvpOptions::default()
            .vm_id("00000000-0000-0000-0000-000000000042")
            .event_prefix("my-service-1.0")
            .file_path("/tmp/kvp_pool_test")
            .truncate_on_start(false),
    )?;

    kvp.diagnostics.emit(&DiagnosticEvent {
        level: "DEBUG".into(),
        name: "test:message".into(),
        span_id: "test".into(),
        message: "integration test message".into(),
        timestamp: Utc::now(),
    })?;

    Ok(())
}
```

## azure-init Internal Tracing Path

azure-init itself does not use the external caller model above.
Instead, it uses `setup_layers()` in `libazureinit::logging` to wire the
`TracingKvpLayer` into the tracing subscriber stack. This means all
existing `#[instrument]` and `event!` instrumentation automatically emits
KVP data without any code changes.

For details on how `setup_layers` constructs the subscriber, filter
precedence, and the separation between what the kvp crate provides vs.
what azure-init adds on top, see
[What the Crate Provides vs. What azure-init Adds](./libazurekvp.md#what-the-crate-provides-vs-what-azure-init-adds)
and
[Integration with azure-init](./libazurekvp.md#integration-with-azure-init)
in `libazurekvp.md`.

For configuration knobs (`telemetry.kvp_diagnostics`,
`telemetry.kvp_filter`, `AZURE_INIT_KVP_FILTER`, `AZURE_INIT_LOG`), see
[Configuration](./libazurekvp.md#configuration) in `libazurekvp.md`.

## Key Differences from Previous Design

| Concern | Previous | Current |
|---------|----------|---------|
| Storage | Async channel to background writer task | Synchronous `flock` + write + unlock per call |
| External API | Required tokio, `close().await`, shutdown tokens | Synchronous, no runtime, no lifecycle management |
| Tracing coupling | `EmitKVPLayer` owned channel, encoding, and `Layer` impl | `TracingKvpLayer` is a thin adapter over `DiagnosticsKvp<S>` |
| Provisioning reports | Encoded inline in `emit_health_report` | `ProvisioningReport` struct with `write_to()`/`read_from()` |
| Testability | Tests required tempfiles and real binary format | Any layer testable against `InMemoryKvpStore` |
| Crate boundary | Everything in `libazureinit::kvp` (private module) | Standalone `libazureinit-kvp` crate with public API |
| Dependency weight | Full `libazureinit` (tokio, OpenTelemetry, reqwest, etc.) | Minimal (`chrono`, `csv`, `fs2`, `tracing`, `uuid`) |
