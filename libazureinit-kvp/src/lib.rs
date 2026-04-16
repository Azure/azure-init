// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `libazureinit-kvp` provides a storage trait and unified KVP pool
//! implementation for Hyper-V/Azure guests.
//!
//! - [`KvpStore`]: storage interface used by higher layers.
//! - [`KvpPoolStore`]: KVP pool file implementation with
//!   [`PoolMode`]-based policy.

mod error;
mod store;

pub use error::KvpError;
pub use store::{KvpPool, KvpPoolStore, KvpStore, PoolMode};
