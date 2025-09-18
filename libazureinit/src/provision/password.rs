// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//!
//! Password provisioning behavior for `libazureinit`.
//!
//! - If `User.password` is present, the password is set securely using the
//!   `chpasswd` utility. The input format is `"username:password"` written to
//!   stdin. Stdout is discarded and stderr is inherited. Secrets are not
//!   included in argv or logs.
//! - If `User.password` is absent, the account is locked using `passwd -l`.
//!   The path to `passwd` is provided at build time via the `PATH_PASSWD`
//!   environment variable (see `libazureinit/build.rs`).
//!
//! Notes
//! - The reference binary `azure-init` does not set passwords; it constructs a
//!   `User` without calling `with_password`, which results in account locking.
//!   External consumers of `libazureinit` can opt-in to password usage by
//!   calling `User::with_password`.
//!
//! Example (library consumer)
//! ```ignore
//! use libazureinit::{Provision, User};
//! use libazureinit::config::Config;
//!
//! let user = User::new("azureuser", vec![]).with_password("s3cr3t");
//! let config = Config::default();
//! let _ = Provision::new("host", user, config, true).provision();
//! ```

use std::io::Write;
use std::process::{Command, Stdio};

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
/// This function supports two modes of operation:
/// - If a password is provided in the `User` object, it sets the password securely
///   using the `chpasswd` utility via stdin to avoid exposing secrets.
/// - If no password is provided, it disables password-based login by locking the
///   account's password using `passwd -l`.
///
/// Reference `azure-init` behavior: it never calls `User::with_password`.
/// Therefore `user.password` is `None`, and this function always locks the
/// account via `passwd -l` (there is no alternate locking path). Library
/// consumers that want a password must explicitly call `User::with_password`.
/// See `doc/azure_init_behavior.md` for details.
#[instrument(skip_all)]
fn passwd(user: &User) -> Result<(), Error> {
    if let Some(ref password) = user.password {
        // Set password securely via chpasswd using piped stdin to avoid exposing secrets
        let input = format!("{}:{}", user.name, password);
        let mut child = Command::new("chpasswd")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes())?;
        }

        let status = child.wait()?;
        if !status.success() {
            tracing::error!(username = %user.name, ?status, "chpasswd failed to set password");
            return Err(Error::SubprocessFailed {
                command: "chpasswd".to_string(),
                status,
            });
        }
        tracing::info!(target: "libazureinit::password::status", username = %user.name, "Successfully set password via chpasswd");
    } else {
        // No password provided; lock the account
        let path_passwd = env!("PATH_PASSWD");
        let mut command = Command::new(path_passwd);
        command.arg("-l").arg(&user.name);
        crate::run(command).map_err(|e| {
            tracing::error!(username = %user.name, error = ?e, "Failed to lock account via passwd -l");
            e
        })?;
        tracing::info!(target: "libazureinit::password::status", username = %user.name, "Locked account via passwd -l");
    }

    Ok(())
}

#[instrument(skip_all)]
#[cfg(test)]
fn mock_passwd(user: &User) -> Result<(), Error> {
    // In tests, simulate success for both setting a password and locking
    // (no external command execution).
    let _ = user;
    Ok(())
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
    fn test_passwd_with_password_succeeds() {
        // Test that passwd mock function succeeds when user has a password
        let user = User::new("azureuser", []).with_password("somepassword");
        assert!(user.password.is_some());

        let result = mock_passwd(&user);

        // Should succeed
        assert!(result.is_ok());
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

        // Should succeed with FakePasswd backend
        assert!(result.is_ok());
    }
}
