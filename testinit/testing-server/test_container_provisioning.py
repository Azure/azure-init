# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

"""Check shared HTTP handlers, fixture isolation, and the Docker lifecycle."""

from contextlib import contextmanager, redirect_stdout
from http.client import HTTPConnection, RemoteDisconnected
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch

import container_provisioning as fixture


class HandlerTests(unittest.TestCase):
    @contextmanager
    def serve(self, handler):
        with fixture.HTTPServer(("127.0.0.1", 0), handler) as server:
            thread = threading.Thread(
                target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True
            )
            thread.start()
            connection = HTTPConnection(*server.server_address, timeout=5)
            try:
                yield connection
            finally:
                connection.close()
                server.shutdown()
                thread.join(timeout=5)
                self.assertFalse(thread.is_alive(), "mock server did not shut down")

    def test_scenarios_share_handlers_with_fresh_response_state(self):
        for scenario in fixture.SCENARIOS:
            with self.subTest(scenario=scenario):
                requests, errors = [], []
                metadata, health = fixture.make_handlers(scenario, requests, errors)
                self.assertEqual(metadata._response_position, 0)
                self.assertEqual(health._response_position, 0)
                with self.serve(metadata) as connection:
                    connection.request(
                        "GET", "http://169.254.169.254/metadata/instance?extended=true",
                        headers={"Metadata": "true"},
                    )
                    response = connection.getresponse()
                    self.assertEqual(response.status, 200)
                    compute = json.loads(response.read())["compute"]
                    self.assertEqual(compute["osProfile"]["adminUsername"], fixture.USERNAME)
                    self.assertEqual(bool(compute["publicKeys"]), scenario == "config-error")
                report = {"state": "Ready" if scenario == "success" else "NotReady"}
                with self.serve(health) as connection:
                    connection.request("POST", "/provisioning/health", json.dumps(report))
                    response = connection.getresponse()
                    self.assertEqual(response.status, 201)
                    self.assertEqual(response.read(), b"")
                self.assertEqual(requests, [("imds", None), ("report", report)])
                self.assertEqual(errors, [])
                self.assertEqual(metadata._response_position, 1)
                self.assertEqual(health._response_position, 1)

    def test_invalid_requests_are_reported(self):
        requests, errors = [], []
        metadata, health = fixture.make_handlers("success", requests, errors)
        for handler, method, path, body in (
            (metadata, "GET", "/unexpected", ""),
            (health, "POST", "/provisioning/health", "not-json"),
        ):
            with self.subTest(method=method), patch.object(fixture.HTTPServer, "handle_error"):
                with self.serve(handler) as connection:
                    connection.request(method, path, body)
                    with self.assertRaises(RemoteDisconnected):
                        connection.getresponse()
        self.assertEqual(len(errors), 2)
        self.assertEqual(requests, [])

    def test_systemd_health_responses_still_snapshot_kvp(self):
        for custom in (False, True):
            with self.subTest(custom=custom):
                class Handler(fixture.WireServerHandler):
                    _response_position = 0

                if custom:
                    response_file = Path(fixture.__file__).parent / (
                        "api-responses/wireserver/one-bad-then-success.xml"
                    )
                    Handler.set_response_file_path(response_file)
                    Handler.load_responses()
                statuses = (500, 200) if custom else (200,)
                with (
                    patch.object(Handler, "store_kvp_file") as snapshot,
                    patch("wireserver_handler.time.sleep"),
                    self.serve(Handler) as connection,
                ):
                    for status in statuses:
                        connection.request("POST", "/provisioning/health", '{"state":"Ready"}')
                        response = connection.getresponse()
                        self.assertEqual(response.status, status)
                        self.assertEqual(response.read(), b"")
                self.assertEqual(snapshot.call_count, len(statuses))


class MountTableTests(unittest.TestCase):
    def check_mount_table(self, contents):
        with tempfile.TemporaryDirectory() as temporary:
            mtab = Path(temporary) / "mtab"
            mtab.write_text(contents)
            fixture.check_mount_table(mtab)

    def test_allows_empty_and_standard_container_mounts(self):
        for contents in ("", "# comment\n", "overlay / overlay rw 0 0\n"):
            with self.subTest(contents=contents):
                self.check_mount_table(contents)

    def test_rejects_provisioning_media(self):
        for filesystem in ("iso9660", "udf"):
            with self.subTest(filesystem=filesystem):
                with self.assertRaisesRegex(RuntimeError, filesystem):
                    self.check_mount_table(f"/dev/sr0 /media {filesystem} ro 0 0\n")

    def test_rejects_malformed_mount_entries(self):
        with self.assertRaisesRegex(RuntimeError, "invalid mount table"):
            self.check_mount_table("incomplete\n")


class ContainerRunnerTests(unittest.TestCase):
    def run_fixture(self, root, docker, profile_pattern=None):
        artifacts = root / "artifacts"
        artifacts.mkdir()
        with patch.dict(os.environ):
            os.environ.pop("LLVM_PROFILE_FILE", None)
            if profile_pattern:
                os.environ["LLVM_PROFILE_FILE"] = str(profile_pattern)
            with patch.object(fixture.subprocess, "run", side_effect=docker):
                with redirect_stdout(io.StringIO()):
                    fixture.run_containers(Path("/test-agent"), artifacts)

    def test_collects_profiles_and_removes_each_container(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            profiles = root / "profiles"
            profiles.mkdir()
            calls = []
            output = None

            def docker(command, **_options):
                nonlocal output
                calls.append(command[1])
                if command[1] == "create":
                    mount = next(arg for arg in command if arg.endswith("dst=/artifacts"))
                    output = Path(mount.split("src=", 1)[1].split(",dst=", 1)[0])
                    self.assertEqual(output.stat().st_mode & 0o777, 0o777)
                    self.assertIn("LLVM_PROFILE_FILE=/artifacts/agent-%p-%m.profraw", command)
                    self.assertEqual(command[command.index("--network") + 1], "none")
                    self.assertEqual(command[command.index("--cap-drop") + 1], "ALL")
                    self.assertIn("no-new-privileges", command)
                    self.assertIn("--read-only", command)
                    self.assertIn("/work:exec,mode=0700", command)
                    return subprocess.CompletedProcess(command, 0, "container-id\n", "")
                if command[1] == "start":
                    (output / "agent-1-123.profraw").write_bytes(b"synthetic profile")
                return subprocess.CompletedProcess(command, 0, "", "")

            self.run_fixture(root, docker, profiles / "agent-%p-%m.profraw")
            self.assertEqual(calls, ["create", "start", "rm"] * len(fixture.SCENARIOS))
            for scenario in fixture.SCENARIOS:
                profile = profiles / f"agent-1-123-container-id-{scenario}.profraw"
                self.assertEqual(profile.read_bytes(), b"synthetic profile")

    def test_normal_run_does_not_require_profiles(self):
        with tempfile.TemporaryDirectory() as temporary:
            calls = []

            def docker(command, **_options):
                calls.append(command[1])
                self.assertFalse(any(arg.startswith("LLVM_PROFILE_FILE=") for arg in command))
                return subprocess.CompletedProcess(command, 0, "container-id\n", "")

            self.run_fixture(Path(temporary), docker)
            self.assertEqual(calls, ["create", "start", "rm"] * len(fixture.SCENARIOS))

    def test_failure_and_timeout_still_remove_the_container(self):
        for timed_out in (False, True):
            with self.subTest(timed_out=timed_out), tempfile.TemporaryDirectory() as temporary:
                calls = []

                def docker(command, **_options):
                    calls.append(command[1])
                    if command[1] == "start":
                        if timed_out:
                            raise subprocess.TimeoutExpired(command, 60)
                        return subprocess.CompletedProcess(command, 1, "", "fixture failed")
                    return subprocess.CompletedProcess(command, 0, "container-id\n", "")

                error = subprocess.TimeoutExpired if timed_out else subprocess.CalledProcessError
                with self.assertRaises(error):
                    self.run_fixture(Path(temporary), docker)
                self.assertEqual(calls, ["create", "start", "rm"])

    def test_missing_profile_is_an_error_and_cleans_up(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            calls = []

            def docker(command, **_options):
                calls.append(command[1])
                return subprocess.CompletedProcess(command, 0, "container-id\n", "")

            with self.assertRaisesRegex(AssertionError, "no coverage profile"):
                self.run_fixture(root, docker, root / "agent-%p-%m.profraw")
            self.assertEqual(calls, ["create", "start", "rm"])


if __name__ == "__main__":
    unittest.main()
