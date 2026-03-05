// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! # libazureinit-kvp
//!
//! A layered library for Hyper-V KVP (Key-Value Pair) storage.
//!
//! The library is organized into four layers:
//!
//! - **Layer 0 (`KvpStore` trait):** Core storage abstraction with
//!   `write`, `read`, `entries`, and `delete` operations.
//!   - [`HyperVKvpStore`]: Production implementation using the binary
//!     Hyper-V pool-file format with flock-based concurrency.
//!   - [`InMemoryKvpStore`]: HashMap-backed test double.
//!
//! - **Layer 1 (`DiagnosticsKvp`):** Typed access to diagnostic
//!   key-value entries with value splitting for the Azure platform's
//!   1,022-byte read limit.
//!
//! - **Layer 2 (`TracingKvpLayer`):** A `tracing_subscriber::Layer`
//!   that translates span and event data into diagnostic KVP entries.
//!
//! - **Layer 3 (`ProvisioningReport`):** Typed accessor for the
//!   `PROVISIONING_REPORT` KVP key used by the Azure platform.

use std::io;
use std::path::{Path, PathBuf};

pub mod diagnostics;
pub mod hyperv;
pub mod memory;
pub mod provisioning;
pub mod tracing_layer;

pub use diagnostics::{DiagnosticEvent, DiagnosticsKvp};
pub use hyperv::HyperVKvpStore;
pub use memory::InMemoryKvpStore;
pub use provisioning::ProvisioningReport;
pub use tracing_layer::TracingKvpLayer;

/// The default event prefix used when no custom prefix is provided.
pub const EVENT_PREFIX: &str =
    concat!("azure-init-", env!("CARGO_PKG_VERSION"));

const DEFAULT_KVP_FILE_PATH: &str = "/var/lib/hyperv/.kvp_pool_1";

/// The fundamental storage abstraction for KVP.
///
/// Implementations handle encoding, persistence, and concurrency
/// internally. Higher layers build on this trait without knowledge of
/// the underlying storage mechanism.
pub trait KvpStore: Send + Sync {
    /// Write a key-value pair into the store.
    fn write(&self, key: &str, value: &str) -> io::Result<()>;

    /// Read the value for a given key, returning `None` if absent.
    fn read(&self, key: &str) -> io::Result<Option<String>>;

    /// Return all key-value pairs currently in the store.
    fn entries(&self) -> io::Result<Vec<(String, String)>>;

    /// Remove a key. Returns `true` if the key existed.
    fn delete(&self, key: &str) -> io::Result<bool>;
}

/// Configuration options for creating a [`Kvp`] client.
#[derive(Clone, Debug)]
pub struct KvpOptions {
    pub vm_id: Option<String>,
    pub event_prefix: Option<String>,
    pub file_path: PathBuf,
    pub truncate_on_start: bool,
}

impl Default for KvpOptions {
    fn default() -> Self {
        Self {
            vm_id: None,
            event_prefix: None,
            file_path: PathBuf::from(DEFAULT_KVP_FILE_PATH),
            truncate_on_start: true,
        }
    }
}

impl KvpOptions {
    pub fn vm_id<T: Into<String>>(mut self, vm_id: T) -> Self {
        self.vm_id = Some(vm_id.into());
        self
    }

    pub fn event_prefix<T: Into<String>>(mut self, event_prefix: T) -> Self {
        self.event_prefix = Some(event_prefix.into());
        self
    }

    pub fn file_path<T: AsRef<Path>>(mut self, file_path: T) -> Self {
        self.file_path = file_path.as_ref().to_path_buf();
        self
    }

    pub fn truncate_on_start(mut self, truncate_on_start: bool) -> Self {
        self.truncate_on_start = truncate_on_start;
        self
    }
}

/// Top-level KVP client that wires together all layers.
///
/// Callers interact with the layers through the public fields:
/// - `store`: raw key-value access (`write`, `read`, `entries`)
/// - `diagnostics`: typed diagnostic event emission
/// - `tracing_layer`: tracing subscriber layer for automatic KVP emission
pub struct Kvp<S: KvpStore + 'static> {
    pub store: S,
    pub diagnostics: DiagnosticsKvp<S>,
    pub tracing_layer: TracingKvpLayer<S>,
}

impl Kvp<HyperVKvpStore> {
    /// Production constructor from explicit options.
    ///
    /// `options.vm_id` must be `Some` -- the caller (typically
    /// `logging.rs`) is responsible for resolving the VM ID before
    /// calling this.
    pub fn with_options(options: KvpOptions) -> io::Result<Self> {
        let vm_id = options.vm_id.ok_or_else(|| {
            io::Error::other("vm_id is required in KvpOptions")
        })?;
        let event_prefix = options
            .event_prefix
            .unwrap_or_else(|| EVENT_PREFIX.to_string());

        let store = HyperVKvpStore::new(&options.file_path);

        if options.truncate_on_start {
            store.truncate_if_stale()?;
        }

        Ok(Self::from_store(store, &vm_id, &event_prefix))
    }
}

impl<S: KvpStore + Clone + 'static> Kvp<S> {
    /// Construct a `Kvp` client from any `KvpStore` implementation.
    pub fn from_store(store: S, vm_id: &str, event_prefix: &str) -> Self {
        let diagnostics =
            DiagnosticsKvp::new(store.clone(), vm_id, event_prefix);
        let tracing_layer = TracingKvpLayer::new(DiagnosticsKvp::new(
            store.clone(),
            vm_id,
            event_prefix,
        ));
        Self {
            store,
            diagnostics,
            tracing_layer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_store_wires_layers() {
        let store = InMemoryKvpStore::default();
        let kvp = Kvp::from_store(store.clone(), "test-vm", "test-prefix");

        kvp.diagnostics
            .emit(&diagnostics::DiagnosticEvent {
                level: "INFO".into(),
                name: "test::event".into(),
                span_id: "span-1".into(),
                message: "hello".into(),
                timestamp: chrono::Utc::now(),
            })
            .unwrap();

        let entries = store.entries().unwrap();
        assert!(!entries.is_empty());
        assert!(entries.iter().any(|(k, _)| k.contains("test-prefix")));
    }

    #[test]
    fn test_provisioning_report_through_kvp_client() {
        let store = InMemoryKvpStore::default();
        let kvp = Kvp::from_store(store, "vm-123", "prefix");

        let report = ProvisioningReport::success("vm-123");
        report.write_to(&kvp.store).unwrap();

        let read_back = ProvisioningReport::read_from(&kvp.store).unwrap();
        assert!(read_back.is_some());
        let read_back = read_back.unwrap();
        assert_eq!(read_back.result, "success");
        assert_eq!(read_back.vm_id, "vm-123");
    }

    #[test]
    fn test_with_options_requires_vm_id() {
        let opts = KvpOptions::default();
        let result = Kvp::with_options(opts);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("vm_id"));
    }
}
