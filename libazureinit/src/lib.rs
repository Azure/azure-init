// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub mod error;
pub mod goalstate;
pub(crate) mod http;
pub mod imds;
pub mod media;

mod provision;
pub use provision::{
    hostname::Provisioner as HostnameProvisioner,
    password::Provisioner as PasswordProvisioner,
    user::{Provisioner as UserProvisioner, User},
    Provision,
};

// Re-export as the Client is used in our API.
pub use reqwest;
