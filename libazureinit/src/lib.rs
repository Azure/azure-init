// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// The version of the libazureinit package
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
/// Command execution errors and unsuccessful exit statuses are logged at
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
    let span = tracing::info_span!("subprocess", program = %program);
    let _entered = span.enter();

    tracing::debug!(?command, "About to execute system program");
    let output = command.output().inspect_err(|error| {
        tracing::error!(
            program = %program,
            %error,
            "Failed to execute system program"
        );
    })?;
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

#[cfg(test)]
mod lib_tests {
    use super::*;
    use std::{fs, io::ErrorKind, process::Command};
    use tempfile::{tempdir, NamedTempFile};
    use tracing::Level;

    fn run_with_logs(
        command: Command,
        level: Level,
    ) -> (Result<(), error::Error>, String) {
        let log_file = NamedTempFile::new().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(level)
            .with_ansi(false)
            .with_writer(log_file.reopen().unwrap())
            .finish();
        let result =
            tracing::subscriber::with_default(subscriber, || run(command));
        let logs = fs::read_to_string(log_file.path()).unwrap();
        (result, logs)
    }

    #[test]
    fn test_run_success() {
        let cmd = Command::new("true");
        let (result, logs) = run_with_logs(cmd, Level::INFO);
        assert!(result.is_ok());
        assert!(!logs.contains("ERROR"), "{logs}");
    }

    #[test]
    fn test_run_failure() {
        let cmd = Command::new("false");
        let (result, logs) = run_with_logs(cmd, Level::INFO);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, error::Error::SubprocessFailed { .. }));

        let errors = logs
            .lines()
            .filter(|line| line.contains("ERROR"))
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{logs}");
        assert!(errors[0].contains("Command 'false' failed"), "{logs}");
    }

    #[test]
    fn test_run_launch_errors_preserve_context_and_io_error() {
        for level in [Level::INFO, Level::ERROR] {
            for kind in [ErrorKind::NotFound, ErrorKind::PermissionDenied] {
                let dir = tempdir().unwrap();
                let program = dir.path().join("program");
                if kind == ErrorKind::PermissionDenied {
                    fs::write(&program, "not executable").unwrap();
                }

                let expected_error =
                    Command::new(&program).output().unwrap_err();
                assert_eq!(expected_error.kind(), kind);

                let (result, logs) =
                    run_with_logs(Command::new(&program), level);
                let io_error = match result.unwrap_err() {
                    error::Error::Io(error) => error,
                    error => panic!("Expected an I/O error, got {error:?}"),
                };
                assert_eq!(io_error.kind(), kind);
                assert_eq!(
                    io_error.raw_os_error(),
                    expected_error.raw_os_error()
                );
                assert_eq!(io_error.to_string(), expected_error.to_string());

                let errors = logs
                    .lines()
                    .filter(|line| line.contains("ERROR"))
                    .collect::<Vec<_>>();
                assert_eq!(errors.len(), 1, "{level} {kind:?}:\n{logs}");
                let event = errors[0];
                assert!(
                    event.contains("Failed to execute system program"),
                    "{event}"
                );
                assert!(
                    event.contains(&format!("program={}", program.display())),
                    "{event}"
                );
                assert!(
                    event.contains(&format!("error={expected_error}")),
                    "{event}"
                );
                if level == Level::INFO {
                    assert!(event.contains("subprocess"), "{event}");
                }
            }
        }
    }
}
