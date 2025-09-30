// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
pub mod config;
pub use config::{HostnameProvisioner, PasswordProvisioner, UserProvisioner};
pub mod error;
pub mod health;
pub(crate) mod http;
pub mod imds;
mod kvp;
pub mod logging;
pub mod media;

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
/// In the event of a failure, the provided `error_message` is logged at
/// error level.
///
/// <div class="warning">
///
/// This logs the command and its arguments, and as such is not appropriate
/// if the command contains sensitive information.
///
/// </div>
pub(crate) fn run(
    mut command: std::process::Command,
) -> Result<(), error::Error> {
    let program = command.get_program().to_string_lossy().to_string();
    let span = tracing::info_span!("subprocess", program);
    let _entered = span.enter();

    tracing::debug!(?command, "About to execute system program");
    let output = command.output()?;
    let status = output.status;
    tracing::debug!(?status, "System program completed");

    if !status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        tracing::error!(
            ?status,
            ?command,
            ?stdout,
            ?stderr,
            "Command '{}' failed",
            program
        );
        return Err(error::Error::SubprocessFailed {
            command: format!("{command:?}"),
            status,
        });
    }

    Ok(())
}
