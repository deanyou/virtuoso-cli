"""Tests for gui_runner.py CLI."""
import json
import os
import shutil
import subprocess
import tempfile
import unittest
import sys
from pathlib import Path

SCRIPT_PATH = Path(__file__).parent.parent / "scripts" / "gui_runner.py"


def write_scenario(path, data):
    with open(path, "w") as f:
        json.dump(data, f)


def write_outcomes(path, outcomes_list):
    """Write a fake-outcomes JSON file."""
    with open(path, "w") as f:
        json.dump(outcomes_list, f)


def clean_env():
    """Return a sanitized environment for subprocess."""
    env = os.environ.copy()
    for key in ["VB_SESSION", "VB_PORT", "VCLI_REMOTE_HOST", "DISPLAY"]:
        env.pop(key, None)
    return env


def run_cli(*args, env=None):
    """Invoke the CLI and return the CompletedProcess."""
    if env is None:
        env = clean_env()
    return subprocess.run(
        ["python3", str(SCRIPT_PATH)] + list(args),
        capture_output=True,
        text=True,
        env=env,
    )


VALID_SCENARIO = {
    "version": "1.0",
    "task_id": "test-001",
    "session_id": "sess-abc",
    "pid": 12345,
    "display": ":0",
    "cellview": {"lib": "myLib", "cell": "myCell", "view": "schematic"},
    "steps": [{
        "id": "s1",
        "operation": "VCLI_LOAD",
        "arguments": {"command": "test"},
        "verifier": {"predicate": "window_exists", "expected": True},
        "timeout_seconds": 30,
        "max_retries": 0,
    }],
}


class TestCliValidate(unittest.TestCase):
    def test_validate_returns_zero_on_valid(self):
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            write_scenario(f.name, VALID_SCENARIO)
            tmp_path = f.name
        try:
            result = run_cli("validate", tmp_path)
            self.assertEqual(result.returncode, 0, f"stderr: {result.stderr}")
            output = json.loads(result.stdout)
            self.assertEqual(output["status"], "valid")
            self.assertEqual(output["task_id"], "test-001")
        finally:
            os.unlink(tmp_path)

    def test_validate_returns_two_on_invalid(self):
        data = dict(VALID_SCENARIO)
        data["session_id"] = ""
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            write_scenario(f.name, data)
            tmp_path = f.name
        try:
            result = run_cli("validate", tmp_path)
            self.assertEqual(result.returncode, 2, f"stdout: {result.stdout}")
            # JSON error must be on stdout (machine-readable contract)
            err_output = json.loads(result.stdout)
            self.assertEqual(err_output["status"], "invalid")
            self.assertIn("session_id", err_output["error"])
        finally:
            os.unlink(tmp_path)

    def test_validate_version_2_rejected(self):
        data = dict(VALID_SCENARIO)
        data["version"] = "2.0"
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            write_scenario(f.name, data)
            tmp_path = f.name
        try:
            result = run_cli("validate", tmp_path)
            self.assertEqual(result.returncode, 2)
            err_output = json.loads(result.stdout)
            self.assertEqual(err_output["status"], "invalid")
            self.assertIn("version", err_output["error"])
        finally:
            os.unlink(tmp_path)


class TestCliRunFake(unittest.TestCase):
    def test_run_fake_returns_zero_on_pass(self):
        staging = tempfile.mkdtemp()
        outdir = tempfile.mkdtemp()
        child_out = Path(outdir) / "run"
        try:
            with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False, dir=staging) as f:
                write_scenario(f.name, VALID_SCENARIO)
                tmp_path = f.name
            try:
                result = run_cli("run", tmp_path, "--output", str(child_out), "--executor", "fake")
                self.assertEqual(result.returncode, 0, f"stderr: {result.stderr}")
                output = json.loads(result.stdout)
                self.assertEqual(output["status"], "passed")
            finally:
                os.unlink(tmp_path)
        finally:
            shutil.rmtree(staging, ignore_errors=True)
            shutil.rmtree(outdir, ignore_errors=True)

    def test_run_fake_returns_one_on_failure(self):
        staging = tempfile.mkdtemp()
        outdir = tempfile.mkdtemp()
        child_out = Path(outdir) / "run"
        try:
            with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False, dir=staging) as f:
                write_scenario(f.name, VALID_SCENARIO)
                tmp_path = f.name

            outcomes_path = Path(staging) / "fail-outcomes.json"
            write_outcomes(outcomes_path, [
                {"step_id": "s1", "phase": "execute", "attempt": 0, "outcome": "FAILURE"}
            ])

            try:
                result = run_cli(
                    "run", tmp_path,
                    "--output", str(child_out),
                    "--executor", "fake",
                    "--fake-outcomes", str(outcomes_path),
                )
                self.assertEqual(result.returncode, 1, f"stderr: {result.stderr}")
                output = json.loads(result.stdout)
                self.assertEqual(output["status"], "failed")
            finally:
                os.unlink(tmp_path)
                os.unlink(outcomes_path)
        finally:
            shutil.rmtree(staging, ignore_errors=True)
            shutil.rmtree(outdir, ignore_errors=True)

    def test_run_refuses_non_fake_executor(self):
        staging = tempfile.mkdtemp()
        outdir = tempfile.mkdtemp()
        child_out = Path(outdir) / "run"
        try:
            with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False, dir=staging) as f:
                write_scenario(f.name, VALID_SCENARIO)
                tmp_path = f.name
            try:
                result = run_cli("run", tmp_path, "--output", str(child_out), "--executor", "real")
                self.assertEqual(result.returncode, 2, f"stdout: {result.stdout}")
                err_output = json.loads(result.stdout)
                self.assertEqual(err_output["status"], "error")
                self.assertIn("not supported", err_output["error"])
            finally:
                os.unlink(tmp_path)
        finally:
            shutil.rmtree(staging, ignore_errors=True)
            shutil.rmtree(outdir, ignore_errors=True)

    def test_run_returns_two_on_invalid_scenario(self):
        staging = tempfile.mkdtemp()
        outdir = tempfile.mkdtemp()
        child_out = Path(outdir) / "run"
        try:
            data = dict(VALID_SCENARIO)
            data["session_id"] = ""
            with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False, dir=staging) as f:
                write_scenario(f.name, data)
                tmp_path = f.name
            try:
                result = run_cli("run", tmp_path, "--output", str(child_out), "--executor", "fake")
                self.assertEqual(result.returncode, 2, f"stdout: {result.stdout}")
                err_output = json.loads(result.stdout)
                self.assertEqual(err_output["status"], "invalid")
            finally:
                os.unlink(tmp_path)
        finally:
            shutil.rmtree(staging, ignore_errors=True)
            shutil.rmtree(outdir, ignore_errors=True)

    def test_run_unknown_scenario_file(self):
        outdir = tempfile.mkdtemp()
        child_out = Path(outdir) / "run"
        try:
            result = run_cli("run", "/nonexistent/path.json", "--output", str(child_out), "--executor", "fake")
            self.assertEqual(result.returncode, 2)
            err_output = json.loads(result.stdout)
            self.assertEqual(err_output["status"], "error")
        finally:
            shutil.rmtree(outdir, ignore_errors=True)


class TestCliRunLive(unittest.TestCase):
    """--executor live wiring: argument validation, no subprocess side effects."""

    def _run_live(self, tmp_path, *extra):
        outdir = tempfile.mkdtemp()
        child_out = Path(outdir) / "run"
        try:
            result = run_cli(
                "run", tmp_path, "--output", str(child_out),
                "--executor", "live", *extra,
            )
            return result, child_out
        finally:
            shutil.rmtree(outdir, ignore_errors=True)

    def _write_scenario(self, session_id="sess-abc"):
        staging = tempfile.mkdtemp()
        data = dict(VALID_SCENARIO)
        data["session_id"] = session_id
        f = tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False, dir=staging
        )
        write_scenario(f.name, data)
        return staging, f.name

    def test_live_requires_session(self):
        staging, tmp_path = self._write_scenario()
        try:
            result, _ = self._run_live(tmp_path, "--vcli", "/usr/bin/vcli")
            self.assertEqual(result.returncode, 2)
            err = json.loads(result.stdout)
            self.assertEqual(err["status"], "error")
            self.assertIn("--session", err["error"])
        finally:
            os.unlink(tmp_path)
            shutil.rmtree(staging, ignore_errors=True)

    def test_live_requires_vcli(self):
        staging, tmp_path = self._write_scenario()
        try:
            result, _ = self._run_live(tmp_path, "--session", "sess-abc")
            self.assertEqual(result.returncode, 2)
            err = json.loads(result.stdout)
            self.assertIn("--vcli", err["error"])
        finally:
            os.unlink(tmp_path)
            shutil.rmtree(staging, ignore_errors=True)

    def test_live_rejects_scenario_session_mismatch(self):
        staging, tmp_path = self._write_scenario(session_id="sess-other")
        try:
            result, _ = self._run_live(
                tmp_path, "--session", "sess-abc", "--vcli", "/usr/bin/vcli"
            )
            self.assertEqual(result.returncode, 2)
            err = json.loads(result.stdout)
            self.assertIn("does not match", err["error"])
        finally:
            os.unlink(tmp_path)
            shutil.rmtree(staging, ignore_errors=True)

    def test_live_rejects_unsafe_ssh_host(self):
        staging, tmp_path = self._write_scenario()
        try:
            result, _ = self._run_live(
                tmp_path,
                "--session", "sess-abc",
                "--vcli", "/usr/bin/vcli",
                "--ssh-host", "bad host\nrm -rf /",
            )
            self.assertEqual(result.returncode, 2)
            err = json.loads(result.stdout)
            self.assertEqual(err["status"], "error")
        finally:
            os.unlink(tmp_path)
            shutil.rmtree(staging, ignore_errors=True)

    def test_live_fails_closed_on_missing_session_query(self):
        # vcli points at a binary that reports no sessions; the run must
        # fail closed with a precheck error, never send GUI input.
        staging, tmp_path = self._write_scenario()
        stub = Path(staging) / "vcli-stub"
        stub.write_text(
            "#!/bin/sh\n"
            "echo '{\"status\":\"success\",\"count\":0,\"sessions\":[]}'\n"
        )
        stub.chmod(0o755)
        try:
            result, out = self._run_live(
                tmp_path,
                "--session", "sess-abc",
                "--vcli", str(stub),
            )
            self.assertEqual(result.returncode, 1)
            summary = json.loads(result.stdout)
            self.assertEqual(summary["status"], "failed")
            self.assertEqual(summary["error_code"], "PRECHECK_ERROR")
        finally:
            os.unlink(tmp_path)
            shutil.rmtree(staging, ignore_errors=True)


class TestCliMachineReadableOutput(unittest.TestCase):
    def test_stdout_is_valid_json_on_pass(self):
        staging = tempfile.mkdtemp()
        outdir = tempfile.mkdtemp()
        child_out = Path(outdir) / "run"
        try:
            with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False, dir=staging) as f:
                write_scenario(f.name, VALID_SCENARIO)
                tmp_path = f.name
            try:
                result = run_cli("run", tmp_path, "--output", str(child_out), "--executor", "fake")
                output = json.loads(result.stdout)
                self.assertIn("status", output)
                self.assertEqual(output["status"], "passed")
            finally:
                os.unlink(tmp_path)
        finally:
            shutil.rmtree(staging, ignore_errors=True)
            shutil.rmtree(outdir, ignore_errors=True)

    def test_stdout_is_valid_json_on_invalid(self):
        data = dict(VALID_SCENARIO)
        data["version"] = "2.0"
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as f:
            write_scenario(f.name, data)
            tmp_path = f.name
        try:
            result = run_cli("validate", tmp_path)
            err_output = json.loads(result.stdout)
            self.assertEqual(err_output["status"], "invalid")
        finally:
            os.unlink(tmp_path)


class TestNoOutputOutsideDir(unittest.TestCase):
    def test_no_files_outside_output_dir(self):
        staging = tempfile.mkdtemp()
        outdir = tempfile.mkdtemp()
        child_out = Path(outdir) / "run"
        try:
            with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False, dir=staging) as f:
                write_scenario(f.name, VALID_SCENARIO)
                tmp_path = f.name
            run_cli("run", tmp_path, "--output", str(child_out), "--executor", "fake")
            entries = os.listdir(staging)
            other_files = [e for e in entries if not e.endswith(".json")]
            self.assertEqual(other_files, [], f"Unexpected files in staging: {other_files}")
            os.unlink(tmp_path)
        finally:
            shutil.rmtree(staging, ignore_errors=True)
            shutil.rmtree(outdir, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()