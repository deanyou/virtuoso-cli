#!/usr/bin/env python3
"""
Tests for the command builder and the runner.

The point of these tests is the argument boundary: a brief is data, and a
value in it may carry shell syntax that came from somewhere else — a cell
name in a colleague's ticket, a design variable in a spreadsheet.  Such a
value has to reach vcli as one argument, never as a second command.

The tests drive a fake `vcli` on PATH that records the argv it received, so
they are offline, fast, and exact.
"""

import json
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent.parent / 'scripts'))

import run
from vcli_cmd import build_argv


# Payloads that break a shell, each with its own way in.
INJECTIONS = [
    ("semicolon", "top;id"),
    ("command substitution", "top$(id)"),
    ("backtick", "top`id`"),
    ("double quote breakout", 'top" ; id ; echo "'),
    ("single quote breakout", "top' ; id ; echo '"),
    ("pipe", "top|id"),
    ("newline", "top\nid"),
    ("redirect", "top>/tmp/vcli_should_not_exist"),
]


class TestBuildArgv(unittest.TestCase):
    """The argv builder never concatenates a command line."""

    def test_plain_arguments(self):
        argv = build_argv("cell.open", {"lib": "mylib", "cell": "mycell"})
        self.assertEqual(argv, ["vcli", "cell", "open", "--lib", "mylib", "--cell", "mycell"])

    def test_numeric_argument_is_one_element(self):
        argv = build_argv("maestro.run", {"timeout": 30})
        self.assertEqual(argv[-2:], ["--timeout", "30"])

    def test_boolean_true_contributes_only_the_flag(self):
        argv = build_argv("maestro.run", {"force": True})
        self.assertEqual(argv[-1], "--force")

    def test_boolean_false_contributes_nothing(self):
        argv = build_argv("maestro.run", {"force": False})
        self.assertNotIn("--force", argv)

    def test_every_payload_stays_one_argument(self):
        """The value must be a single argv element, whatever it contains."""
        for label, payload in INJECTIONS:
            with self.subTest(payload=label):
                argv = build_argv("cell.open", {"lib": payload})
                self.assertEqual(len(argv), 5, f"{label}: {argv}")
                self.assertEqual(argv[3:], ["--lib", payload])

    def test_unknown_action_becomes_words_not_shell(self):
        argv = build_argv("bogus.action", {"lib": "x"})
        self.assertEqual(argv[:3], ["vcli", "bogus", "action"])


class TestRunDoesNotUseAShell(unittest.TestCase):
    """The runner hands the argv to the OS, not to a shell."""

    def test_subprocess_is_called_without_a_shell(self):
        seen = {}
        real_run = run.subprocess.run

        def spy(argv, **kwargs):
            seen["argv"] = argv
            seen["kwargs"] = kwargs
            return real_run(["true"], capture_output=True, text=True)

        run.subprocess.run = spy
        try:
            run.run_vcli_command(["vcli", "cell", "open", "--lib", "x;id"])
        finally:
            run.subprocess.run = real_run

        self.assertFalse(seen["kwargs"].get("shell", False),
                         "the runner must not use a shell")
        self.assertEqual(seen["argv"], ["vcli", "cell", "open", "--lib", "x;id"])


class TestInjectedCommandDoesNotRun(unittest.TestCase):
    """End to end: a brief whose value carries shell syntax runs nothing extra."""

    def _fake_vcli(self, tmp: Path, record: Path) -> Path:
        """Put a fake `vcli` on PATH that records its argv."""
        bindir = tmp / "bin"
        bindir.mkdir()
        script = bindir / "vcli"
        # NUL-separated, so an argument that contains a newline survives
        # the round trip intact.
        script.write_text(
            "#!/bin/sh\n"
            f'printf "%s\\000" "$@" > {record}\n'
            "exit 0\n"
        )
        script.chmod(script.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
        return bindir

    def _execute(self, payload):
        tmp = Path(tempfile.mkdtemp())
        record = tmp / "argv.txt"
        bindir = self._fake_vcli(tmp, record)

        brief = {
            "id": "test-injection",
            "requirement": {
                "commands": [
                    {"action": "cell.open", "args": {"lib": payload, "cell": "top"}}
                ]
            },
        }
        brief_path = tmp / "brief.json"
        brief_path.write_text(json.dumps(brief))

        env_path = f"{bindir}{os.pathsep}{os.environ.get('PATH', '')}"
        old_path = os.environ.get("PATH")
        os.environ["PATH"] = env_path
        try:
            run.execute_brief(brief)
        finally:
            if old_path is None:
                os.environ.pop("PATH", None)
            else:
                os.environ["PATH"] = old_path

        received = []
        if record.exists():
            received = [
                part.decode()
                for part in record.read_bytes().split(b"\x00")
                if part
            ]
        return received, tmp

    def test_shell_syntax_in_a_value_is_not_executed(self):
        """The fake vcli must receive the payload as one argument, and the
        commands the payload names must never run."""
        canary = Path(tempfile.gettempdir()) / "vcli_injection_canary"
        if canary.exists():
            canary.unlink()

        for label, payload in INJECTIONS + [
            ("touch canary", f"top; touch {canary}"),
            ("substitution canary", f"top$(touch {canary})"),
        ]:
            with self.subTest(payload=label):
                received, _tmp = self._execute(payload)

                self.assertIn("--lib", received, f"{label}: {received!r}")
                at = received.index("--lib") + 1
                self.assertEqual(
                    received[at], payload,
                    f"{label}: the payload must arrive as one argument, got {received!r}",
                )
                self.assertEqual(
                    len(received), 6,
                    f"{label}: the payload must not split the argument list, got {received!r}",
                )
                self.assertFalse(
                    canary.exists(),
                    f"{label}: the injected command executed",
                )
                if canary.exists():
                    canary.unlink()

    def test_a_redirect_in_a_value_writes_no_file(self):
        target = Path(tempfile.gettempdir()) / "vcli_should_not_exist"
        if target.exists():
            target.unlink()
        self._execute(f"top>{target}")
        self.assertFalse(target.exists(),
                         "a redirect inside a value must not create a file")


if __name__ == "__main__":
    unittest.main()
