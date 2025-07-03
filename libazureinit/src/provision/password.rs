// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use std::path::PathBuf;

use tracing::instrument;

use crate::{error::Error, User};

use crate::provision::PasswordProvisioner;

impl PasswordProvisioner {
    pub(crate) fn set(&self, user: &User) -> Result<(), Error> {
        match self {
            Self::Passwd => passwd(user),
            #[cfg(test)]
            Self::FakePasswd => Ok(()),
        }
    }
}

// Determines the appropriate SSH configuration file path based on the filesystem.
// If the "/etc/ssh/sshd_config.d" directory exists, it returns the path for a drop-in configuration file.
// Otherwise, it defaults to the main SSH configuration file at "/etc/ssh/sshd_config".
pub(crate) fn get_sshd_config_path() -> &'static str {
    if PathBuf::from("/etc/ssh/sshd_config.d").is_dir() {
        "/etc/ssh/sshd_config.d/50-azure-init.conf"
    } else {
        "/etc/ssh/sshd_config"
    }
}

#[instrument(skip_all)]
fn passwd(user: &User) -> Result<(), Error> {
    let path_passwd = env!("PATH_PASSWD");

    if user.password.is_none() {
        let mut command = Command::new(path_passwd);
        command.arg("-l").arg(&user.name);
        crate::run(command)?;
    } else {
        // creating user with a non-empty password is not allowed.
        return Err(Error::NonEmptyPassword);
    }

    Ok(())
}
