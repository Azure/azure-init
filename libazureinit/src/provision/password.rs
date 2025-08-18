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
            Self::FakePasswd => mock_passwd(user),
        }
    }
}

/// Manages the user's password during provisioning.
///
/// By default, `azure-init` does not support provisioning users with passwords.
/// This function ensures that password-based login is disabled for the provisioned
/// user by locking the account's password using `passwd -l`.
///
/// If the `User` object contains a password, this function will return a
/// `NonEmptyPassword` error, as provisioning with a password is not a
/// supported feature.
#[instrument(skip_all)]
fn passwd(user: &User) -> Result<(), Error> {
    let path_passwd = env!("PATH_PASSWD");

    if user.password.is_none() {
        tracing::info!(
            target = "libazureinit::password::lock",
            "Locking password for user '{}' to disable password-based login.",
            user.name
        );
        let mut command = Command::new(path_passwd);
        command.arg("-l").arg(&user.name);
        crate::run(command)?;
    } else {
        // creating user with a non-empty password is not allowed.
        tracing::error!("Attempted to provision user with a password, which is not supported.");
        return Err(Error::NonEmptyPassword);
    }

    Ok(())
}

#[instrument(skip_all)]
#[cfg(test)]
fn mock_passwd(user: &User) -> Result<(), Error> {
    if user.password.is_none() {
        Ok(())
    } else {
        // creating user with a non-empty password is not allowed.
        return Err(Error::NonEmptyPassword);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::User;

    #[test]
    fn test_passwd_with_no_password_succeeds() {
        // Test that passwd function succeeds when user has no password
        let user = User::new("azureuser", []);
        assert!(user.password.is_none());

        let result = mock_passwd(&user);

        // The function should complete without error when user has no password
        assert!(result.is_ok());
    }

    #[test]
    fn test_passwd_with_password_returns_error() {
        // Test that passwd function returns Error::NonEmptyPassword error when user has a password
        let user = User::new("azureuser", []).with_password("somepassword");
        assert!(user.password.is_some());

        let result = mock_passwd(&user);

        // Should return NonEmptyPassword error
        assert!(matches!(result, Err(Error::NonEmptyPassword)));
    }

    #[test]
    fn test_passwd_provisioner_set_with_no_password() {
        // Test the PasswordProvisioner::set method with no password
        let provisioner = PasswordProvisioner::FakePasswd;
        let user = User::new("azureuser", []);

        let result = provisioner.set(&user);

        // Should succeed without calling real passwd command
        assert!(result.is_ok());
    }

    #[test]
    fn test_passwd_provisioner_set_with_password() {
        // Test the PasswordProvisioner::set method with a password
        let provisioner = PasswordProvisioner::FakePasswd;
        let user = User::new("azureuser", []).with_password("somepassword");

        let result = provisioner.set(&user);

        // Should return NonEmptyPassword error
        assert!(matches!(result, Err(Error::NonEmptyPassword)));
    }
}
