"""Check container invocation and profile collection without a Docker daemon."""

from contextlib import redirect_stdout
import io
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import container_provisioning as fixture


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
                calls.append(command)
                if command[1] == "create":
                    mount = next(
                        arg for arg in command if arg.endswith("dst=/artifacts")
                    )
                    output = Path(mount.split("src=", 1)[1].split(",dst=", 1)[0])
                    self.assertEqual(output.stat().st_mode & 0o777, 0o777)
                    self.assertIn(
                        "LLVM_PROFILE_FILE=/artifacts/agent-%p-%m.profraw", command
                    )
                    self.assertEqual(command[command.index("--network") + 1], "none")
                    self.assertIn("/work:exec,mode=0700", command)
                    self.assertIn("--read-only", command)
                    return subprocess.CompletedProcess(command, 0, "container-id\n", "")
                if command[1] == "start":
                    (output / "agent-1-123.profraw").write_bytes(b"profile bytes")
                return subprocess.CompletedProcess(command, 0, "", "")

            self.run_fixture(root, docker, profiles / "agent-%p-%m.profraw")
            self.assertEqual(
                [command[1] for command in calls], ["create", "start", "rm"] * 2
            )
            for scenario in fixture.SCENARIOS:
                profile = profiles / f"agent-1-123-container-id-{scenario}.profraw"
                self.assertEqual(profile.read_bytes(), b"profile bytes")

    def test_run_without_coverage_does_not_require_profiles(self):
        with tempfile.TemporaryDirectory() as temporary:
            calls = []

            def docker(command, **_options):
                calls.append(command)
                self.assertFalse(
                    any(arg.startswith("LLVM_PROFILE_FILE=") for arg in command)
                )
                return subprocess.CompletedProcess(command, 0, "container-id\n", "")

            self.run_fixture(Path(temporary), docker)
            self.assertEqual(
                [command[1] for command in calls], ["create", "start", "rm"] * 2
            )

    def test_container_failure_and_timeout_still_remove_the_container(self):
        for timeout in (False, True):
            with self.subTest(timeout=timeout), tempfile.TemporaryDirectory() as temporary:
                calls = []

                def docker(command, **_options):
                    calls.append(command)
                    if command[1] == "start":
                        if timeout:
                            raise subprocess.TimeoutExpired(command, 60)
                        return subprocess.CompletedProcess(command, 1, "", "fixture failed")
                    return subprocess.CompletedProcess(command, 0, "container-id\n", "")

                error = subprocess.TimeoutExpired if timeout else subprocess.CalledProcessError
                with self.assertRaises(error):
                    self.run_fixture(Path(temporary), docker)
                self.assertEqual(
                    [command[1] for command in calls], ["create", "start", "rm"]
                )

    def test_missing_coverage_profile_is_an_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            calls = []

            def docker(command, **_options):
                calls.append(command)
                return subprocess.CompletedProcess(command, 0, "container-id\n", "")

            with self.assertRaisesRegex(AssertionError, "no coverage profile"):
                self.run_fixture(root, docker, root / "agent-%p-%m.profraw")
            self.assertEqual(
                [command[1] for command in calls], ["create", "start", "rm"]
            )


if __name__ == "__main__":
    unittest.main()
