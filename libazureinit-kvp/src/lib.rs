// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `libazureinit-kvp` provides a unified KVP pool file store for
//! Hyper-V/Azure guests.
//!
//! - [`KvpPoolStore`]: KVP pool file store with
//!   [`PoolMode`]-based policy.

pub mod cli;
mod error;
mod store;

pub use error::KvpError;
pub use store::{KvpPool, KvpPoolStore, PoolMode};
