use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;

use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use tempfile::tempdir;

// Assert help text includes the --groups flag
#[test]
fn help_groups() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    command.arg("--help");
    command
        .assert()
        .success()
        .stdout(predicate::str::contains("-g, --groups <GROUPS>"));

    Ok(())
}

// Ensure no password-related flags are exposed by the CLI
#[test]
fn help_has_no_password_flags() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    command.arg("--help");
    command
        .assert()
        .success()
        .stdout(predicate::str::is_match("(?i)password").unwrap().not());

    Ok(())
}

// Assert that the --version flag works and outputs the version
#[test]
fn version_flag() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    command.arg("--version");
    command
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));

    Ok(())
}

// Assert that the -V flag works and outputs the version
#[test]
fn version_flag_short() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    command.arg("-V");
    command
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));

    Ok(())
}

// Helper function to set up the log and provision files for cleaning
fn setup_clean_test() -> Result<
    (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ),
    Box<dyn std::error::Error>,
> {
    let temp_dir = tempdir()?;
    let data_dir = temp_dir.path().join("data");
    let log_file = temp_dir.path().join("azure-init.log");
    fs::create_dir_all(&data_dir)?;

    let provisioned_file = data_dir.join("vm-id.provisioned");
    File::create(provisioned_file)?;

    let mut log = File::create(&log_file)?;
    writeln!(log, "fake log line")?;

    let config_contents = format!(
        r#"
        [azure_init_data_dir]
        path = "{}"

        [azure_init_log_path]
        path = "{}"

        [telemetry]
        kvp_diagnostics = false
        "#,
        data_dir.display(),
        log_file.display()
    );
    let config_path = temp_dir.path().join("azure-init-config.toml");
    fs::write(&config_path, config_contents)?;

    Ok((temp_dir, data_dir, log_file, config_path))
}

// Ensures that the `clean` command removes only the provisioned file
#[test]
fn clean_removes_only_provision_files_without_log_arg(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_temp_dir, _data_dir, log_file, config_path) = setup_clean_test()?;
    let provisioned_file = _data_dir.join("vm-id.provisioned");

    assert!(
        provisioned_file.exists(),
        ".provisioned file should exist before cleaning"
    );
    assert!(log_file.exists(), "log file should exist before cleaning");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    cmd.args(["--config", config_path.to_str().unwrap(), "clean"]);

    cmd.assert().success();

    assert!(
        !provisioned_file.exists(),
        "Expected .provisioned file to be deleted"
    );
    assert!(log_file.exists(), "log file should exist after cleaning");

    Ok(())
}

// Ensures that the `clean` command with the --logs arg
// removes both the provisioned file and the log file
#[test]
fn clean_removes_provision_and_log_files_with_log_arg(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_temp_dir, _data_dir, log_file, config_path) = setup_clean_test()?;
    let provisioned_file = _data_dir.join("vm-id.provisioned");

    assert!(
        provisioned_file.exists(),
        ".provisioned file should exist before cleaning"
    );
    assert!(log_file.exists(), "log file should exist before cleaning");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    cmd.args(["--config", config_path.to_str().unwrap(), "clean", "--logs"]);

    cmd.assert().success();

    assert!(
        !provisioned_file.exists(),
        "Expected .provisioned file to be deleted"
    );
    assert!(
        !log_file.exists(),
        "Expected azure-init.log file to be deleted"
    );

    Ok(())
}

#[test]
fn clean_succeeds_without_provisioning_state(
) -> Result<(), Box<dyn std::error::Error>> {
    for missing_directory in [false, true] {
        for logs in [false, true] {
            let (_temp_dir, data_dir, log_file, config_path) =
                setup_clean_test()?;
            fs::remove_file(data_dir.join("vm-id.provisioned"))?;
            if missing_directory {
                fs::remove_dir(&data_dir)?;
            }

            let mut cmd =
                Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
            cmd.env("AZURE_INIT_LOG", "info")
                .env_remove("AZURE_INIT_KVP_FILTER")
                .args(["--config", config_path.to_str().unwrap(), "clean"]);
            if logs {
                cmd.arg("--logs");
            }

            let assertion = cmd.assert().success();
            let stderr = std::str::from_utf8(&assertion.get_output().stderr)?;
            let expected_message = if missing_directory {
                "No provisioning state directory found; nothing to clean"
            } else {
                "No provisioning marker files (*.provisioned) found in"
            };
            let events = stderr
                .lines()
                .filter(|line| line.contains(expected_message))
                .collect::<Vec<_>>();
            assert_eq!(events.len(), 1, "{stderr}");
            assert!(events[0].contains("INFO"), "{stderr}");
            assert!(events[0].contains(data_dir.to_str().unwrap()), "{stderr}");
            assert!(!stderr.contains("ERROR"), "{stderr}");
            assert_eq!(data_dir.exists(), !missing_directory);
            if !missing_directory {
                assert!(fs::read_dir(&data_dir)?.next().is_none());
            }

            if logs {
                assert!(!log_file.exists());
            } else {
                let log_contents = fs::read_to_string(&log_file)?;
                assert!(log_contents.contains("fake log line"));
                assert!(
                    log_contents.contains(expected_message),
                    "{log_contents}"
                );
                assert!(!log_contents.contains("ERROR"), "{log_contents}");
            }
        }
    }

    Ok(())
}

#[test]
fn clean_reports_data_directory_errors(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_temp_dir, data_dir, log_file, config_path) = setup_clean_test()?;
    fs::remove_file(data_dir.join("vm-id.provisioned"))?;
    fs::remove_dir(&data_dir)?;
    fs::write(&data_dir, "not a directory")?;

    let expected_error = fs::read_dir(&data_dir).unwrap_err();
    assert_eq!(expected_error.kind(), ErrorKind::NotADirectory);
    let expected_error = expected_error.to_string();

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    cmd.env_remove("AZURE_INIT_LOG")
        .env_remove("AZURE_INIT_KVP_FILTER")
        .args(["--config", config_path.to_str().unwrap(), "clean", "--logs"]);

    let assertion = cmd.assert().code(1);
    let stderr = std::str::from_utf8(&assertion.get_output().stderr)?;
    let log_contents = fs::read_to_string(&log_file)?;
    for contents in [stderr, log_contents.as_str()] {
        let errors = contents
            .lines()
            .filter(|line| line.contains("ERROR"))
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{contents}");
        let error = errors[0];
        assert!(
            error.contains("Failed to read provisioning state directory"),
            "{error}"
        );
        assert!(error.contains(data_dir.to_str().unwrap()), "{error}");
        assert!(error.contains(&expected_error), "{error}");
    }

    assert!(log_contents.contains("fake log line"));
    assert_eq!(fs::read_to_string(&data_dir)?, "not a directory");

    Ok(())
}
