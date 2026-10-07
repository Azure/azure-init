"""Run the real agent against deterministic services in a disposable container."""

import argparse
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import uuid


IMAGE = "azure-init-main-tests:local"
VM_ID = "00000000-0000-0000-0000-000000000000"
SCENARIOS = ("success", "failure")


def inside_container(scenario):
    if not Path("/test/isolated-provisioning").is_file():
        raise RuntimeError("provisioning fixture must only run in its isolated image")
    if Path("/sys/class/dmi/id/product_uuid").exists():
        raise RuntimeError("fixture requires an isolated, empty /sys")
    if Path("/etc/mtab").read_text():
        raise RuntimeError("fixture requires an empty mount table")

    requests = []
    service_errors = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def handle(self):
            try:
                super().handle()
            except (AssertionError, ValueError) as error:
                service_errors.append(str(error))
                raise

        def send_json(self, status, value):
            body = json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            assert self.path.startswith(
                "http://169.254.169.254/metadata/instance?"
            ), self.path
            assert self.headers["Metadata"] == "true"
            requests.append(("imds", None))
            self.send_json(
                200,
                {
                    "compute": {
                        "osProfile": {
                            "adminUsername": "coverage-user",
                            "computerName": "coverage-host",
                            "disablePasswordAuthentication": "true",
                        },
                        "publicKeys": [],
                    }
                },
            )

        def do_POST(self):
            assert self.path == "/provisioning/health", self.path
            body = self.rfile.read(int(self.headers["Content-Length"]))
            requests.append(("report", json.loads(body)))
            self.send_json(201, {})

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

    with HTTPServer(("127.0.0.1", 0), Handler) as server:
        endpoint = f"http://127.0.0.1:{server.server_port}"
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
            f'health_endpoint = "{endpoint}/provisioning/health"\n'
            '[telemetry]\nkvp_diagnostics = true\nkvp_filter = "info"\n'
        )
        environment = dict(
            os.environ,
            PATH=str(stubs),
            TEST_SCENARIO=scenario,
            HTTP_PROXY=endpoint,
            http_proxy=endpoint,
            NO_PROXY="127.0.0.1,localhost",
            no_proxy="127.0.0.1,localhost",
            AZURE_INIT_LOG="info",
        )
        thread = threading.Thread(target=server.serve_forever, daemon=True)
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
            expected_status = 0 if scenario == "success" else 1
            assert result.returncode == expected_status, result
            assert [kind for kind, _ in requests] == ["imds", "report"], requests
            report = requests[1][1]
            if scenario == "success":
                assert report == {"state": "Ready"}, report
                assert (work / "state" / f"{VM_ID}.provisioned").is_file()
                assert Path("/etc/sudoers.d/azure-init-user").read_text() == (
                    "coverage-user ALL=(ALL) NOPASSWD: ALL\n"
                )
                calls = [json.loads(line) for line in commands.read_text().splitlines()]
                assert [call[0] for call in calls] == [
                    "hostnamectl", "getent", "useradd", "passwd"
                ], calls
                assert calls[0] == ["hostnamectl", "set-hostname", "coverage-host"]
                assert calls[-1] == ["passwd", "-l", "coverage-user"]
                completed = subprocess.run(
                    ["/test/azure-init", "--config", str(config)],
                    env=environment,
                    capture_output=True,
                    text=True,
                    timeout=20,
                )
                assert completed.returncode == 0, completed
                assert len(requests) == 2, "completed provisioning contacted a service"
                assert len(commands.read_text().splitlines()) == 4
            else:
                assert report["state"] == "NotReady", report
                assert report["details"]["subStatus"] == "ProvisioningFailed", report
                description = report["details"]["description"]
                assert "reason=failed to provision hostname" in description
                assert not (work / "state" / f"{VM_ID}.provisioned").exists()
                (artifacts / "http-description").write_text(description)
            log = (work / "azure-init.log").read_text()
            assert "Failed to find valid OVF provisioning data" in log
            shutil.copyfile("/var/lib/hyperv/.kvp_pool_1", artifacts / ".kvp_pool_1")
            print(f"Verified {scenario}: missing VM ID, OVF fallback, provisioning and reporting")
        finally:
            server.shutdown()
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
        # The container drops DAC_OVERRIDE, so its root user needs explicit
        # access to these host-owned, disposable bind mounts.
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
