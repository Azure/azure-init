// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use tracing::instrument;

use crate::error::Error;

use crate::provision::HostnameProvisioner;

impl HostnameProvisioner {
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
