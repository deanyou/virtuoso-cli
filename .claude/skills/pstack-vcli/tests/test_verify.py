"""CLI contract tests using temporary fake tools, never live EDA processes.

Fake-tool reports are test fixtures and are removed with their temporary roots.
They do not establish whether the real vcli build passes its verification gates.
"""

import ast
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest


VERIFY = Path(__file__).resolve().parents[1] / "scripts" / "verify.py"
SPEC = importlib.util.spec_from_file_location("pstack_vcli_verify", VERIFY)
VERIFY_MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY_MODULE)


@unittest.skipUnless(os.name == "posix", "fake executable harness uses POSIX shebangs")
class VerificationCliTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="pstack-verify-tests-")
        self.root = Path(self.temporary.name).resolve()
        # Quoting failures would execute the touch command in this directory.
        self.repo = self.root / "vcli ; touch INJECTED ; $(touch INJECTED2) ' space"
        (self.repo / "src").mkdir(parents=True)
        (self.repo / "Cargo.toml").write_text('[package]\nname = "virtuoso-cli"\n')
        (self.repo / "src" / "main.rs").write_text("fn main() {}\n")
        (self.repo / "AGENTS.md").write_text("fixture repository\n")
        self.tools = self.root / "tools with spaces"
        self.tools.mkdir()
        self.calls = self.root / "fixture-calls.jsonl"
        self.env = dict(os.environ)
        self.env["PATH"] = str(self.tools)
        self.env["PSTACK_TEST_CALLS"] = str(self.calls)
        self.output = self.root / "fixture-evidence"
        self.install_tool("cargo", """
            import json, os, sys
            with open(os.environ['PSTACK_TEST_CALLS'], 'a') as stream:
                stream.write(json.dumps({'argv': sys.argv[1:], 'cwd': os.getcwd()}) + '\\n')
            print('fixture stdout: ' + ' '.join(sys.argv[1:]))
            print('fixture stderr', file=sys.stderr)
        """)

    def tearDown(self):
        self.temporary.cleanup()

    def install_tool(self, name, body):
        path = self.tools / name
        path.write_text("#!" + sys.executable + "\n" + textwrap.dedent(body), encoding="utf-8")
        path.chmod(0o755)

    def invoke(self, *arguments, env=None):
        result = subprocess.run(
            [sys.executable, str(VERIFY), "--repo", str(self.repo),
             "--output-dir", str(self.output)] + list(arguments),
            env=self.env if env is None else env,
            cwd=str(self.root), capture_output=True, text=True, timeout=60,
        )
        payload = json.loads(result.stdout)
        return result, payload

    def recorded_calls(self):
        if not self.calls.exists():
            return []
        return [json.loads(line) for line in self.calls.read_text().splitlines()]

    def test_fixed_rust_commands_preserve_paths_and_evidence(self):
        result, report = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(report["status"], "passed")
        self.assertEqual([item["argv"] for item in self.recorded_calls()], [
            ["test"], ["clippy", "--", "-D", "warnings"], ["fmt", "--check"],
        ])
        self.assertTrue(all(item["cwd"] == str(self.repo) for item in self.recorded_calls()))
        self.assertFalse((self.root / "INJECTED").exists())
        self.assertFalse((self.root / "INJECTED2").exists())
        self.assertFalse((self.repo / "INJECTED").exists())
        stored = json.loads((self.output / "report.json").read_text())
        self.assertEqual(stored, report)
        for step in report["steps"]:
            self.assertEqual(step["status"], "passed")
            self.assertEqual(step["returncode"], 0)
            stdout = Path(step["stdout_log"]).read_bytes()
            self.assertIn(b"fixture stdout", stdout)
            self.assertEqual(step["stdout_sha256"], hashlib.sha256(stdout).hexdigest())
            self.assertIn("fixture stderr", Path(step["stderr_log"]).read_text())
            self.assertGreaterEqual(step["duration_seconds"], 0)
            self.assertIn("started_at", step)
            self.assertIn("finished_at", step)

    def test_failed_command_stops_later_gates(self):
        self.install_tool("cargo", """
            import json, os, sys
            with open(os.environ['PSTACK_TEST_CALLS'], 'a') as stream:
                stream.write(json.dumps({'argv': sys.argv[1:]}) + '\\n')
            print('partial fixture output', flush=True)
            print('fixture failure', file=sys.stderr)
            raise SystemExit(7)
        """)
        result, report = self.invoke("--scope", "all")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["steps"][0]["returncode"], 7)
        self.assertTrue(all(step["status"] == "skipped" for step in report["steps"][1:]))
        self.assertEqual(len(self.recorded_calls()), 1)
        self.assertFalse((self.output / "skills-snapshot").exists())
        self.assertEqual(Path(report["steps"][0]["stdout_log"]).read_text(), "partial fixture output\n")

    def test_plan_neither_executes_nor_writes(self):
        result, report = self.invoke("--scope", "all", "--plan")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(report["status"], "planned")
        self.assertTrue(all(step["status"] == "planned" for step in report["steps"]))
        self.assertFalse(self.calls.exists())
        self.assertFalse(self.output.exists())
        self.assertNotIn("report_path", report)

    def test_missing_tool_is_failure_with_logs(self):
        env = dict(self.env)
        env["PATH"] = str(self.root / "no-tools")
        result, report = self.invoke(env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(report["steps"][0]["status"], "missing_tool")
        self.assertIsNone(report["steps"][0]["returncode"])
        self.assertTrue(Path(report["steps"][0]["stderr_log"]).read_text())
        self.assertTrue((self.output / "report.json").exists())

    def test_timeout_preserves_partial_output_and_stops(self):
        self.install_tool("cargo", """
            import time
            print('before fixture timeout', flush=True)
            time.sleep(30)
        """)
        result, report = self.invoke("--timeout", "10")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(report["steps"][0]["status"], "timeout")
        self.assertEqual([step["status"] for step in report["steps"][1:]], ["skipped", "skipped"])
        self.assertIn("before fixture timeout", Path(report["steps"][0]["stdout_log"]).read_text())

    def test_repo_markers_are_checked_before_execution(self):
        (self.repo / "AGENTS.md").unlink()
        result, report = self.invoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(report["status"], "failed")
        self.assertFalse(self.calls.exists())
        self.assertFalse(self.output.exists())

    def test_existing_evidence_is_never_overwritten(self):
        self.output.mkdir()
        original = self.output / "report.json"
        original.write_text("old evidence")
        result, report = self.invoke()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(original.read_text(), "old evidence")
        self.assertFalse(self.calls.exists())

    def test_default_output_directory_is_persistent(self):
        result = subprocess.run(
            [sys.executable, str(VERIFY), "--repo", str(self.repo)],
            env=self.env, capture_output=True, text=True, timeout=60,
        )
        report = json.loads(result.stdout)
        self.assertEqual(result.returncode, 0, result.stderr)
        output = Path(report["output_dir"])
        try:
            self.assertTrue((output / "report.json").is_file())
            self.assertTrue(Path(report["steps"][0]["stdout_log"]).is_file())
        finally:
            import shutil
            shutil.rmtree(str(output))

    def install_smoke_tools(self, session_payload=None, schema_payload=None):
        if session_payload is None:
            session_payload = {"status": "success", "sync_status": "ok", "count": 0, "sessions": []}
        if schema_payload is None:
            schema_payload = {"skill exec": {"args": {"code": {"type": "string", "required": True}}}}
        binary_source = "#!" + sys.executable + "\n" + textwrap.dedent("""
            import json, os, sys
            from pathlib import Path
            with open(os.environ['PSTACK_TEST_CALLS'], 'a') as stream:
                stream.write(json.dumps({'argv': sys.argv[1:], 'env': {
                    key: value for key, value in os.environ.items()
                    if key.startswith(('VB_', 'VCLI_'))
                }}) + '\\n')
            if sys.argv[1:] == ['--version']:
                print('vcli 0.0.0-fixture')
            elif sys.argv[1:4] == ['schema', 'skill', 'exec']:
                print(SCHEMA_FIXTURE)
            elif sys.argv[1:3] == ['session', 'list']:
                print(SESSION_FIXTURE)
            else:
                raise SystemExit(99)
        """).replace("SCHEMA_FIXTURE", repr(json.dumps(schema_payload))).replace(
            "SESSION_FIXTURE", repr(json.dumps(session_payload)))
        self.install_tool("cargo", """
            import json, os, sys
            from pathlib import Path
            with open(os.environ['PSTACK_TEST_CALLS'], 'a') as stream:
                stream.write(json.dumps({'argv': sys.argv[1:]}) + '\\n')
            binary = Path(os.environ['CARGO_TARGET_DIR']) / 'debug' / 'vcli'
            binary.parent.mkdir(parents=True, exist_ok=True)
            binary.write_text(BINARY_SOURCE)
            binary.chmod(0o755)
        """.replace("BINARY_SOURCE", repr(binary_source)))

    def test_smoke_builds_before_contracts_and_isolates_runtime(self):
        self.install_smoke_tools()
        self.env.update({"VB_REMOTE_HOST": "DO_NOT_CONNECT", "VB_SESSION": "USER_SESSION",
                         "VB_PROFILE": "USER_PROFILE", "VB_PORT": "1",
                         "VB_REMOTE_HOST_USER_PROFILE": "DO_NOT_CONNECT_EITHER",
                         "VCLI_API_KEY": "FIXTURE_SECRET", "CARGO_TARGET_DIR": "WRONG"})
        result, report = self.invoke("--scope", "smoke")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(report["status"], "passed")
        self.assertEqual(len(report["steps"]), 4)
        calls = self.recorded_calls()
        self.assertEqual(calls[0]["argv"], ["build", "--bin", "vcli"])
        self.assertEqual(calls[1]["argv"], ["--version"])
        self.assertEqual(calls[2]["argv"], ["schema", "skill", "exec", "--format", "json"])
        self.assertEqual(calls[3]["argv"], ["session", "list", "--format", "json"])
        for call in calls[1:]:
            env = call["env"]
            self.assertNotIn("VB_REMOTE_HOST", env)
            self.assertNotIn("VB_SESSION", env)
            self.assertNotIn("VCLI_API_KEY", env)
            self.assertNotIn("VB_REMOTE_HOST_USER_PROFILE", env)
            self.assertEqual(env["VB_PROFILE"], "pstack-smoke")
            for key in ("VB_HOME", "VB_CACHE_DIR", "VB_LOG_DIR", "VB_OUTPUT_DIR",
                        "VB_TMP_DIR", "VB_STATE_DIR", "VB_CONFIG_DIR", "VB_TARGETS_FILE"):
                self.assertIn(Path(report["smoke_runtime"]), Path(env[key]).parents)
        self.assertTrue(all(step["contract_verified"] for step in report["steps"][1:]))
        self.assertTrue(report["limitations"])

    def test_bad_schema_contract_fails_and_does_not_list_sessions(self):
        self.install_smoke_tools(schema_payload={"skill exec": {"args": []}})
        result, report = self.invoke("--scope", "smoke")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(report["steps"][2]["status"], "failed")
        self.assertFalse(report["steps"][2]["contract_verified"])
        self.assertEqual(report["steps"][3]["status"], "skipped")
        self.assertEqual(len(self.recorded_calls()), 3)

    def test_unexpected_sessions_cannot_pass(self):
        self.install_smoke_tools(session_payload={"status": "success", "sync_status": "ok",
                                                  "count": 1, "sessions": [{"id": "user"}]})
        result, report = self.invoke("--scope", "smoke")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(report["steps"][3]["status"], "failed")
        self.assertFalse(report["steps"][3]["contract_verified"])

    def test_skills_suite_executes_inside_a_snapshot(self):
        source = self.repo / ".claude" / "skills" / "fixture" / "data"
        source.mkdir(parents=True)
        database = source / "skill_db.sqlite3"
        database.write_bytes(b"original user fixture database")
        runner = self.repo / ".github" / "scripts" / "run-skill-tests.sh"
        runner.parent.mkdir(parents=True)
        runner.write_text("fixture runner\n")
        self.install_tool("bash", """
            import json, os, sys
            from pathlib import Path
            with open(os.environ['PSTACK_TEST_CALLS'], 'a') as stream:
                stream.write(json.dumps({'argv': sys.argv[1:], 'cwd': os.getcwd()}) + '\\n')
            assert Path(sys.argv[1]).is_file()
            Path('.claude/skills/fixture/data/skill_db.sqlite3').write_bytes(b'changed snapshot only')
            print('fixture suite completed')
        """)
        result, report = self.invoke("--scope", "skills")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(report["status"], "passed")
        self.assertEqual(database.read_bytes(), b"original user fixture database")
        copied = Path(report["skills_snapshot"]) / ".claude" / "skills" / "fixture" / "data" / database.name
        self.assertEqual(copied.read_bytes(), b"changed snapshot only")
        self.assertEqual(self.recorded_calls()[0]["cwd"], report["skills_snapshot"])

    def test_skills_snapshot_rejects_write_through_symlinks(self):
        source = self.repo / ".claude" / "skills"
        source.mkdir(parents=True)
        (source / "user-data").symlink_to(self.root / "outside")
        runner = self.repo / ".github" / "scripts" / "run-skill-tests.sh"
        runner.parent.mkdir(parents=True)
        runner.write_text("fixture runner\n")
        result, report = self.invoke("--scope", "skills")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(report["steps"][0]["status"], "failed")
        self.assertFalse(self.calls.exists())
        self.assertTrue((self.output / "report.json").is_file())

    def test_skills_output_inside_source_is_rejected_before_writes(self):
        source = self.repo / ".claude" / "skills"
        source.mkdir(parents=True)
        alias = self.root / "skills-alias"
        alias.symlink_to(source, target_is_directory=True)
        for path in (source / "fixture" / "evidence", alias / "evidence"):
            with self.subTest(path=path):
                self.output = path
                result, report = self.invoke("--scope", "skills")
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(report["status"], "failed")
                self.assertFalse(self.calls.exists())
                self.assertFalse(path.exists())


class PortableSourceTests(unittest.TestCase):
    def test_python_39_syntax(self):
        for path in (VERIFY, Path(__file__)):
            ast.parse(path.read_text(encoding="utf-8"), filename=str(path), feature_version=(3, 9))

    def test_repo_default_points_to_project_root(self):
        self.assertEqual(VERIFY.parents[4] / ".claude" / "skills" / "pstack-vcli" / "scripts" / "verify.py", VERIFY)

    def test_json_boolean_count_is_rejected(self):
        with self.assertRaises(ValueError):
            VERIFY_MODULE.validate_contract("sessions", json.dumps({
                "status": "success", "sync_status": "ok", "count": False, "sessions": [],
            }))


if __name__ == "__main__":
    unittest.main()
