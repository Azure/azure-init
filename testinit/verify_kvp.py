"""Exercise the installed KVP CLI in scratch pools during testinit CI."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import uuid


KVP = "/usr/bin/libazureinit-kvp"
VM_ID = "3f2504e0-4f89-41d3-9a0c-0305e82c3301"
REPORT_KEY = "PROVISIONING_REPORT"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def cli(directory, *arguments, expected=0, stdin=None):
    command = [KVP, "--dir", str(directory), *map(str, arguments)]
    result = subprocess.run(
        command, input=stdin, text=True, capture_output=True, timeout=30
    )
    require(
        result.returncode == expected,
        f"{command!r}: expected exit {expected}, got {result.returncode}\n"
        f"stdout: {result.stdout}\nstderr: {result.stderr}",
    )
    if expected in (0, 1):
        require(not result.stderr, f"unexpected CLI stderr: {result.stderr}")
    return result.stdout


def cli_json(directory, *arguments, **options):
    return json.loads(cli(directory, "--json", *arguments, **options))


def verify_cli():
    with tempfile.TemporaryDirectory(prefix="testinit-cli-") as temporary:
        directory = Path(temporary)
        commands = (
            "info", "dump", "entries", "read", "write", "emit", "load",
            "append-multiple", "delete", "delete-multiple", "clear", "is-stale",
            "report-success", "report-failure",
        )
        help_text = cli(directory, "--help")
        for command in commands:
            require(command in help_text, f"missing command in help: {command}")
            cli(directory, command, "--help")

        info = cli_json(directory, "info")
        require(info["empty"] and info["records"] == 0, "new pool is not empty")
        require(info["max_key_size"] == 254, "unexpected safe key limit")
        require(info["max_value_size"] == 1022, "unexpected safe value limit")
        require("pool=guest" in cli(directory, "info"), "missing text metadata")
        cli(directory, "write", "z", "first")
        cli(directory, "write", "--append", "z", "second")
        cli(directory, "write", "a", "other")
        require(cli(directory, "read", "z") == "second\n", "read is not last-value-wins")
        require(
            json.loads(cli(directory, "dump")) == [
                {"key": "z", "value": "first"},
                {"key": "z", "value": "second"},
                {"key": "a", "value": "other"},
            ],
            "dump lost duplicates or physical order",
        )
        require(cli(directory, "entries") == "a=other\nz=second\n", "entries not sorted")
        cli(directory, "write", "z", "last")
        require(len(cli_json(directory, "dump")) == 2, "upsert did not collapse duplicates")
        value = 'url=https://example.invalid/?x=1\n"quoted"|value'
        cli(directory, "write", "a", value)
        require(cli_json(directory, "read", "a")["value"] == value, "JSON value changed")
        require(cli(directory, "read", "missing", expected=1) == "", "missing read has output")
        require(cli_json(directory, "delete", "a") == {"removed": True}, "delete failed")
        require(cli(directory, "delete", "a") == "false\n", "missing delete changed status")
        require(cli(directory, "delete-multiple", "z", "missing") == "1\n", "wrong delete count")

        source = directory / "records.txt"
        source.write_text("b=two=parts\nc=three\n", encoding="utf-8")
        cli(directory, "write", "obsolete", "value")
        cli(directory, "load", "--file", source)
        require(cli(directory, "dump", "--text") == source.read_text(), "load did not replace")
        cli(directory, "append-multiple", stdin="b=new\nd=four\n")
        cli(directory, "append-multiple", "--file", source)
        require(cli_json(directory, "entries") == {"b": "two=parts", "c": "three", "d": "four"}, "batch values differ")
        require(cli_json(directory, "delete-multiple", "b", "c") == {"removed": 5}, "batch delete count differs")
        cli(directory, "load", stdin="x=one\n")
        require(cli_json(directory, "entries") == {"x": "one"}, "stdin load failed")
        cli(directory, "clear")
        require(cli_json(directory, "dump") == [], "clear failed")

        cli(directory, "write", "", "invalid", expected=2)
        cli(directory, "write", "k" * 255, "invalid", expected=2)
        cli(directory, "--unsafe", "write", "k" * 255, "full-width")
        require(cli_json(directory, "--unsafe", "info")["max_key_size"] == 512, "unsafe mode not applied")
        cli(directory, "--json", "dump", "--text", expected=2)
        cli(directory, "dump", "--name", "missing-parse", expected=2)
        cli(directory, "emit", "--name", "invalid", "--message", "value", "--vm-id", "invalid", expected=2)
        cli(directory, "clear")

        pools = ("external", "guest", "auto", "auto-external", "auto-internal")
        for index, pool in enumerate(pools):
            cli(directory, "--pool", pool, "write", "pool", pool)
            require(cli(directory, "--pool", str(index), "read", "pool") == f"{pool}\n", "pool selection differs")
            require((directory / f".kvp_pool_{index}").exists(), "pool file missing")
        cli(directory, "clear")

        payload = "\u20ac" * 1023
        cli(directory, "emit", "--name", "artifact", "--message", payload, "--vm-id", VM_ID, "--agent", "testinit/1")
        raw = cli_json(directory, "dump")
        require(len(raw) > 1, "long event was not chunked")
        for record in raw:
            require(len(record["key"].encode()) <= 254, "oversized diagnostic key")
            require(len(record["value"].encode()) <= 1022, "oversized diagnostic chunk")
        event = cli_json(directory, "dump", "--parse")[0]
        require(event["type"] == "diagnostic" and event["payload"] == payload, "event did not roundtrip")
        require(event["vm_id"] == VM_ID and event["agent"] == "testinit/1", "event identity differs")
        uuid.UUID(event["event_id"])
        cli(directory, "write", "raw-note", "preserved")
        cli(directory, "report-success", "--vm-id", VM_ID, "--supporting-data", "detail='left,right',empty=")
        report = next(entry for entry in cli_json(directory, "dump", "--parse") if entry["type"] == REPORT_KEY)
        require(report["result"] == "success", "success report missing")
        require(report["extra"] == [["detail", "left,right"], ["empty", ""]], "supporting data changed")
        reason = 'failed | "quoted"\nnext line'
        cli(directory, "report-failure", "--vm-id", VM_ID, "--agent", "testinit/1", "--reason", reason, "--documentation-url", "https://example.invalid/help")
        parsed = cli_json(directory, "dump", "--parse", "--name", "artifact", "--kind", "event")
        reports = [entry for entry in parsed if entry["type"] == REPORT_KEY]
        require(len(reports) == 1 and reports[0]["result"] == "error", "report was not replaced")
        require(reports[0]["reason"] == reason, "report quoting changed")
        require(reports[0]["documentation_url"] == "https://example.invalid/help", "help URL changed")
        require(any(entry["type"] == "raw" for entry in parsed), "parsed filter lost raw data")
        filtered = cli_json(directory, "dump", "--parse", "--kind", "start")
        require(len(filtered) == 2, "kind filter must keep only the report and raw record")
        filtered = cli_json(directory, "dump", "--parse", "--name", "absent")
        require(len(filtered) == 2, "name filter must keep the report and raw record")
        require("diagnostic kind=event" in cli(directory, "dump", "--parse", "--text"), "parsed text output missing")
        cli(directory, "report-failure", "--vm-id", VM_ID, "--reason", "invalid", "--supporting-data", "bad", expected=2)

        stale_dir = directory / "stale"
        stale_dir.mkdir()
        cli(stale_dir, "clear", "--if-stale")
        require(cli_json(stale_dir, "is-stale", expected=1) == {"stale": False}, "missing pool is stale")
        cli(stale_dir, "write", "keep", "value")
        pool_file = stale_dir / ".kvp_pool_1"
        boot = next(int(line.split()[1]) for line in Path("/proc/stat").read_text().splitlines() if line.startswith("btime "))
        os.utime(pool_file, (boot + 60, boot + 60))
        cli(stale_dir, "clear", "--if-stale")
        require(cli(stale_dir, "is-stale", expected=1) == "false\n", "fresh pool reported stale")
        require(cli(stale_dir, "read", "keep") == "value\n", "fresh pool cleared")
        os.utime(pool_file, (boot - 1, boot - 1))
        require(cli(stale_dir, "is-stale") == "true\n", "stale pool not detected")
        cli(stale_dir, "clear", "--if-stale")
        require(cli_json(stale_dir, "dump") == [], "stale pool not cleared")
        pool_file.write_bytes(b"invalid framing")
        cli(stale_dir, "dump", expected=3)
        cli(directory / "missing-directory", "write", "key", "value", expected=3)
        print(f"Verified {len(commands)} KVP CLI commands and exit codes 0, 1, 2, 3", flush=True)


if __name__ == "__main__":
    verify_cli()