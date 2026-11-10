// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use libazureinit_kvp::{
    KvpPool, KvpPoolStore, PoolMode, ProvisioningReport,
    PROVISIONING_REPORT_KEY,
};
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
        .arg("--artifacts")
        .arg(artifacts.path())
        .timeout(std::time::Duration::from_secs(240))
        .assert()
        .success();

    for (scenario, result) in [
        ("success", Some("success")),
        ("failure", Some("error")),
        ("config-error", Some("error")),
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
