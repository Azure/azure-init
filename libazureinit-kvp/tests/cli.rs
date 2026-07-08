// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::io::Write;
use std::process::{Command, Output};

use tempfile::TempDir;

fn kvp(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_libazureinit-kvp"))
        .args(args)
        .output()
        .unwrap()
}

fn kvp_with_stdin(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_libazureinit-kvp"))
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn with_dir<'a>(dir: &'a TempDir, args: &'a [&'a str]) -> Vec<&'a str> {
    let mut all = vec!["--dir", dir.path().to_str().unwrap()];
    all.extend_from_slice(args);
    all
}

fn assert_success(output: Output) -> String {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn help_lists_commands() {
    let stdout = assert_success(kvp(&["--help"]));
    assert!(stdout.contains("Usage: libazureinit-kvp"));
    assert!(stdout.contains("write"));
    assert!(stdout.contains("append-multiple"));
    assert!(stdout.contains("delete-multiple"));
    assert!(stdout.contains("is-stale"));
}

#[test]
fn write_help_documents_append_flag() {
    let stdout = assert_success(kvp(&["write", "--help"]));
    assert!(stdout.contains("--append"));
    assert!(stdout.contains("<KEY>"));
    assert!(stdout.contains("<VALUE>"));
}

#[test]
fn default_info_uses_default_store_constructor() {
    let stdout = assert_success(kvp(&["info"]));
    assert!(stdout.contains("pool=guest"));
    assert!(stdout.contains("path=/var/lib/hyperv/.kvp_pool_1"));
    assert!(stdout.contains("mode=safe"));
}

#[test]
fn info_reports_custom_store_metadata() {
    let dir = TempDir::new().unwrap();
    let args = with_dir(&dir, &["--pool", "external", "--unsafe", "info"]);
    let stdout = assert_success(kvp(&args));

    assert!(stdout.contains("pool=external"));
    assert!(stdout.contains(&format!(
        "path={}",
        dir.path().join(".kvp_pool_0").display()
    )));
    assert!(stdout.contains("mode=unsafe"));
    assert!(stdout.contains("records=0"));
    assert!(stdout.contains("empty=true"));
    assert!(stdout.contains("stale=false"));
    assert!(stdout.contains("max_key_size=512"));
    assert!(stdout.contains("max_value_size=2048"));
}

#[test]
fn info_accepts_equals_style_global_options() {
    let dir = TempDir::new().unwrap();
    let dir_arg = format!("--dir={}", dir.path().display());
    let stdout = assert_success(kvp(&[&dir_arg, "--pool=auto", "info"]));

    assert!(stdout.contains("pool=auto"));
    assert!(stdout.contains(&format!(
        "path={}",
        dir.path().join(".kvp_pool_2").display()
    )));
}

#[test]
fn write_append_read_dump_entries_delete_and_clear() {
    let dir = TempDir::new().unwrap();
    assert_success(kvp(&with_dir(&dir, &["write", "a", "1"])));
    assert_success(kvp(&with_dir(&dir, &["write", "--append", "a", "2"])));

    assert_eq!(assert_success(kvp(&with_dir(&dir, &["read", "a"]))), "2\n");
    assert_eq!(
        assert_success(kvp(&with_dir(&dir, &["dump"]))),
        "a=1\na=2\n"
    );
    assert_eq!(assert_success(kvp(&with_dir(&dir, &["entries"]))), "a=2\n");
    assert_eq!(
        assert_success(kvp(&with_dir(&dir, &["delete", "a"]))),
        "true\n"
    );
    assert_eq!(assert_success(kvp(&with_dir(&dir, &["dump"]))), "");

    assert_success(kvp(&with_dir(&dir, &["write", "b", "3"])));
    assert_success(kvp(&with_dir(&dir, &["clear"])));
    assert_eq!(assert_success(kvp(&with_dir(&dir, &["dump"]))), "");
}

#[test]
fn load_replaces_pool_from_file() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("records.txt");
    fs::write(&input, "a=1\nb=2\n").unwrap();

    assert_success(kvp(&with_dir(
        &dir,
        &["load", "--file", input.to_str().unwrap()],
    )));
    assert_eq!(
        assert_success(kvp(&with_dir(&dir, &["dump"]))),
        "a=1\nb=2\n"
    );
}

#[test]
fn load_can_read_from_stdin() {
    let dir = TempDir::new().unwrap();
    assert_success(kvp_with_stdin(&with_dir(&dir, &["load"]), "x=1\ny=2\n"));

    assert_eq!(
        assert_success(kvp(&with_dir(&dir, &["entries"]))),
        "x=1\ny=2\n"
    );
}

#[test]
fn append_multiple_can_read_from_stdin() {
    let dir = TempDir::new().unwrap();
    assert_success(kvp(&with_dir(&dir, &["write", "x", "1"])));
    assert_success(kvp_with_stdin(
        &with_dir(&dir, &["append-multiple"]),
        "x=2\ny=3\n",
    ));

    assert_eq!(
        assert_success(kvp(&with_dir(&dir, &["dump"]))),
        "x=1\nx=2\ny=3\n"
    );
}

#[test]
fn append_multiple_can_read_from_file() {
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("records.txt");
    fs::write(&input, "a=1\nb=2\n").unwrap();

    assert_success(kvp(&with_dir(
        &dir,
        &["append-multiple", "--file", input.to_str().unwrap()],
    )));
    assert_eq!(
        assert_success(kvp(&with_dir(&dir, &["dump"]))),
        "a=1\nb=2\n"
    );
}

#[test]
fn delete_multiple_prints_removed_record_count() {
    let dir = TempDir::new().unwrap();
    assert_success(kvp_with_stdin(
        &with_dir(&dir, &["append-multiple"]),
        "a=1\nb=2\na=3\n",
    ));

    assert_eq!(
        assert_success(kvp(&with_dir(&dir, &["delete-multiple", "a", "z"]))),
        "2\n"
    );
    assert_eq!(assert_success(kvp(&with_dir(&dir, &["entries"]))), "b=2\n");
}

#[test]
fn clear_if_stale_and_is_stale_use_status_apis() {
    let dir = TempDir::new().unwrap();
    assert_success(kvp(&with_dir(&dir, &["clear", "--if-stale"])));

    let output = kvp(&with_dir(&dir, &["is-stale"]));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "false\n");
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
}

#[test]
fn read_missing_exits_one_without_output() {
    let dir = TempDir::new().unwrap();
    let output = kvp(&with_dir(&dir, &["read", "missing"]));

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
}

#[test]
fn validation_errors_exit_two() {
    let dir = TempDir::new().unwrap();
    let output = kvp(&with_dir(&dir, &["write", "", "value"]));
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("KVP key must not be empty"));
}

#[test]
fn json_read_round_trips_value_with_equals_and_newline() {
    let dir = TempDir::new().unwrap();
    // A value containing both '=' and an embedded newline would be
    // ambiguous in the default key=value text output but must survive
    // round-tripping through JSON unchanged.
    let raw_value = "https://example.test/q=1\nline2";
    let status =
        std::process::Command::new(env!("CARGO_BIN_EXE_libazureinit-kvp"))
            .args(with_dir(&dir, &["write", "url", raw_value]))
            .status()
            .unwrap();
    assert!(status.success());

    let stdout =
        assert_success(kvp(&with_dir(&dir, &["--json", "read", "url"])));
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("read --json parses");
    assert_eq!(value["key"], "url");
    assert_eq!(value["value"], raw_value);
}

#[test]
fn report_success_defaults_agent_and_accepts_supporting_data() {
    let dir = TempDir::new().unwrap();
    assert_success(kvp(&with_dir(
        &dir,
        &[
            "report-success",
            "--vm-id",
            "vm-1",
            "--supporting-data",
            "build=123,commit=abc",
        ],
    )));

    let report =
        assert_success(kvp(&with_dir(&dir, &["read", "PROVISIONING_REPORT"])));
    let expected_agent =
        format!("agent=libazureinit-kvp/{}", env!("CARGO_PKG_VERSION"));
    assert!(report.contains(&expected_agent), "report was: {report}");
    assert!(report.contains("vm_id=vm-1"), "report was: {report}");
    assert!(report.trim_end().ends_with("|build=123|commit=abc"));
}

#[test]
fn report_failure_rejects_invalid_supporting_data() {
    let dir = TempDir::new().unwrap();
    let output = kvp(&with_dir(
        &dir,
        &[
            "report-failure",
            "--vm-id",
            "vm-1",
            "--reason",
            "boom",
            "--supporting-data",
            "novalue",
        ],
    ));
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("key=value"));
}
