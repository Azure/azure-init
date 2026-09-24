// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// The version of the libazureinit package
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod config;
pub use config::{HostnameProvisioner, PasswordProvisioner, UserProvisioner};
pub mod error;
pub(crate) mod http;
pub mod imds;
pub mod media;
pub mod wireserver;

mod provision;
pub use provision::{
    password::{lock_user, set_user_password},
    user::User,
    Provision,
};
mod status;
pub use status::{
    get_vm_id, is_provisioning_complete, mark_provisioning_complete,
};

#[cfg(test)]
mod unittest;

// Re-export as the Client is used in our API.
pub use reqwest;

/// Run a command, capturing its output and logging it if it fails.
///
/// Launch failures and unsuccessful exit statuses are logged at error level.
///
/// <div class="warning">
///
/// This logs the command and its arguments, and as such is not appropriate
/// if the command contains sensitive information.
///
/// </div>
#[tracing::instrument(
    name = "subprocess",
    skip_all,
    err,
    fields(program = %command.get_program().to_string_lossy())
)]
pub(crate) fn run(
    mut command: std::process::Command,
) -> Result<(), error::Error> {
    tracing::debug!(?command, "About to execute system program");
    let output = command.output()?;
    let status = output.status;
    tracing::debug!(?status, "System program completed");

    if !status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        tracing::debug!(
            ?status,
            ?command,
            ?stdout,
            ?stderr,
            "Failed command output"
        );
        return Err(error::Error::SubprocessFailed {
            command: format!("{command:?}"),
            status,
        });
    }

    Ok(())
}

#[cfg(test)]
mod lib_tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn test_run_success() {
        let cmd = Command::new("true");
        assert!(run(cmd).is_ok());
    }

    #[test]
    fn test_run_failure() {
        let cmd = Command::new("false");
        let result = run(cmd);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, error::Error::SubprocessFailed { .. }));
    }
}
