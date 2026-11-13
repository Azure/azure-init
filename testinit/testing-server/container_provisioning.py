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
SCENARIOS = {
    "success": (0, None),
    "failure": (1, "failed to provision hostname"),
    "config-error": (78, "user not found"),
    "ssh-success": (0, None),
    "sshd-error": (1, "failed to update sshd config"),
    "state-permission-error": (0, None),
    "marker-error": (1, "I/O error"),
    "ovf-fallback": (1, "failed to retrieve instance metadata"),
    "ovf-missing": (1, "failed to determine username"),
    "clean-read-error": (1, "Failed to read directory entry"),
    "clean-marker-error": (1, "Failed to clean provisioning marker"),
    "clean-race": (0, "No provisioning marker found"),
    "passwords": (0, None),
}
OVF_ENV = """<Environment xmlns="http://schemas.dmtf.org/ovf/environment/1"
    xmlns:wa="http://schemas.microsoft.com/windowsazure">
  <wa:ProvisioningSection>
    <wa:Version>1.0</wa:Version>
    <LinuxProvisioningConfigurationSet xmlns="http://schemas.microsoft.com/windowsazure">
      <UserName>ovf-user</UserName><HostName>coverage-host</HostName>
    </LinuxProvisioningConfigurationSet>
  </wa:ProvisioningSection>
  <wa:PlatformSettingsSection>
    <wa:Version>1.0</wa:Version>
    <PlatformSettings xmlns="http://schemas.microsoft.com/windowsazure"/>
  </wa:PlatformSettingsSection>
</Environment>"""


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
            "status_code": 403 if scenario.startswith("ovf-") else 200,
            "headers": {"Content-Type": "application/json"},
            "response": {
                "compute": {
                    "osProfile": {
                        "adminUsername": "root" if scenario == "ssh-success" else USERNAME,
                        "computerName": "coverage-host",
                        "disablePasswordAuthentication": "true",
                    },
                    "publicKeys": (
                        [{"keyData": "ssh-ed25519 test-key", "path": "/unused"}]
                        if scenario in ("config-error", "ssh-success")
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
    check_mount_table(Path("/proc/mounts"))
    os.umask(0o022)
    mtab = Path("/etc/mtab")
    mtab.unlink(missing_ok=True)
    mtab.write_text(Path("/test/mtab").read_text())

    requests = []
    service_errors = []
    metadata_handler, health_handler = make_handlers(scenario, requests, service_errors)

    artifacts = Path("/artifacts")
    work = Path("/work")
    cleaning = scenario.startswith("clean-")
    if scenario == "marker-error":
        (work / "state").write_text("not a directory")
    if scenario == "sshd-error":
        Path("/etc/ssh/sshd_config").mkdir()
    if scenario == "ssh-success":
        assert pwd.getpwnam("root").pw_dir == "/root"
    if cleaning:
        (work / "state").mkdir()
        marker = work / "state" / "vanished.provisioned"
        if scenario == "clean-marker-error":
            marker.mkdir()
        else:
            marker.write_text("complete")
        (work / "state" / "keep.txt").write_text("keep")
    (work / "ovf-env.xml").write_text(OVF_ENV)
    commands = work / "commands.jsonl"
    stubs = work / "bin"
    stubs.mkdir()
    stub = stubs / "command-stub"
    stub.write_text(
        "#!/usr/bin/python3\n"
        "import json, os, shutil, sys\n"
        "name = os.path.basename(sys.argv[0])\n"
        "with open('/work/commands.jsonl', 'a') as output:\n"
        "    output.write(json.dumps([name, *sys.argv[1:]]) + '\\n')\n"
        "if name == 'getent':\n"
        "    if os.environ['TEST_SCENARIO'] == 'ssh-success':\n"
        "        assert sys.argv[1:] == ['passwd', 'root']\n"
        "        sys.exit(0)\n"
        "    sys.exit(1)\n"
        "if name == 'chpasswd':\n"
        "    assert os.environ['TEST_SCENARIO'] == 'passwords'\n"
        "    data = sys.stdin.read()\n"
        "    with open('/work/passwords.jsonl', 'a') as output:\n"
        "        output.write(json.dumps({'args': sys.argv[1:], 'stdin': data}) + '\\n')\n"
        "    if data == 'coverage-user:synthetic-rejected-password':\n"
        "        print('expected password failure', file=sys.stderr)\n"
        "        sys.exit(17)\n"
        "    sys.exit(0)\n"
        "if name == 'sshd':\n"
        "    assert os.environ['TEST_SCENARIO'] == 'ssh-success'\n"
        "    assert sys.argv[1:] == ['-G']\n"
        "    print('authorizedkeysfile .ssh/queried_keys')\n"
        "    sys.exit(0)\n"
        "if name == 'mount':\n"
        "    if sys.argv[-2] != '/test/ovf-good':\n"
        "        sys.exit(17)\n"
        "    shutil.copyfile('/work/ovf-env.xml', os.path.join(sys.argv[-1], 'ovf-env.xml'))\n"
        "if name == 'hostnamectl' and os.environ['TEST_SCENARIO'] == 'failure':\n"
        "    print('expected hostname failure', file=sys.stderr)\n"
        "    sys.exit(17)\n"
        "if name not in ('hostnamectl', 'useradd', 'usermod', 'passwd', 'mount', 'umount', 'eject'):\n"
        "    raise RuntimeError('unexpected command: ' + name)\n"
    )
    stub.chmod(0o755)
    for name in (
        "hostnamectl", "getent", "useradd", "usermod", "passwd",
        "chpasswd", "sshd", "mount", "umount", "eject",
    ):
        (stubs / name).symlink_to(stub)

    if scenario == "passwords":
        result = subprocess.run(
            ["/test/testinit", "--ignored", "isolated_password_provisioning",
             "--test-threads=1", "--nocapture"],
            env=dict(os.environ, PATH=str(stubs), TEST_SCENARIO=scenario),
            capture_output=True, text=True, timeout=20,
        )
        (artifacts / "stdout").write_text(result.stdout)
        (artifacts / "stderr").write_text(result.stderr)
        result.check_returncode()
        passwords = [
            json.loads(line)
            for line in (work / "passwords.jsonl").read_text().splitlines()
        ]
        assert all(request["args"] == [] for request in passwords), passwords
        assert sorted(request["stdin"] for request in passwords) == sorted([
            f"{USERNAME}:synthetic password:with-colon",
            f"{USERNAME}:synthetic-rejected-password",
        ] * 2), passwords
        log = result.stdout + result.stderr
        assert log.count("Successfully set password via chpasswd") == 2, log
        assert log.count("chpasswd failed to set password") == 2, log
        assert "synthetic password:with-colon" not in log, log
        assert "synthetic-rejected-password" not in log, log
        for output in artifacts.iterdir():
            output.chmod(0o644)
        print("Verified passwords: stdin/EOF, redaction, and both API error contracts")
        return

    with (
        HTTPServer(("127.0.0.1", 0), metadata_handler) as imds_server,
        HTTPServer(("127.0.0.1", 0), health_handler) as health_server,
    ):
        imds_endpoint = f"http://127.0.0.1:{imds_server.server_port}"
        health_endpoint = f"http://127.0.0.1:{health_server.server_port}"
        config = work / "azure-init.toml"
        config.write_text(
            '[ssh]\nconfigure_sshd_password_authentication = '
            f'{str(scenario in ("ssh-success", "sshd-error")).lower()}\n'
            f'query_sshd_config = {str(scenario == "ssh-success").lower()}\n'
            'authorized_keys_path = ".ssh/fallback_keys"\n'
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
        if scenario in ("clean-read-error", "clean-race", "state-permission-error"):
            environment["LD_PRELOAD"] = "/test/fs_faults.so"
        servers = (imds_server, health_server)
        threads = [
            threading.Thread(target=server.serve_forever, daemon=True)
            for server in servers
        ]
        for thread in threads:
            thread.start()
        try:
            result = subprocess.run(
                ["/test/azure-init", "--config", str(config),
                 *(["--groups", "wheel,docker"] if scenario == "ssh-success" else []),
                 *(["clean"] if cleaning else [])],
                env=environment,
                capture_output=True,
                text=True,
                timeout=20,
            )
            (artifacts / "stdout").write_text(result.stdout)
            (artifacts / "stderr").write_text(result.stderr)
            assert not service_errors, service_errors
            expected_status, reason = SCENARIOS[scenario]
            assert result.returncode == expected_status, result
            log = (work / "azure-init.log").read_text()
            if cleaning:
                assert requests == [], requests
                assert not commands.exists()
                assert marker.exists() == (scenario != "clean-race")
                assert (work / "state" / "keep.txt").read_text() == "keep"
                assert reason in log, log
            else:
                assert [kind for kind, _ in requests] == ["imds", "report"], requests
                report = requests[1][1]
                (artifacts / "http-report.json").write_text(json.dumps(report))
                marker = work / "state" / f"{VM_ID}.provisioned"
                calls = [json.loads(line) for line in commands.read_text().splitlines()]
                if scenario == "ovf-fallback":
                    expected_commands = ["mount", "mount", "umount", "eject", "mount"]
                    assert log.count("Successfully parsed OVF environment.") == 1, log
                elif scenario == "ovf-missing":
                    expected_commands = ["mount", "mount"]
                elif scenario == "ssh-success":
                    expected_commands = ["hostnamectl", "getent", "usermod", "passwd", "sshd"]
                else:
                    expected_commands = (
                        ["hostnamectl"]
                        if scenario == "failure"
                        else ["hostnamectl", "getent", "useradd", "passwd"]
                    )
                assert [call[0] for call in calls] == expected_commands, calls
                if expected_status == 0:
                    assert report == {"state": "Ready"}, report
                    assert marker.is_file()
                    assert Path("/etc/sudoers.d/azure-init-user").read_text() == (
                        f"{'root' if scenario == 'ssh-success' else USERNAME} ALL=(ALL) NOPASSWD: ALL\n"
                    )
                else:
                    assert report["state"] == "NotReady", report
                    assert report["details"]["subStatus"] == "ProvisioningFailed", report
                    assert f"reason={reason}" in report["details"]["description"], report
                    assert not marker.exists()
                if scenario == "marker-error":
                    assert (work / "state").read_text() == "not a directory"
                    assert "Failed to mark provisioning complete." in log, log
                if scenario == "state-permission-error":
                    assert "Failed to set permissions on /work/state:" in log, log
                    assert (work / "state").stat().st_mode & 0o777 == 0o755
                    assert marker.stat().st_mode & 0o777 == 0o600
                if scenario == "ssh-success":
                    assert calls[2] == ["usermod", "-aG", "wheel,docker", "root"], calls
                    assert calls[-1] == ["sshd", "-G"], calls
                    assert Path("/etc/ssh/sshd_config").read_text() == (
                        "PasswordAuthentication no\n"
                    )
                    assert Path("/etc/ssh/sshd_config").stat().st_mode & 0o777 == 0o600
                    ssh_dir = Path("/root/.ssh")
                    authorized_keys = ssh_dir / "queried_keys"
                    assert authorized_keys.read_text() == "ssh-ed25519 test-key\n"
                    assert not (ssh_dir / "fallback_keys").exists()
                    for path, mode in ((ssh_dir, 0o700), (authorized_keys, 0o600)):
                        metadata = path.stat()
                        assert metadata.st_mode & 0o777 == mode
                        assert metadata.st_uid == metadata.st_gid == 0
                if scenario == "sshd-error":
                    assert "Failed to update sshd configuration for password authentication" in log, log
                    assert Path("/etc/ssh/sshd_config").is_dir()
                expected_log = (
                    "Provisioning completed successfully"
                    if expected_status == 0
                    else "Provisioning failed with error:"
                )
                assert expected_log in log, log
            shutil.copyfile("/var/lib/hyperv/.kvp_pool_1", artifacts / ".kvp_pool_1")
            print(f"Verified {scenario}: actual agent exit {result.returncode}")
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


def run_containers(agent, artifacts, test_binary):
    profile_pattern = os.environ.get("LLVM_PROFILE_FILE")
    for scenario in SCENARIOS:
        output = artifacts / scenario
        output.mkdir()
        # Container root lacks DAC_OVERRIDE; only this disposable output is writable.
        output.chmod(0o777)
        with tempfile.TemporaryDirectory(prefix="azure-init-fixture-") as temporary:
            empty_sys = Path(temporary) / "sys"
            empty_sys.mkdir(mode=0o755)
            mtab = Path(temporary) / "mtab"
            devices = []
            if scenario.startswith("ovf-"):
                devices = ["/test/missing-before", "/test/missing-after"]
                if scenario == "ovf-fallback":
                    devices.insert(1, "/test/ovf-good")
            mtab.write_text("".join(f"{device} /media iso9660 ro 0 0\n" for device in devices))
            mtab.chmod(0o644)
            command = [
                "docker", "create", "--name", f"azure-init-test-{uuid.uuid4().hex}",
                "--network", "none", "--cap-drop", "ALL",
                "--security-opt", "no-new-privileges", "--read-only",
                "--mount", "type=volume,dst=/etc",
                "--tmpfs", "/work:exec,mode=0700",
                "--tmpfs", "/var/lib/hyperv:mode=0700",
                "--tmpfs", "/etc/sudoers.d:mode=0700",
                "--tmpfs", "/etc/ssh:mode=0700",
                "--tmpfs", "/root:mode=0700",
                "--tmpfs", "/run/azure-init/media:mode=0700",
                "--mount", f"type=bind,src={agent},dst=/test/azure-init,readonly",
                "--mount", f"type=bind,src={test_binary},dst=/test/testinit,readonly",
                "--mount", f"type=bind,src={empty_sys},dst=/sys,readonly",
                "--mount", f"type=bind,src={mtab},dst=/test/mtab,readonly",
                "--mount", f"type=bind,src={artifacts / 'fs_faults.so'},dst=/test/fs_faults.so,readonly",
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
                    ["docker", "rm", "--force", "--volumes", container],
                    check=True, capture_output=True, text=True, timeout=15,
                )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--inside", choices=SCENARIOS)
    parser.add_argument("--agent", type=Path)
    parser.add_argument("--test-binary", type=Path)
    parser.add_argument("--artifacts", type=Path)
    args = parser.parse_args()
    if args.inside:
        inside_container(args.inside)
    elif args.agent and args.artifacts and args.test_binary:
        artifacts = args.artifacts.resolve(strict=True)
        faults = artifacts / "fs_faults.so"
        subprocess.run(
            ["cc", "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
             str(Path(__file__).with_name("fs_faults.c")), "-o", str(faults), "-ldl"],
            check=True, timeout=30,
        )
        faults.chmod(0o644)
        run_containers(
            args.agent.resolve(strict=True), artifacts,
            args.test_binary.resolve(strict=True),
        )
    else:
        parser.error("supply --inside or --agent, --test-binary, and --artifacts")
