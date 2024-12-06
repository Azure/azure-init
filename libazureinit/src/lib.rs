// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
pub mod config;
pub use config::{HostnameProvisioner, PasswordProvisioner, UserProvisioner};
pub mod error;
pub mod goalstate;
pub(crate) mod http;
pub mod imds;
pub mod media;

mod provision;
pub use provision::{user::User, Provision};

#[cfg(test)]
mod unittest;

// Re-export as the Client is used in our API.
pub use reqwest;
