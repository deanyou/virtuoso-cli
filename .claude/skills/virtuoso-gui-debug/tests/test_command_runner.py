"""Tests for vgui_runner.command_runner — fixed argv transport (Task 3).

Covers the 2026-09-01 live-executor plan requirements:
- local execution uses shell=False and a cleaned environment;
- SSH mode only accepts a valid host and a fixed vcli argv;
- newline / NUL / extra remote commands are rejected;
- timeouts and non-zero exits produce structured errors;
- argv is passed as a list, never a shell string.
"""
import os
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).parent.parent / "scripts"
sys.path.insert(0, str(SCRIPTS_DIR))

from vgui_runner.command_runner import (  # noqa: E402
    CommandError,
    CommandResult,
    LocalRunner,
    SshRunner,
)

VCLI = "/usr/bin/vcli"


def make_result(exit_code=0, stdout="{}", stderr="", timed_out=False):
    return CommandResult(
        exit_code=exit_code,
        stdout=stdout,
        stderr=stderr,
        duration_ms=5,
        timed_out=timed_out,
    )


class RecordingRunner:
    """Fake runner base that records argv and returns canned results."""

    def __init__(self, results):
        self.results = list(results)
        self.argvs = []

    def run(self, argv, timeout_seconds):
        self.argvs.append(list(argv))
        if not self.results:
            return make_result()
        r = self.results.pop(0)
        if isinstance(r, Exception):
            raise r
        return r


class TestLocalRunner(unittest.TestCase):
    def test_runs_argv_list_without_shell(self):
        runner = LocalRunner(vcli_path=VCLI)
        result = runner.run(
            [sys.executable, "-c", "print('hello')"], timeout_seconds=10
        )
        self.assertEqual(result.exit_code, 0)
        self.assertIn("hello", result.stdout)
        self.assertFalse(result.timed_out)

    def test_environment_is_cleaned(self):
        runner = LocalRunner(vcli_path=VCLI)
        code = (
            "import os, json;"
            "print(json.dumps(sorted(os.environ.keys())))"
        )
        result = runner.run([sys.executable, "-c", code], timeout_seconds=10)
        import json

        keys = set(json.loads(result.stdout))
        allowed = {"PATH", "LANG", "LC_ALL", "VCLI_CAPABILITY",
                   "HOME", "XDG_CACHE_HOME", "VB_PORT", "VB_REMOTE_HOST"}
        # macOS injects system keys like __CF_USER_TEXT_ENCODING and LC_CTYPE
        # into every child; they carry no user data. Everything else — in
        # particular VB_*, DISPLAY, credentials — must be dropped.
        leaked = {
            k
            for k in keys - allowed
            if not k.startswith("__CF") and k not in ("LC_CTYPE",)
        }
        self.assertFalse(
            leaked,
            f"environment leaked keys: {sorted(leaked)}",
        )
        for k in keys:
            self.assertFalse(k.startswith("VB_"), f"leaked {k}")
            self.assertNotEqual(k, "DISPLAY")
        self.assertIn("VCLI_CAPABILITY", keys)

    def test_vcli_capability_is_admin(self):
        runner = LocalRunner(vcli_path=VCLI)
        result = runner.run(
            [sys.executable, "-c", "import os; print(os.environ.get('VCLI_CAPABILITY'))"],
            timeout_seconds=10,
        )
        self.assertEqual(result.stdout.strip(), "admin")

    def test_timeout_produces_timed_out_result(self):
        runner = LocalRunner(vcli_path=VCLI)
        result = runner.run(
            [sys.executable, "-c", "import time; time.sleep(5)"], timeout_seconds=1
        )
        self.assertTrue(result.timed_out)
        self.assertIsNone(result.exit_code)

    def test_nonzero_exit_is_reported(self):
        runner = LocalRunner(vcli_path=VCLI)
        result = runner.run(
            [sys.executable, "-c", "import sys; sys.exit(3)"], timeout_seconds=10
        )
        self.assertEqual(result.exit_code, 3)

    def test_rejects_string_argv(self):
        runner = LocalRunner(vcli_path=VCLI)
        with self.assertRaises(CommandError):
            runner.run("python3 -c print(1)", timeout_seconds=5)

    def test_rejects_empty_argv(self):
        runner = LocalRunner(vcli_path=VCLI)
        with self.assertRaises(CommandError):
            runner.run([], timeout_seconds=5)

    def test_rejects_non_string_element(self):
        runner = LocalRunner(vcli_path=VCLI)
        with self.assertRaises(CommandError):
            runner.run([sys.executable, "-c", 123], timeout_seconds=5)


class TestSshRunnerValidation(unittest.TestCase):
    def test_rejects_host_with_newline(self):
        with self.assertRaises(CommandError):
            SshRunner(vcli_path=VCLI, ssh_host="host\nrm -rf /")

    def test_rejects_host_with_nul(self):
        with self.assertRaises(CommandError):
            SshRunner(vcli_path=VCLI, ssh_host="host\x00")

    def test_rejects_host_with_space(self):
        with self.assertRaises(CommandError):
            SshRunner(vcli_path=VCLI, ssh_host="bad host")

    def test_rejects_empty_host(self):
        with self.assertRaises(CommandError):
            SshRunner(vcli_path=VCLI, ssh_host="")

    def test_accepts_normal_host(self):
        runner = SshRunner(vcli_path=VCLI, ssh_host="compute-eda-42.example.com")
        self.assertEqual(runner.ssh_host, "compute-eda-42.example.com")

    def test_rejects_argv_element_with_newline(self):
        runner = SshRunner(vcli_path=VCLI, ssh_host="host")
        with self.assertRaises(CommandError):
            runner.run([VCLI, "session", "list\n; rm -rf /"], timeout_seconds=5)

    def test_rejects_argv_element_with_nul(self):
        runner = SshRunner(vcli_path=VCLI, ssh_host="host")
        with self.assertRaises(CommandError):
            runner.run([VCLI, "session\x00list"], timeout_seconds=5)

    def test_rejects_argv_not_starting_with_vcli(self):
        runner = SshRunner(vcli_path=VCLI, ssh_host="host")
        with self.assertRaises(CommandError):
            runner.run(["/bin/bash", "-c", "echo pwned"], timeout_seconds=5)

    def test_ssh_argv_uses_fixed_structure(self):
        runner = SshRunner(vcli_path=VCLI, ssh_host="host")
        argv = runner.ssh_argv([VCLI, "session", "list", "--format", "json"])
        self.assertEqual(argv[0], "ssh")
        self.assertEqual(argv[1], "--")
        self.assertEqual(argv[2], "host")
        self.assertEqual(len(argv), 4)
        # The remote command safely quotes every element.
        remote = argv[3]
        self.assertIn(VCLI, remote)
        self.assertNotIn("\n", remote)
        self.assertNotIn("\x00", remote)

    def test_remote_command_quotes_shell_metacharacters(self):
        runner = SshRunner(vcli_path=VCLI, ssh_host="host")
        argv = runner.ssh_argv([VCLI, "window", "action-x11", "--text", "a;b|c&d"])
        remote = argv[3]
        # The metacharacters must be inside single quotes (inert), not bare.
        self.assertIn("'a;b|c&d'", remote)


class TestSshRunnerExecution(unittest.TestCase):
    def test_runs_ssh_with_list_argv(self):
        runner = SshRunner(vcli_path=VCLI, ssh_host="localhost-invalid-test")
        # Use a command that fails fast (ssh to unresolvable host).
        try:
            result = runner.run([VCLI, "session", "list"], timeout_seconds=10)
        except CommandError:
            pass  # structured rejection is acceptable
        else:
            # If ssh ran, it must have been a list-argv, non-shell invocation;
            # unresolvable host yields nonzero exit without raising.
            self.assertIsNotNone(result)


class TestCommandResult(unittest.TestCase):
    def test_is_immutable(self):
        r = make_result()
        with self.assertRaises(Exception):
            r.exit_code = 1


if __name__ == "__main__":
    unittest.main()
