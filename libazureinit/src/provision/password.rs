// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//!
//! Password provisioning behavior for `libazureinit`.
//!
//! This module provides both low-level building block functions and higher-level
//! provisioning interfaces for managing user passwords and account locking.
//!
//! ## Building Block Functions
//!
//! - [`set_user_password`] - Sets a password for a user using `chpasswd`. The password
//!   is passed securely via stdin to avoid exposing secrets in process arguments or logs.
//! - [`lock_user`] - Locks a user account using `passwd -l`. The path to `passwd` is
//!   provided at build time via the `PATH_PASSWD` environment variable.
//!
//! These functions are decoupled and perform only their specific task - they do not
//! modify SSH configuration or perform other side effects.
//!
//! ## Higher-Level Provisioning Interface
//!
//! The [`PasswordProvisioner`] provides the traditional provisioning interface that
//! works with [`User`] structs:
//! - If `User.password` is present, it calls [`set_user_password`]
//! - If `User.password` is absent, it calls [`lock_user`]
//!
//! ## Usage Examples
//!
//! ### Direct API Usage (Building Blocks)
//! ```ignore
//! use libazureinit::{set_user_password, lock_user};
//!
//! // Set a password for a specific user
//! set_user_password("azureuser", "s3cr3t")?;
//!
//! // Lock a user account  
//! lock_user("azureuser")?;
//! ```
//!
//! ### Traditional Provisioning Interface
//! ```ignore
//! use libazureinit::{Provision, User};
//! use libazureinit::config::Config;
//!
//! let user = User::new("azureuser", vec![]).with_password("s3cr3t");
//! let config = Config::default();
//! let _ = Provision::new("host", user, config, true).provision();
//! ```
//!
//! ## Notes
//!
//! - The reference binary `azure-init` does not set passwords; it constructs a
//!   `User` without calling `with_password`, which results in account locking.
//! - External consumers can use either the building block functions for fine-grained
//!   control or the traditional provisioning interface for convenience.
//! - SSH configuration is handled separately and is not modified by these functions.

use std::io::Write;
use std::process::{Command, Stdio};

use tracing::instrument;
use zeroize::Zeroize;

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

/// Set a password for the specified user.
///
/// This function only sets the password using `chpasswd` - it does not
/// modify SSH configuration or perform any other actions.
///
/// # Arguments
/// * `user` - The username to set the password for
/// * `password` - The password to set
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(Error)` if the password setting fails
///
/// # Security
/// The password is passed securely to `chpasswd` via stdin to avoid
/// exposing secrets in process arguments or logs. The password is also
/// securely cleared from memory after use using zeroization.
#[instrument(skip_all)]
pub fn set_user_password(user: &str, password: &str) -> Result<(), Error> {
    // Basic input validation
    if user.is_empty() {
        return Err(Error::UnhandledError {
            details: "Username cannot be empty".to_string(),
        });
    }
    if password.is_empty() {
        return Err(Error::UnhandledError {
            details: "Password cannot be empty".to_string(),
        });
    }

    let mut input = format!("{user}:{password}");
    let mut child = Command::new("chpasswd")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
        // Close stdin to signal EOF to chpasswd
        drop(stdin);
    }

    input.zeroize();

    let status = child.wait()?;
    if !status.success() {
        tracing::error!(username = %user, ?status, "chpasswd failed to set password");
        return Err(Error::SubprocessFailed {
            command: "chpasswd".to_string(),
            status,
        });
    }
    tracing::info!(target: "libazureinit::password::status", username = %user, "Successfully set password via chpasswd");
    Ok(())
}

/// Lock the specified user account.
///
/// This function only locks the user account using `passwd -l` - it does not
/// modify SSH configuration or perform any other actions.
///
/// # Arguments  
/// * `user` - The username to lock
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(Error)` if the account locking fails
#[instrument(skip_all)]
pub fn lock_user(user: &str) -> Result<(), Error> {
    if user.is_empty() {
        return Err(Error::UnhandledError {
            details: "Username cannot be empty".to_string(),
        });
    }

    let path_passwd = env!("PATH_PASSWD");
    let mut command = Command::new(path_passwd);
    command.arg("-l").arg(user);
    crate::run(command).map_err(|e| {
        tracing::error!(username = %user, error = ?e, "Failed to lock account via passwd -l");
        e
    })?;
    tracing::info!(target: "libazureinit::password::status", username = %user, "Locked account via passwd -l");
    Ok(())
}

/// Manages the user's password during provisioning using the building block functions.
///
/// This function supports two modes of operation:
/// - If a password is provided in the `User` object, it calls [`set_user_password`]
/// - If no password is provided, it calls [`lock_user`]
///
/// This function serves as a bridge between the traditional `User`-based provisioning
/// interface and the new decoupled password management functions.
///
/// Reference `azure-init` behavior: it never calls `User::with_password`.
/// Therefore `user.password` is `None`, and this function always calls [`lock_user`]
/// (there is no alternate locking path). Library consumers that want a password
/// must explicitly call `User::with_password`.
/// See `doc/azure_init_behavior.md` for details.
#[instrument(skip_all)]
fn passwd(user: &User) -> Result<(), Error> {
    if let Some(ref password) = user.password {
        set_user_password(&user.name, password)
    } else {
        lock_user(&user.name)
    }
}

#[instrument(skip_all)]
#[cfg(test)]
fn mock_passwd(_user: &User) -> Result<(), Error> {
    // In tests, simulate success for both setting a password and locking
    // (no external command execution).
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

    #[test]
    fn test_set_user_password_function_exists() {
        // Test that the function compiles and has the expected signature
        // In a real environment, this would require mocking chpasswd
        let _fn_ptr: fn(&str, &str) -> Result<(), crate::error::Error> =
            set_user_password;
    }

    #[test]
    fn test_lock_user_function_exists() {
        // Test that the function compiles and has the expected signature
        // In a real environment, this would require mocking passwd
        let _fn_ptr: fn(&str) -> Result<(), crate::error::Error> = lock_user;
    }

    #[test]
    fn test_set_user_password_empty_username() {
        let result = set_user_password("", "password123");
        assert!(result.is_err());
        if let Err(crate::error::Error::UnhandledError { details }) = result {
            assert!(details.contains("Username cannot be empty"));
        } else {
            panic!("Expected UnhandledError for empty username");
        }
    }

    #[test]
    fn test_set_user_password_empty_password() {
        let result = set_user_password("testuser", "");
        assert!(result.is_err());
        if let Err(crate::error::Error::UnhandledError { details }) = result {
            assert!(details.contains("Password cannot be empty"));
        } else {
            panic!("Expected UnhandledError for empty password");
        }
    }

    #[test]
    fn test_lock_user_empty_username() {
        let result = lock_user("");
        assert!(result.is_err());
        if let Err(crate::error::Error::UnhandledError { details }) = result {
            assert!(details.contains("Username cannot be empty"));
        } else {
            panic!("Expected UnhandledError for empty username");
        }
    }
}
