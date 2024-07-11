// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use tracing::instrument;

use crate::error::Error;

/// Available tools to set the host's hostname.
#[derive(strum::EnumIter, Debug, Clone)]
#[non_exhaustive]
pub enum Provisioner {
    /// Use the `hostnamectl` command from `systemd`.
    Hostnamectl,
    #[cfg(test)]
    FakeHostnamectl,
}

impl Provisioner {
    pub(crate) fn set(&self, hostname: impl AsRef<str>) -> Result<(), Error> {
        match self {
            Self::Hostnamectl => hostnamectl(hostname.as_ref()),
            #[cfg(test)]
            Self::FakeHostnamectl => Ok(()),
        }
    }
}

#[instrument(skip_all)]
fn hostnamectl(hostname: &str) -> Result<(), Error> {
    let path_hostnamectl = env!("PATH_HOSTNAMECTL");

    let status = Command::new(path_hostnamectl)
        .arg("set-hostname")
        .arg(hostname)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::SubprocessFailed {
            command: path_hostnamectl.to_string(),
            status,
        })
    }
}
