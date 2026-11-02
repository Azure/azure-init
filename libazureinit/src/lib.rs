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

/// Run a command, capturing its output.
///
/// Command execution errors and exit statuses are logged at debug level.
/// Execution errors and unsuccessful exit statuses are returned to the caller.
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
        tracing::debug!(
            program = %program,
            %error,
            "Failed to execute system program"
        );
    })?;
    let status = output.status;
    tracing::debug!(?status, "System program completed");

    if !status.success() {
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

    fn expect_io_error(error: error::Error) -> std::io::Error {
        match error {
            error::Error::Io(error) => error,
            error => panic!("Expected an I/O error, got {error:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "Expected an I/O error, got Timeout")]
    fn test_expect_io_error_rejects_non_io_error() {
        expect_io_error(error::Error::Timeout);
    }

    #[test]
    fn test_run_success() {
        let cmd = Command::new("true");
        let (result, logs) = run_with_logs(cmd, Level::DEBUG);
        assert!(result.is_ok());
        assert!(!logs.contains("WARN"), "{logs}");
        assert!(!logs.contains("ERROR"), "{logs}");
        assert!(!logs.contains("failed"), "{logs}");
    }

    #[test]
    fn test_run_failure() {
        for level in [Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR] {
            let cmd = Command::new("false");
            let (result, logs) = run_with_logs(cmd, level);
            let err = result.unwrap_err();
            assert!(
                matches!(
                    err,
                    error::Error::SubprocessFailed { ref command, status }
                        if command.contains("false") && status.code() == Some(1)
                ),
                "{err:?}"
            );
            assert!(!logs.contains("Command 'false' failed"), "{logs}");

            if level != Level::DEBUG {
                assert!(logs.is_empty(), "{level}:\n{logs}");
                continue;
            }

            let diagnostics = logs
                .lines()
                .filter(|line| line.contains("System program completed"))
                .collect::<Vec<_>>();
            assert_eq!(diagnostics.len(), 1, "{logs}");
            assert!(diagnostics[0].contains("DEBUG"), "{logs}");
            assert!(logs.contains("command=\"false\""), "{logs}");
        }
    }

    #[test]
    fn test_run_failure_does_not_log_output() {
        let script = NamedTempFile::new().unwrap();
        fs::write(
            script.path(),
            "printf 'subprocess stdout'; printf 'subprocess stderr' >&2; exit 7",
        )
        .unwrap();

        for level in [Level::TRACE, Level::DEBUG] {
            let mut command = Command::new("sh");
            command.arg(script.path());
            let (result, logs) = run_with_logs(command, level);
            let err = result.unwrap_err();
            assert!(
                matches!(
                    err,
                    error::Error::SubprocessFailed { status, .. }
                        if status.code() == Some(7)
                ),
                "{err:?}"
            );
            assert!(logs.contains("System program completed"), "{logs}");
            assert!(!logs.contains("subprocess stdout"), "{logs}");
            assert!(!logs.contains("subprocess stderr"), "{logs}");
        }
    }

    #[test]
    fn test_run_launch_errors_preserve_context_and_io_error() {
        for level in [Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR] {
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
                let io_error = expect_io_error(result.unwrap_err());
                assert_eq!(io_error.kind(), kind);
                assert_eq!(
                    io_error.raw_os_error(),
                    expected_error.raw_os_error()
                );
                assert_eq!(io_error.to_string(), expected_error.to_string());

                if level != Level::DEBUG {
                    assert!(logs.is_empty(), "{level} {kind:?}:\n{logs}");
                    continue;
                }

                let diagnostics = logs
                    .lines()
                    .filter(|line| {
                        line.contains("Failed to execute system program")
                    })
                    .collect::<Vec<_>>();
                assert_eq!(diagnostics.len(), 1, "{kind:?}:\n{logs}");
                let event = diagnostics[0];
                assert!(event.contains("DEBUG"), "{event}");
                assert!(
                    event.contains(&format!("program={}", program.display())),
                    "{event}"
                );
                assert!(
                    event.contains(&format!("error={expected_error}")),
                    "{event}"
                );
                assert!(event.contains("subprocess"), "{event}");
            }
        }
    }
}
