// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use libazureinit::config::Config;
use libazureinit::imds::PublicKeys;
use libazureinit::User;
use libazureinit::{
    health::report_ready,
    imds,
    reqwest::{header, Client},
    Provision,
};
use std::env;

#[tokio::main]
async fn main() {
    let config = Config::default();

    let cli_args: Vec<String> = env::args().collect();
    let mut default_headers = header::HeaderMap::new();
    let user_agent = header::HeaderValue::from_str("azure-init").unwrap();
    default_headers.insert(header::USER_AGENT, user_agent);
    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs_f64(
            config.imds.connection_timeout_secs,
        ))
        .default_headers(default_headers)
        .build()
        .unwrap();

    println!();
    println!("**********************************");
    println!("* Beginning functional testing");
    println!("**********************************");
    println!();

    println!("Reporting VM Health to wireserver");
    let vm_id = "00000000-0000-0000-0000-000000000000";
    match report_ready(&config, vm_id, None).await {
        Ok(()) => println!("VM Health successfully reported"),
        Err(err) => {
            println!("Failed to report health: {err:?}");
            return;
        }
    }

    // Simplified version of calling imds::query. Since username is directly
    // given by cli_args below, it is not needed to get instance metadata like
    // how it is done in provision() in main.
    let _ = imds::query(&client, Some(&config), None)
        .await
        .expect("Failed to query IMDS");

    let username = &cli_args[1];

    let keys: Vec<PublicKeys> = vec![
        PublicKeys {
            path: "/path/to/.ssh/keys/".to_owned(),
            key_data: "ssh-rsa test_key_1".to_owned(),
        },
        PublicKeys {
            path: "/path/to/.ssh/keys/".to_owned(),
            key_data: "ssh-rsa test_key_2".to_owned(),
        },
        PublicKeys {
            path: "/path/to/.ssh/keys/".to_owned(),
            key_data: "ssh-rsa test_key_3".to_owned(),
        },
    ];

    Provision::new(
        "my-hostname".to_string(),
        User::new(username, keys),
        config,
        false,
    )
    .provision()
    .expect("Failed to provision host");

    println!("VM successfully provisioned");
    println!();

    println!("**********************************");
    println!("* Functional testing completed successfully!");
    println!("**********************************");
    println!();
}

#[test]
#[ignore = "requires Docker and the azure-init-main-tests:local image"]
fn agent_provisioning_in_container() {
    use libazureinit_kvp::{
        Diagnostic, DiagnosticReader, Entry, KvpPool, KvpPoolStore, Outcome,
        PoolMode, PROVISIONING_REPORT_KEY,
    };
    use std::path::Path;
    use std::time::Duration;

    let artifacts = tempfile::TempDir::new().unwrap();
    let runner = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/support/container_provisioning.py");
    let agent = match option_env!("CARGO_BIN_EXE_azure-init") {
        Some(agent) => std::path::PathBuf::from(agent),
        // Cargo also builds this file as a binary test target under --all-targets.
        None => std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("azure-init"),
    };
    assert!(
        agent.is_file(),
        "agent binary is missing: {}",
        agent.display()
    );
    assert_cmd::Command::new("python3")
        .arg(runner)
        .arg("--agent")
        .arg(agent)
        .arg("--artifacts")
        .arg(artifacts.path())
        .timeout(Duration::from_secs(240))
        .assert()
        .success();

    for (scenario, expected, report_prefix) in [
        ("success", Outcome::Success, "result=success|"),
        ("failure", Outcome::Fail, "result=error|"),
    ] {
        let directory = artifacts.path().join(scenario);
        let store =
            KvpPoolStore::new_in(KvpPool::Guest, &directory, PoolMode::Safe)
                .unwrap();
        let entries = DiagnosticReader::new(store.clone()).entries().unwrap();
        let report = store.read(PROVISIONING_REPORT_KEY).unwrap().unwrap();
        assert!(
            report.starts_with(report_prefix),
            "unexpected {scenario} report: {report}"
        );
        assert!(
            entries.iter().any(|entry| matches!(
                entry,
                Entry::Diagnostic(Diagnostic::Finish(finish))
                    if finish.key.name == "provision" && finish.result == expected
            )),
            "missing provision finish with {expected} in {scenario}"
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry, Entry::Report(_)))
                .count(),
            1,
            "expected one parsed provisioning report in {scenario}"
        );
        assert!(entries.iter().all(|entry| matches!(
            entry,
            Entry::Diagnostic(_) | Entry::Report(_)
        )));
    }
}
