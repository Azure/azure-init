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

#[test]
fn clean_removes_provision_and_log_files(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempdir()?;
    let data_dir = temp_dir.path().join("data");
    let log_file = temp_dir.path().join("azure-init.log");
    fs::create_dir_all(&data_dir)?;

    let provisioned_file = data_dir.join("vm-id.provisioned");
    File::create(&provisioned_file)?;

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
