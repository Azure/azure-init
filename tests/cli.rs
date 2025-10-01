use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;

use std::fs::{self, File};
use std::io::Write;
use tempfile::tempdir;

// Assert help text includes the --groups flag
#[test]
fn help_groups() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::cargo_bin("azure-init")?;
    command.arg("--help");
    command
        .assert()
        .success()
        .stdout(predicate::str::contains("-g, --groups <GROUPS>"));

    Ok(())
}

// Assert that the --version flag works and outputs the version
#[test]
fn version_flag() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::cargo_bin("azure-init")?;
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
    let mut command = Command::cargo_bin("azure-init")?;
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

    let mut cmd = Command::cargo_bin("azure-init")?;
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

    let mut cmd = Command::cargo_bin("azure-init")?;
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
