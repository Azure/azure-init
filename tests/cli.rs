use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;

use std::fs::{self, File};
use std::io::Write;
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

    // Create both .provisioned and .failed files
    let provisioned_file = data_dir.join("vm-id.provisioned");
    File::create(provisioned_file)?;

    let failed_file = data_dir.join("vm-id.failed");
    fs::write(&failed_file, "result=error|reason=test")?;

    let mut log = File::create(&log_file)?;
    writeln!(log, "fake log line")?;

    let config_contents = format!(
        r#"
        [azure_init_data_dir]
        path = "{}"

        [azure_init_log_path]
        path = "{}"
        "#,
        data_dir.display(),
        log_file.display()
    );
    let config_path = temp_dir.path().join("azure-init-config.toml");
    fs::write(&config_path, config_contents)?;

    Ok((temp_dir, data_dir, log_file, config_path))
}

// Ensures that the `clean` command removes both .provisioned and .failed files
#[test]
fn clean_removes_only_provision_files_without_log_arg(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_temp_dir, _data_dir, log_file, config_path) = setup_clean_test()?;
    let provisioned_file = _data_dir.join("vm-id.provisioned");
    let failed_file = _data_dir.join("vm-id.failed");

    assert!(
        provisioned_file.exists(),
        ".provisioned file should exist before cleaning"
    );
    assert!(
        failed_file.exists(),
        ".failed file should exist before cleaning"
    );
    assert!(log_file.exists(), "log file should exist before cleaning");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    cmd.args(["--config", config_path.to_str().unwrap(), "clean"]);

    cmd.assert().success();

    assert!(
        !provisioned_file.exists(),
        "Expected .provisioned file to be deleted"
    );
    assert!(!failed_file.exists(), "Expected .failed file to be deleted");
    assert!(log_file.exists(), "log file should exist after cleaning");

    Ok(())
}

// Ensures that the `clean` command with the --logs arg
// removes both .provisioned, .failed, and log files
#[test]
fn clean_removes_provision_and_log_files_with_log_arg(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_temp_dir, _data_dir, log_file, config_path) = setup_clean_test()?;
    let provisioned_file = _data_dir.join("vm-id.provisioned");
    let failed_file = _data_dir.join("vm-id.failed");

    assert!(
        provisioned_file.exists(),
        ".provisioned file should exist before cleaning"
    );
    assert!(
        failed_file.exists(),
        ".failed file should exist before cleaning"
    );
    assert!(log_file.exists(), "log file should exist before cleaning");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    cmd.args(["--config", config_path.to_str().unwrap(), "clean", "--logs"]);

    cmd.assert().success();

    assert!(
        !provisioned_file.exists(),
        "Expected .provisioned file to be deleted"
    );
    assert!(!failed_file.exists(), "Expected .failed file to be deleted");
    assert!(
        !log_file.exists(),
        "Expected azure-init.log file to be deleted"
    );

    Ok(())
}

// Assert report command exists in help
#[test]
fn help_shows_report_command() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::cargo_bin("azure-init")?;
    command.arg("--help");
    command
        .assert()
        .success()
        .stdout(predicate::str::contains("report"))
        .stdout(predicate::str::contains(
            "Report provisioning status to Azure",
        ));

    Ok(())
}

// Assert report subcommands exist
#[test]
fn report_help_shows_subcommands() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::cargo_bin("azure-init")?;
    command.args(["report", "--help"]);
    command
        .assert()
        .success()
        .stdout(predicate::str::contains("auto"))
        .stdout(predicate::str::contains("ready"))
        .stdout(predicate::str::contains("failure"));

    Ok(())
}

// Test that report auto fails gracefully when no state files exist
#[test]
fn report_auto_fails_without_state_files(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let data_dir = temp_dir.path().join("data");
    fs::create_dir_all(&data_dir)?;

    let config_contents = format!(
        r#"
        [azure_init_data_dir]
        path = "{}"
        "#,
        data_dir.display()
    );
    let config_path = temp_dir.path().join("azure-init-config.toml");
    fs::write(&config_path, config_contents)?;

    let mut cmd = Command::cargo_bin("azure-init")?;
    cmd.args(["--config", config_path.to_str().unwrap(), "report", "auto"]);

    cmd.assert().failure();

    Ok(())
}

// Test that report ready fails gracefully when no .provisioned file exists
#[test]
fn report_ready_fails_without_provisioned_file(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let data_dir = temp_dir.path().join("data");
    fs::create_dir_all(&data_dir)?;

    let config_contents = format!(
        r#"
        [azure_init_data_dir]
        path = "{}"
        "#,
        data_dir.display()
    );
    let config_path = temp_dir.path().join("azure-init-config.toml");
    fs::write(&config_path, config_contents)?;

    let mut cmd = Command::cargo_bin("azure-init")?;
    cmd.args(["--config", config_path.to_str().unwrap(), "report", "ready"]);

    cmd.assert().failure();

    Ok(())
}

// Test that report failure fails gracefully when no .failed file exists
#[test]
fn report_failure_fails_without_failed_file(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let data_dir = temp_dir.path().join("data");
    fs::create_dir_all(&data_dir)?;

    let config_contents = format!(
        r#"
        [azure_init_data_dir]
        path = "{}"
        "#,
        data_dir.display()
    );
    let config_path = temp_dir.path().join("azure-init-config.toml");
    fs::write(&config_path, config_contents)?;

    let mut cmd = Command::cargo_bin("azure-init")?;
    cmd.args([
        "--config",
        config_path.to_str().unwrap(),
        "report",
        "failure",
    ]);

    cmd.assert().failure();

    Ok(())
}
