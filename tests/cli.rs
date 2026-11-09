use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use rstest::rstest;

use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::time::timeout;

#[rstest]
#[case::delivered("200 OK", false)]
#[case::delivery_failed("403 Forbidden", true)]
#[tokio::test]
async fn invalid_configuration_reports_and_surfaces_http_failures(
    #[case] status: &str,
    #[case] delivery_failed: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let config_path = dir.path().join("invalid.toml");
    let log_path = dir.path().join("azure-init.log");
    let data_dir = dir.path().join("data");
    fs::write(
        &config_path,
        format!(
            r#"
        [ssh]
        query_sshd_config = "invalid"
        [azure_init_log_path]
        path = "{}"
        [azure_init_data_dir]
        path = "{}"
        [telemetry]
        kvp_diagnostics = false
        "#,
            log_path.display(),
            data_dir.display()
        ),
    )?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy = format!("http://{}", listener.local_addr()?);
    let mut command = tokio::process::Command::new(
        assert_cmd::cargo::cargo_bin!("azure-init"),
    );
    command
        .arg("--config")
        .arg(&config_path)
        .env("HTTP_PROXY", &proxy)
        .env("http_proxy", &proxy)
        .env("ALL_PROXY", &proxy)
        .env("all_proxy", &proxy)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .kill_on_drop(true);

    let capture = async {
        let (mut stream, _) = listener.accept().await?;
        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await?;
        assert_eq!(
            request_line.trim_end(),
            "POST http://168.63.129.16/provisioning/health HTTP/1.1"
        );
        let mut content_length = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await? == 0 {
                return Err(std::io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "incomplete HTTP headers",
                ));
            }
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = Some(
                        value.trim().parse::<usize>().map_err(|error| {
                            std::io::Error::new(ErrorKind::InvalidData, error)
                        })?,
                    );
                }
            }
        }
        let length = content_length.ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::InvalidData,
                "missing content length",
            )
        })?;
        let mut body = vec![0; length];
        reader.read_exact(&mut body).await?;
        let body: serde_json::Value =
            serde_json::from_slice(&body).map_err(|error| {
                std::io::Error::new(ErrorKind::InvalidData, error)
            })?;
        drop(reader);
        stream.write_all(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes(),
        ).await?;
        Ok::<_, std::io::Error>(body)
    };
    let (body, output) = timeout(Duration::from_secs(5), async {
        tokio::try_join!(capture, command.output())
    })
    .await??;

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("Failed to load configuration:"), "{stderr}");
    assert_eq!(
        stderr.contains("Failed to send provisioning failure report:"),
        delivery_failed,
        "{stderr}"
    );
    assert_eq!(body["state"], "NotReady");
    assert_eq!(body["details"]["subStatus"], "ProvisioningFailed");
    let report: libazureinit_kvp::ProvisioningReport =
        body["details"]["description"].as_str().unwrap().parse()?;
    let report = serde_json::to_value(report)?;
    assert_eq!(report["result"], "error");
    assert_eq!(report["reason"], "failed to load sshd config");
    assert_eq!(
        report["agent"],
        format!("Azure-Init/{}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(report["extra"][0][0], "details");
    assert!(report["extra"][0][1]
        .as_str()
        .unwrap()
        .contains("query_sshd_config"));
    assert!(!log_path.exists());
    assert!(!data_dir.exists());
    Ok(())
}

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

#[rstest]
#[case::clean(true)]
#[case::already_provisioned(false)]
fn non_provisioning_paths_do_not_send_health(
    #[case] clean: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let (_dir, data_dir, _log_file, config_path) = setup_clean_test()?;
    let vm_id = libazureinit::get_vm_id()
        .unwrap_or_else(|| "00000000-0000-0000-0000-000000000000".into());
    let marker = data_dir.join(format!("{vm_id}.provisioned"));
    fs::write(&marker, "already provisioned")?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    writeln!(
        fs::OpenOptions::new().append(true).open(&config_path)?,
        "\n[wireserver]\nhealth_endpoint = \"http://{}/provisioning/health\"\n\
         connection_timeout_secs = 0.1\nread_timeout_secs = 0.1\n\
         total_retry_timeout_secs = 0.5",
        listener.local_addr()?
    )?;
    let mut command =
        assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("azure-init"));
    command
        .arg("--config")
        .arg(&config_path)
        .env("NO_PROXY", "*")
        .env("no_proxy", "*")
        .timeout(Duration::from_secs(5));
    if clean {
        command.arg("clean");
    }
    command.assert().success();
    assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    assert_eq!(marker.exists(), !clean);
    Ok(())
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
