// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

use tracing::instrument;

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Provisioner {
    Passwd,
    #[cfg(test)]
    FakePasswd,
}

impl Provisioner {
    pub(crate) fn set(
        self,
        username: impl AsRef<str>,
        password: impl AsRef<str>,
    ) -> Result<(), Error> {
        match self {
            Self::Passwd => passwd(username.as_ref(), password.as_ref()),
            #[cfg(test)]
            Self::FakePasswd => Ok(()),
        }
    }
}

#[instrument(skip_all)]
fn passwd(username: &str, password: &str) -> Result<(), Error> {
    let path_passwd = env!("PATH_PASSWD");

    if password.is_empty() {
        let status =
            Command::new(path_passwd).arg("-d").arg(username).status()?;
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
