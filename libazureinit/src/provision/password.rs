// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::process::Command;

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
