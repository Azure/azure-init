// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::ExitCode;
use std::time::Duration;

use libazureinit::imds::{Compute, OsProfile};
use predicates::prelude::*;
use tempfile::TempDir;

const CASE_ENV: &str = "AZURE_INIT_TEST_STARTUP_CASE";

fn restore_stderr_on_panic() {
    // Only called in a child test process. Keep the original descriptor alive
    // so the panic hook can restore stderr before main reports the join error.
    let fd = unsafe { libc::dup(libc::STDERR_FILENO) };
    assert!(fd >= 0, "{}", std::io::Error::last_os_error());
    let stderr = unsafe { OwnedFd::from_raw_fd(fd) };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let restored =
            unsafe { libc::dup2(stderr.as_raw_fd(), libc::STDERR_FILENO) };
        assert!(restored >= 0, "{}", std::io::Error::last_os_error());
        previous(info);
    }));
    let full = OpenOptions::new().write(true).open("/dev/full").unwrap();
    let redirected =
        unsafe { libc::dup2(full.as_raw_fd(), libc::STDERR_FILENO) };
    assert!(redirected >= 0, "{}", std::io::Error::last_os_error());
}

#[test]
fn clean_startup_failures() {
    if let Ok(case) = std::env::var(CASE_ENV) {
        match case.as_str() {
            "worker-panic" => restore_stderr_on_panic(),
            "subscriber-conflict" => {
                tracing::subscriber::set_global_default(
                    tracing_subscriber::registry(),
                )
                .unwrap();
            }
            _ => panic!("unexpected startup test case: {case}"),
        }
        assert_eq!(
            format!("{:?}", super::main()),
            format!("{:?}", ExitCode::FAILURE)
        );
        return;
    }

    for (case, expected) in [
        ("worker-panic", "Failed to initialize logging:"),
        (
            "subscriber-conflict",
            "Failed to set global default subscriber:",
        ),
    ] {
        let dir = TempDir::new().unwrap();
        let log_path = if case == "worker-panic" {
            dir.path().to_path_buf()
        } else {
            dir.path().join("azure-init.log")
        };
        let data_dir = dir.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();
        let marker = data_dir.join("untouched.provisioned");
        File::create(&marker).unwrap();
        let config = dir.path().join("azure-init.toml");
        std::fs::write(
            &config,
            format!(
                "[telemetry]\nkvp_diagnostics = false\n\
                 [azure_init_log_path]\npath = {log_path:?}\n\
                 [azure_init_data_dir]\npath = {data_dir:?}\n",
            ),
        )
        .unwrap();

        // "clean" is both a libtest name filter and a valid application
        // command, allowing the unmodified Cli::parse() to run in this child.
        assert_cmd::Command::new(std::env::current_exe().unwrap())
            .arg("clean")
            .env(CASE_ENV, case)
            .env("AZURE_INIT_CONFIG", &config)
            .env("AZURE_INIT_LOG", "off")
            .env("RUST_TEST_NOCAPTURE", "1")
            .env("RUST_TEST_THREADS", "1")
            .timeout(Duration::from_secs(10))
            .assert()
            .success()
            .stderr(predicate::str::contains(expected));
        assert!(marker.exists(), "cleanup ran after initialization failed");
    }
}

#[test]
fn username_prefers_imds_then_ovf_and_reports_missing_sources() {
    let metadata = InstanceMetadata {
        compute: Compute {
            os_profile: OsProfile {
                admin_username: "imds-user".to_owned(),
                computer_name: "test-host".to_owned(),
                disable_password_authentication: true,
            },
            public_keys: Vec::new(),
        },
    };
    let mut ovf = Environment::default();
    ovf.provisioning_section.linux_prov_conf_set.username =
        "ovf-user".to_owned();
    assert_eq!(
        get_username(Some(&metadata), Some(&ovf)).unwrap(),
        "imds-user"
    );
    assert_eq!(get_username(Some(&metadata), None).unwrap(), "imds-user");
    assert_eq!(get_username(None, Some(&ovf)).unwrap(), "ovf-user");
    let error = get_username(None, None).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<LibError>(),
        Some(LibError::UsernameFailure)
    ));
}

#[test]
fn is_config_error_flags_user_configuration_errors() {
    assert!(is_config_error(&anyhow::Error::from(
        LibError::UserMissing {
            user: "missing".to_owned(),
        }
    )));
    assert!(is_config_error(&anyhow::Error::from(
        LibError::NonEmptyPassword
    )));
    assert!(!is_config_error(&anyhow::Error::from(LibError::Timeout)));
}

#[test]
fn exit_code_for_distinguishes_config_errors_from_failures() {
    let config: u8 = exitcode::CONFIG.try_into().unwrap();
    // ExitCode has no PartialEq, so compare Debug of equivalently-built codes.
    assert_eq!(
        format!(
            "{:?}",
            exit_code_for(&anyhow::Error::from(LibError::NonEmptyPassword))
        ),
        format!("{:?}", ExitCode::from(config))
    );
    assert_eq!(
        format!(
            "{:?}",
            exit_code_for(&anyhow::Error::from(LibError::Timeout))
        ),
        format!("{:?}", ExitCode::FAILURE)
    );
}
