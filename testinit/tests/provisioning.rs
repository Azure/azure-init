// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use libazureinit::{
    config::Config, error::Error, set_user_password, Provision, User,
};
use libazureinit_kvp::{
    KvpPool, KvpPoolStore, PoolMode, ProvisioningReport,
    PROVISIONING_REPORT_KEY,
};
use rstest::rstest;
use std::fs;
use std::path::Path;

#[test]
#[ignore = "requires Docker and the azure-init-main-tests:local image"]
fn agent_provisioning_in_container() {
    let artifacts = tempfile::tempdir().unwrap();
    let runner = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testinit/testing-server/container_provisioning.py");
    assert_cmd::Command::new("python3")
        .arg("-B")
        .arg(runner)
        .arg("--agent")
        .arg(assert_cmd::cargo::cargo_bin!("azure-init"))
        .arg("--test-binary")
        .arg(std::env::current_exe().unwrap())
        .arg("--artifacts")
        .arg(artifacts.path())
        .timeout(std::time::Duration::from_secs(240))
        .assert()
        .success();

    for (scenario, result) in [
        ("success", Some("success")),
        ("failure", Some("error")),
        ("config-error", Some("error")),
        ("ssh-success", Some("success")),
        ("ssh-key-error", Some("error")),
        ("sshd-error", Some("error")),
        ("sshd-create-write-error", Some("error")),
        ("sshd-append-write-error", Some("error")),
        ("state-permission-error", Some("success")),
        ("marker-error", Some("error")),
        ("ovf-fallback", Some("error")),
        ("ovf-missing", Some("error")),
        ("clean-read-error", None),
        ("clean-marker-error", None),
        ("clean-race", None),
    ] {
        let directory = artifacts.path().join(scenario);
        let store =
            KvpPoolStore::new_in(KvpPool::Guest, &directory, PoolMode::Safe)
                .unwrap();
        let encoded = store.read(PROVISIONING_REPORT_KEY).unwrap();
        let Some(result) = result else {
            assert_eq!(encoded, None, "cleanup must not report provisioning");
            continue;
        };
        let encoded = encoded.unwrap();
        let report: ProvisioningReport = encoded.parse().unwrap();
        let report = serde_json::to_value(report).unwrap();
        assert_eq!(report["result"], result);
        assert_eq!(report["vm_id"], "00000000-0000-0000-0000-000000000000");
        assert_eq!(
            store
                .dump()
                .unwrap()
                .iter()
                .filter(|(key, _)| key == PROVISIONING_REPORT_KEY)
                .count(),
            1
        );
        let http: serde_json::Value = serde_json::from_slice(
            &fs::read(directory.join("http-report.json")).unwrap(),
        )
        .unwrap();
        if result == "success" {
            assert_eq!(http, serde_json::json!({"state": "Ready"}));
        } else {
            assert_eq!(http["details"]["description"], encoded);
        }
    }
}

#[rstest]
#[case::success("synthetic password:with-colon", true)]
#[case::failure("synthetic-rejected-password", false)]
#[ignore = "invoked only inside the isolated testinit container"]
fn isolated_password_provisioning(
    #[case] password: &str,
    #[case] succeeds: bool,
    #[values(false, true)] through_provision: bool,
) {
    assert!(Path::new("/test/isolated-provisioning").is_file());
    assert_eq!(std::env::var("TEST_SCENARIO").unwrap(), "passwords");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .finish();
    let result = tracing::subscriber::with_default(subscriber, || {
        if through_provision {
            let mut config = Config::default();
            config.ssh.configure_sshd_password_authentication = false;
            Provision::new(
                "coverage-host",
                User::new("coverage-user", []).with_password(password),
                config,
                true,
            )
            .provision()
        } else {
            set_user_password("coverage-user", password)
        }
    });
    if succeeds {
        result.unwrap();
    } else if through_provision {
        assert!(matches!(result, Err(Error::NoPasswordProvisioner)));
    } else {
        assert!(matches!(
            result,
            Err(Error::SubprocessFailed { command, status })
                if command == "chpasswd" && status.code() == Some(17)
        ));
    }
}
