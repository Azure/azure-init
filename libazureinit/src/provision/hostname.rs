// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use tracing::instrument;

use crate::error::Error;

use crate::provision::HostnameProvisioner;

impl HostnameProvisioner {
    /// Set the system hostname via the configured provisioner.
    ///
    /// Delegates to the active `HostnameProvisioner` implementation (e.g. `hostnamectl`).
    /// Expects a pre-validated hostname; no format validation is performed here.
    /// Returns an error if the underlying tool fails to set the hostname.
    /// In tests, `FakeHostnamectl` is a no-op.
    pub(crate) fn set(&self, hostname: impl AsRef<str>) -> Result<(), Error> {
        match self {
            Self::Hostnamectl => hostnamectl(hostname.as_ref()),
            #[cfg(test)]
            Self::FakeHostnamectl => Ok(()),
        }
    }
}

/// Invoke `hostnamectl set-hostname` to change the hostname.
///
/// The binary path is taken from the compile-time `PATH_HOSTNAMECTL`.
/// Requires sufficient privileges; returns an error if the command fails.
#[instrument(skip_all)]
pub fn hostnamectl(hostname: &str) -> Result<(), Error> {
    let path_hostnamectl = env!("PATH_HOSTNAMECTL");

    let mut command = Command::new(path_hostnamectl);
    command.arg("set-hostname").arg(hostname);
    crate::run(command)
}
