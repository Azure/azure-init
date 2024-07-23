use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;

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
