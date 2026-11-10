# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

"""Exercise the real agent against controlled services in a disposable container."""

import argparse
from http.server import HTTPServer
import json
import os
from pathlib import Path
import pwd
import shutil
import subprocess
import tempfile
import threading
import uuid

from imds_handler import IMDSHandler
from wireserver_handler import WireServerHandler


IMAGE = "azure-init-main-tests:local"
VM_ID = "00000000-0000-0000-0000-000000000000"
USERNAME = "coverage-user"
SCENARIOS = ("success", "failure", "config-error")


def check_mount_table(mtab):
    for entry in mtab.read_text().splitlines():
        fields = entry.split()
        if not fields or fields[0].startswith("#"):
            continue
        if len(fields) < 3:
            raise RuntimeError(f"invalid mount table entry: {entry!r}")
        if fields[2] in ("iso9660", "udf"):
            raise RuntimeError(f"fixture must not expose mounted {fields[2]} media")


def make_handlers(scenario, requests, service_errors):
    class CheckedHandler:
        def log_message(self, *_args):
            pass

        def handle(self):
            try:
                super().handle()
            except (AssertionError, ValueError) as error:
                service_errors.append(str(error))
                raise

    class MetadataHandler(CheckedHandler, IMDSHandler):
        _response_position = 0
        _responses = [{
            "status_code": 200,
            "headers": {"Content-Type": "application/json"},
            "response": {
                "compute": {
                    "osProfile": {
                        "adminUsername": USERNAME,
                        "computerName": "coverage-host",
                        "disablePasswordAuthentication": "true",
                    },
                    "publicKeys": (
                        [{"keyData": "ssh-ed25519 test-key", "path": "/unused"}]
                        if scenario == "config-error"
                        else []
                    ),
                }
            },
        }]

        def do_GET(self):
            assert self.path.startswith(
                "http://169.254.169.254/metadata/instance?"
            ), self.path
            assert self.headers["Metadata"] == "true"
            requests.append(("imds", None))
            super().do_GET()

    class HealthHandler(CheckedHandler, WireServerHandler):
        _response_position = 0
        _responses = [{
            "status_code": 201,
            "headers": {"Content-Length": "0"},
            "response": "",
        }]

        def record_report(self, post_data):
            assert self.path == "/provisioning/health", self.path
            requests.append(("report", json.loads(post_data)))

    return MetadataHandler, HealthHandler


def inside_container(scenario):
    if not Path("/test/isolated-provisioning").is_file():
        raise RuntimeError("provisioning fixture must only run in its isolated image")
    if Path("/sys/class/dmi/id/product_uuid").exists():
        raise RuntimeError("fixture requires an isolated, empty /sys")
    if any(user.pw_name == USERNAME for user in pwd.getpwall()):
        raise RuntimeError(f"fixture requires a nonexistent account: {USERNAME}")
    check_mount_table(Path("/etc/mtab"))

    requests = []
    service_errors = []
    metadata_handler, health_handler = make_handlers(scenario, requests, service_errors)

    artifacts = Path("/artifacts")
    work = Path("/work")
    commands = work / "commands.jsonl"
    stubs = work / "bin"
    stubs.mkdir()
    stub = stubs / "command-stub"
    stub.write_text(
        "#!/usr/bin/python3\n"
        "import json, os, sys\n"
        "name = os.path.basename(sys.argv[0])\n"
        "with open('/work/commands.jsonl', 'a') as output:\n"
        "    output.write(json.dumps([name, *sys.argv[1:]]) + '\\n')\n"
        "if name == 'getent':\n"
        "    sys.exit(1)\n"
        "if name == 'hostnamectl' and os.environ['TEST_SCENARIO'] == 'failure':\n"
        "    print('expected hostname failure', file=sys.stderr)\n"
        "    sys.exit(17)\n"
        "if name not in ('hostnamectl', 'useradd', 'usermod', 'passwd'):\n"
        "    raise RuntimeError('unexpected command: ' + name)\n"
    )
    stub.chmod(0o755)
    for name in ("hostnamectl", "getent", "useradd", "usermod", "passwd"):
        (stubs / name).symlink_to(stub)

    with (
        HTTPServer(("127.0.0.1", 0), metadata_handler) as imds_server,
        HTTPServer(("127.0.0.1", 0), health_handler) as health_server,
    ):
        imds_endpoint = f"http://127.0.0.1:{imds_server.server_port}"
        health_endpoint = f"http://127.0.0.1:{health_server.server_port}"
        config = work / "azure-init.toml"
        config.write_text(
            '[ssh]\nconfigure_sshd_password_authentication = false\n'
            'query_sshd_config = false\n'
            '[azure_init_data_dir]\npath = "/work/state"\n'
            '[azure_init_log_path]\npath = "/work/azure-init.log"\n'
            '[imds]\nconnection_timeout_secs = 1\nrequest_timeout_secs = 1\n'
            'retry_interval_secs = 0.01\ntotal_retry_timeout_secs = 3\n'
            '[wireserver]\nconnection_timeout_secs = 0.01\n'
            'read_timeout_secs = 1\ntotal_retry_timeout_secs = 3\n'
            f'health_endpoint = "{health_endpoint}/provisioning/health"\n'
            '[telemetry]\nkvp_diagnostics = true\nkvp_filter = "info"\n'
        )
        environment = dict(
            os.environ,
            PATH=str(stubs),
            TEST_SCENARIO=scenario,
            HTTP_PROXY=imds_endpoint,
            http_proxy=imds_endpoint,
            NO_PROXY="127.0.0.1,localhost",
            no_proxy="127.0.0.1,localhost",
            AZURE_INIT_LOG="info",
        )
        servers = (imds_server, health_server)
        threads = [
            threading.Thread(target=server.serve_forever, daemon=True)
            for server in servers
        ]
        for thread in threads:
            thread.start()
        try:
            result = subprocess.run(
                ["/test/azure-init", "--config", str(config)],
                env=environment,
                capture_output=True,
                text=True,
                timeout=20,
            )
            (artifacts / "stdout").write_text(result.stdout)
            (artifacts / "stderr").write_text(result.stderr)
            assert not service_errors, service_errors
            expected_status = {"success": 0, "failure": 1, "config-error": 78}[scenario]
            assert result.returncode == expected_status, result
            assert [kind for kind, _ in requests] == ["imds", "report"], requests
            report = requests[1][1]
            (artifacts / "http-report.json").write_text(json.dumps(report))
            marker = work / "state" / f"{VM_ID}.provisioned"
            calls = [json.loads(line) for line in commands.read_text().splitlines()]
            expected_commands = (
                ["hostnamectl"]
                if scenario == "failure"
                else ["hostnamectl", "getent", "useradd", "passwd"]
            )
            assert [call[0] for call in calls] == expected_commands, calls
            if scenario == "success":
                assert report == {"state": "Ready"}, report
                assert marker.is_file()
                assert Path("/etc/sudoers.d/azure-init-user").read_text() == (
                    f"{USERNAME} ALL=(ALL) NOPASSWD: ALL\n"
                )
            else:
                assert report["state"] == "NotReady", report
                assert report["details"]["subStatus"] == "ProvisioningFailed", report
                reason = (
                    "failed to provision hostname"
                    if scenario == "failure"
                    else "user not found"
                )
                assert f"reason={reason}" in report["details"]["description"], report
                assert not marker.exists()
            log = (work / "azure-init.log").read_text()
            expected_log = (
                "Provisioning completed successfully"
                if scenario == "success"
                else "Provisioning failed with error:"
            )
            assert expected_log in log, log
            shutil.copyfile("/var/lib/hyperv/.kvp_pool_1", artifacts / ".kvp_pool_1")
            print(f"Verified {scenario}: actual agent exit {result.returncode} and health report")
        finally:
            for server in servers:
                server.shutdown()
            for thread in threads:
                thread.join(timeout=5)
                assert not thread.is_alive(), "mock server did not shut down"
            log = work / "azure-init.log"
            if log.exists():
                shutil.copyfile(log, artifacts / "azure-init.log")
            for output in artifacts.iterdir():
                output.chmod(0o644)


def run_containers(agent, artifacts):
    profile_pattern = os.environ.get("LLVM_PROFILE_FILE")
    for scenario in SCENARIOS:
        output = artifacts / scenario
        output.mkdir()
        # Container root lacks DAC_OVERRIDE; only this disposable output is writable.
        output.chmod(0o777)
        with tempfile.TemporaryDirectory(prefix="azure-init-empty-sys-") as empty_sys:
            Path(empty_sys).chmod(0o755)
            command = [
                "docker", "create", "--name", f"azure-init-test-{uuid.uuid4().hex}",
                "--network", "none", "--cap-drop", "ALL",
                "--security-opt", "no-new-privileges", "--read-only",
                "--tmpfs", "/work:exec,mode=0700",
                "--tmpfs", "/var/lib/hyperv:mode=0700",
                "--tmpfs", "/etc/sudoers.d:mode=0700",
                "--mount", f"type=bind,src={agent},dst=/test/azure-init,readonly",
                "--mount", f"type=bind,src={empty_sys},dst=/sys,readonly",
                "--mount", f"type=bind,src={output},dst=/artifacts",
            ]
            if profile_pattern:
                command.extend([
                    "--env",
                    f"LLVM_PROFILE_FILE=/artifacts/{Path(profile_pattern).name}",
                ])
            command.extend([IMAGE, scenario])
            created = subprocess.run(
                command, check=True, capture_output=True, text=True, timeout=30
            )
            container = created.stdout.strip()
            try:
                result = subprocess.run(
                    ["docker", "start", "--attach", container],
                    capture_output=True, text=True, timeout=60,
                )
                print(result.stdout, end="")
                print(result.stderr, end="")
                result.check_returncode()
                if profile_pattern:
                    profiles = list(output.glob("*.profraw"))
                    assert profiles, "instrumented agent produced no coverage profile"
                    destination = Path(profile_pattern).parent
                    for profile in profiles:
                        shutil.copyfile(
                            profile,
                            destination / f"{profile.stem}-{container[:12]}-{scenario}.profraw",
                        )
            finally:
                subprocess.run(
                    ["docker", "rm", "--force", container],
                    check=True, capture_output=True, text=True, timeout=15,
                )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--inside", choices=SCENARIOS)
    parser.add_argument("--agent", type=Path)
    parser.add_argument("--artifacts", type=Path)
    args = parser.parse_args()
    if args.inside:
        inside_container(args.inside)
    elif args.agent and args.artifacts:
        run_containers(args.agent.resolve(strict=True), args.artifacts.resolve(strict=True))
    else:
        parser.error("supply --inside or both --agent and --artifacts")
