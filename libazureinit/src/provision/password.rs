// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use tracing::instrument;

use crate::{error::Error, User};

use super::ssh::update_sshd_config;

/// Available tools to set the user's password (if a password is provided).
#[derive(strum::EnumIter, Debug, Clone)]
#[non_exhaustive]
pub enum Provisioner {
    /// Use the `passwd` command from `shadow-utils`.
    Passwd,
    #[cfg(test)]
    FakePasswd,
}

impl Provisioner {
    pub(crate) fn set(&self, user: &User) -> Result<(), Error> {
        match self {
            Self::Passwd => passwd(user),
            #[cfg(test)]
            Self::FakePasswd => Ok(()),
        }
    }
}

#[instrument(skip_all)]
fn passwd(user: &User) -> Result<(), Error> {
    // Update the sshd configuration to allow password authentication.
    let sshd_config_path = "/etc/ssh/sshd_config.d/50-azure-init.conf";
    let ret = update_sshd_config(sshd_config_path);
    if ret.is_err() {
        return Err(Error::UpdateSshdConfig);
    }
    let path_passwd = env!("PATH_PASSWD");

    if user.password.is_none() {
        let status = Command::new(path_passwd)
            .arg("-d")
            .arg(&user.name)
            .status()?;
        if !status.success() {
            return Err(Error::SubprocessFailed {
                command: path_passwd.to_string(),
                status,
            });
        }
    } else {
        // creating user with a non-empty password is not allowed.
        return Err(Error::NonEmptyPassword);
    }

    Ok(())
}
