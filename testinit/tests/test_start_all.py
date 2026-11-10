import os
from pathlib import Path
import subprocess
import tempfile
import unittest


START_ALL = Path(__file__).resolve().parents[1] / "start-all.sh"
DOCKER_STUB = r"""#!/bin/bash
printf '%s|%s|%s\n' "${PWD##*/}" "${BASE_IMAGE:-}" "$*" >> "$TESTINIT_CALLS"
if [[ $1 == compose ]]; then
  if [[ ${TESTINIT_FAIL_PROJECT:-} == "${PWD##*/}" ]]; then
    echo "Compose startup failed" >&2
    exit 17
  fi
elif [[ $1 == exec ]]; then
  if [[ ${TESTINIT_FAIL_EXEC:-} == true ]]; then
    echo "No such container: azureinit-provisioning-agent" >&2
    exit 23
  fi
  if [[ ${TESTINIT_DELAY_SERVICE:-} == true ]] && [[ $(grep -c '|exec ' "$TESTINIT_CALLS") -eq 1 ]]; then
    echo "Starting azure-init.service"
  else
    echo "${TESTINIT_SERVICE_LOG:-Finished azure-init.service.}"
  fi
else
  echo "Unexpected Docker command: $*" >&2
  exit 99
fi
"""


class StartAllTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        self.workdir = root / "testinit"
        (self.workdir / "testing-server").mkdir(parents=True)
        self.bin_dir = root / "bin"
        self.bin_dir.mkdir()
        self.calls_path = root / "docker-calls"
        self.sleeps_path = root / "sleeps"
        self.write_tool("docker", DOCKER_STUB)
        self.write_tool("mkdir", '#!/bin/bash\n[[ "$*" == "-p /tmp/testinit" ]]\n')
        self.write_tool("sleep", '#!/bin/bash\necho sleep >> "$TESTINIT_SLEEPS"\n')

    def write_tool(self, name, contents):
        path = self.bin_dir / name
        path.write_text(contents)
        path.chmod(0o755)

    def run_start(self, *args, **overrides):
        self.calls_path.unlink(missing_ok=True)
        self.sleeps_path.unlink(missing_ok=True)
        env = os.environ.copy()
        env.pop("BASE_IMAGE", None)
        env.update(
            PATH=f"{self.bin_dir}:{env['PATH']}",
            TESTINIT_CALLS=str(self.calls_path),
            TESTINIT_SLEEPS=str(self.sleeps_path),
            **overrides,
        )
        result = subprocess.run(
            ["bash", str(START_ALL), *args],
            cwd=self.workdir,
            env=env,
            text=True,
            capture_output=True,
            timeout=5,
        )
        calls = (
            self.calls_path.read_text().splitlines()
            if self.calls_path.exists()
            else []
        )
        return result, calls

    def assert_no_wait(self, result):
        self.assertNotIn("Waiting for", result.stdout)
        self.assertFalse(self.sleeps_path.exists())

    def test_default_builds_both_images(self):
        result, calls = self.run_start()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls[0], "testing-server||compose up -d --build")
        self.assertEqual(calls[1], "testinit|ubuntu:24.04|compose up -d --build")
        self.assertEqual(len(calls), 3)
        self.assertIn("azure-init.service has finished", result.stdout)

    def test_custom_base_image(self):
        result, calls = self.run_start("ubuntu:26.04")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls[1], "testinit|ubuntu:26.04|compose up -d --build")

    def test_prebuilt_mode_never_builds_or_pulls(self):
        result, calls = self.run_start("debian:13", "--no-build")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            calls[0], "testing-server||compose up -d --no-build --pull never"
        )
        self.assertEqual(
            calls[1], "testinit|debian:13|compose up -d --no-build --pull never"
        )
        self.assertEqual(len(calls), 3)

    def test_compose_failure_stops_before_polling(self):
        for options in [(), ("ubuntu:24.04", "--no-build")]:
            for project, expected_calls in [("testing-server", 1), ("testinit", 2)]:
                with self.subTest(options=options, project=project):
                    result, calls = self.run_start(
                        *options, TESTINIT_FAIL_PROJECT=project
                    )
                    self.assertEqual(result.returncode, 17)
                    self.assertIn("Compose startup failed", result.stderr)
                    self.assertEqual(len(calls), expected_calls)
                    self.assert_no_wait(result)

    def test_missing_container_stops_without_waiting(self):
        result, calls = self.run_start(TESTINIT_FAIL_EXEC="true")
        self.assertEqual(result.returncode, 23)
        self.assertIn("No such container", result.stderr)
        self.assertEqual(len(calls), 3)
        self.assert_no_wait(result)

    def test_service_failure_returns_error(self):
        result, calls = self.run_start(
            TESTINIT_SERVICE_LOG="azure-init.service: Failed with result 'exit-code'."
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("azure-init.service has failed", result.stderr)
        self.assertEqual(len(calls), 3)
        self.assert_no_wait(result)

    def test_running_service_is_still_polled(self):
        result, calls = self.run_start(TESTINIT_DELAY_SERVICE="true")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 4)
        self.assertEqual(self.sleeps_path.read_text(), "sleep\n")
        self.assertIn("azure-init.service has finished", result.stdout)

    def test_invalid_options_fail_before_setup(self):
        for args in [
            ("ubuntu:24.04", "--invalid"),
            ("ubuntu:24.04", "--no-build", "extra"),
        ]:
            with self.subTest(args=args):
                result, calls = self.run_start(*args)
                self.assertEqual(result.returncode, 1)
                self.assertIn("Usage:", result.stderr)
                self.assertEqual(calls, [])
                self.assert_no_wait(result)


if __name__ == "__main__":
    unittest.main()
