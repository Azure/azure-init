# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

"""Test compile-time version choices without changing the production function."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
SHA = "0123456789abcdef0123456789abcdef01234567"
GIT_SHIM = """#!/usr/bin/python3
import os, sys
if sys.argv[1:] == ["describe", "--dirty", "--tags"]:
    sys.exit(1)
if sys.argv[1:] == ["rev-parse", "HEAD"]:
    sha = os.environ.get("TEST_BUILD_SHA")
    if sha:
        print(sha)
        sys.exit(0)
    sys.exit(1)
os.execv(os.environ["TEST_REAL_GIT"], ["git", *sys.argv[1:]])
"""


def main():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"],
        cwd=ROOT, text=True,
    ))
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "azure-init")
    real_git = shutil.which("git")
    if real_git is None:
        raise RuntimeError("git is required for version coverage tests")

    for variant, expected in (
        ("override", "coverage-override"),
        ("sha", f"{version}-{SHA[:7]}"),
        ("package", version),
    ):
        with tempfile.TemporaryDirectory(prefix=f"azure-init-version-{variant}-") as temporary:
            directory = Path(temporary)
            environment = dict(os.environ)
            for variable in (
                "AZURE_INIT_VERSION", "AZURE_INIT_BUILD_VERSION",
                "AZURE_INIT_BUILD_SHA", "LLVM_PROFILE_FILE",
            ):
                environment.pop(variable, None)
            environment["CARGO_LLVM_COV_TARGET_DIR"] = str(directory / "target")
            if variant == "override":
                environment["AZURE_INIT_VERSION"] = expected
            else:
                git = directory / "git"
                git.write_text(GIT_SHIM)
                git.chmod(0o755)
                environment.update(
                    PATH=f"{directory}{os.pathsep}{environment['PATH']}",
                    TEST_REAL_GIT=real_git,
                    TEST_BUILD_SHA=SHA if variant == "sha" else "",
                )
            result = subprocess.run(
                ["cargo", "llvm-cov", "run", "--locked", "--all-features",
                 "--package", "azure-init", "--bin", "azure-init", "--no-report",
                 "--", "--version"],
                cwd=ROOT, env=environment, check=True, text=True,
                stdout=subprocess.PIPE, timeout=600,
            )
            assert result.stdout == f"azure-init {expected}\n", result.stdout
            report = ROOT / "target" / f"version-{variant}.lcov"
            report.parent.mkdir(exist_ok=True)
            subprocess.run(
                ["cargo", "llvm-cov", "report", "--package", "azure-init",
                 "--lcov", "--output-path", str(report)],
                cwd=ROOT, env=environment, check=True, timeout=60,
            )
            print(f"Verified {variant}: {expected}", flush=True)


if __name__ == "__main__":
    main()
