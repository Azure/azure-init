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
/// This logs the command and its arguments, plus stdout and stderr as text on
/// failure, preserving embedded newlines. It should not be used for commands
/// whose arguments or output contain sensitive information.
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
        tracing::error!(
            ?status,
            ?command,
            %stdout,
            %stderr,
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
    use crate::unittest::{capture_kvp_at_info, kvp_error_fields};
    use libazureinit_kvp::{Diagnostic, Outcome};
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

    #[test]
    fn test_run_failure_preserves_context_at_info() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            r#"printf '%s\n' 'stdout "context"' 'second line'; printf '%s\n' 'stderr \ detail' >&2; exit 7"#,
        ]);
        let (result, diagnostics) = capture_kvp_at_info(|| run(command));
        assert!(matches!(
            result,
            Err(error::Error::SubprocessFailed { status, .. })
                if status.code() == Some(7)
        ));
        let [Diagnostic::Start(start), Diagnostic::Event(context), Diagnostic::Event(returned), Diagnostic::Finish(finish)] =
            diagnostics.as_slice()
        else {
            panic!("unexpected subprocess lifecycle: {diagnostics:?}");
        };
        assert_eq!(start.key.event_id, finish.key.event_id);
        assert_ne!(context.key.event_id, returned.key.event_id);
        assert_eq!(finish.result, Outcome::Failure);

        let errors = kvp_error_fields(&diagnostics);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0]["message"], "Failed command output");
        assert_eq!(errors[0]["stdout"], "stdout \"context\"\nsecond line\n");
        assert_eq!(errors[0]["stderr"], "stderr \\ detail\n");
        assert!(errors[1]["error"]
            .as_str()
            .unwrap()
            .contains("exit status: 7"));
    }
}
