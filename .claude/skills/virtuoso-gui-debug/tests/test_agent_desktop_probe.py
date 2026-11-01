"""Probe contract: no desktop mutation, no raw UI content in evidence."""

import importlib.util
import json
import subprocess
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "scripts" / "probe_agent_desktop.py"
SPEC = importlib.util.spec_from_file_location("probe_agent_desktop", SCRIPT)
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)


def result(ok, command, data=None, error=None, returncode=None):
    envelope = {"version": "2.5", "ok": ok, "command": command}
    envelope["data" if ok else "error"] = data if ok else error
    return subprocess.CompletedProcess([], 0 if ok else 1 if returncode is None else returncode,
                                       json.dumps(envelope), "")


class ProbeTests(unittest.TestCase):
    def test_reports_only_aggregates_and_uses_read_only_commands(self):
        calls = []
        private = "private cellview name"

        def run(argv, **kwargs):
            calls.append(argv)
            self.assertFalse(kwargs.get("shell", False))
            if argv[1] == "status":
                return result(True, "status", {"supported_surfaces": ["window"]})
            return result(True, "snapshot", {"complete": True, "tree": {
                "role": "window", "name": private, "children": [
                    {"role": "button", "name": private, "ref_id": "@s1:e1",
                     "available_actions": ["Click"]}]}})

        report = PROBE.probe("virtuoso", runner=run)
        self.assertEqual(report["availability"], "observed")
        self.assertEqual(report["actionable_count"], 1)
        self.assertNotIn(private, json.dumps(report))
        self.assertNotIn("@s1:e1", json.dumps(report))
        self.assertEqual(calls, [["agent-desktop", "status"],
                                 ["agent-desktop", "snapshot", "--app", "virtuoso", "-i", "--compact"]])

    def test_partial_snapshot_is_not_availability_proof(self):
        responses = iter([result(True, "status", {}), result(True, "snapshot", {
            "complete": False, "tree": {"role": "window", "ref_id": "@s1:e1"}})])
        report = PROBE.probe("virtuoso", runner=lambda *a, **k: next(responses))
        self.assertEqual(report["availability"], "partial")

    def test_structured_error_never_exposes_desktop_details_or_retries(self):
        calls = []

        def run(argv, **kwargs):
            calls.append(argv)
            if len(calls) == 1:
                return result(True, "status", {})
            return result(False, "snapshot", error={"code": "APP_NOT_FOUND",
                       "message": "private window title", "suggestion": "try another app"})

        report = PROBE.probe("virtuoso", runner=run)
        self.assertEqual(report["availability"], "unavailable")
        self.assertEqual(report["reason_code"], "snapshot_APP_NOT_FOUND")
        self.assertNotIn("private", json.dumps(report))
        self.assertEqual(len(calls), 2)

    def test_unavailable_binary_is_explicit(self):
        def missing(*args, **kwargs):
            raise FileNotFoundError("agent-desktop")

        report = PROBE.probe("virtuoso", runner=missing)
        self.assertEqual(report["reason_code"], "binary_missing")


if __name__ == "__main__":
    unittest.main()
