// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `libazureinit-kvp` provides a unified KVP pool file store for
//! Hyper-V/Azure guests.
//!
//! - [`KvpPoolStore`]: KVP pool file store with
//!   [`PoolMode`]-based policy.
//! - [`ProvisioningReport`]: structured provisioning health report that
//!   converts into KVP entries via [`ToKvp`] and is persisted with
//!   [`write_report`].

mod cli;
mod error;
mod report;
mod store;

pub use cli::run;
pub use error::KvpError;
pub use report::{write_report, ProvisioningReport, ToKvp};
pub use store::{KvpPool, KvpPoolStore, PoolMode};
