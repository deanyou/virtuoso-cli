"""Tests for vgui_runner.engine - state machine and fake executor."""
import json
import shutil
import tempfile
import unittest
import sys
import warnings
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))

from vgui_runner.model import Scenario, Operation
from vgui_runner.engine import (
    FakeExecutor,
    Runner,
    RunState,
    StepOutcome,
    StepPhase,
    ERROR_PRECHECK,
    ERROR_BASELINE,
    ERROR_EXECUTE,
    ERROR_RECOVER,
    ERROR_VERIFY,
    ERROR_ROLLBACK,
)


VALID_SCENARIO_DICT = {
    "version": "1.0",
    "task_id": "test-task-001",
    "session_id": "sess-abc123",
    "pid": 12345,
    "display": ":0",
    "cellview": {"lib": "myLib", "cell": "myCell", "view": "schematic"},
    "steps": [
        {
            "id": "step1",
            "operation": "VCLI_LOAD",
            "arguments": {"command": "myCommand"},
            "verifier": {"predicate": "window_exists", "expected": True},
            "timeout_seconds": 30,
            "max_retries": 1,
        },
        {
            "id": "step2",
            "operation": "WINDOW_WAIT",
            "arguments": {"window_title": "Virtuoso", "state": "visible"},
            "verifier": {"predicate": "state_matches", "expected": "visible"},
            "timeout_seconds": 60,
            "max_retries": 0,
        },
    ],
}


def make_scenario(data=None):
    return Scenario.from_dict(data or VALID_SCENARIO_DICT)


def outcomes_to_dict(outcomes_list):
    """Convert list of (step_id, phase, attempt, outcome) to dict."""
    return {(s, p, a): o for s, p, a, o in outcomes_list}


def make_temp_dir():
    return tempfile.mkdtemp()


def cleanup_temp_dir(path):
    shutil.rmtree(path, ignore_errors=True)


class TestFakeExecutorPerPhase(unittest.TestCase):
    def test_fake_executor_default_success_all_phases(self):
        executor = FakeExecutor()
        scenario = make_scenario()
        self.assertIsNone(executor.precheck(scenario))
        self.assertIsNone(executor.baseline(scenario))
        step = scenario.steps[0]
        self.assertIsNone(executor.execute(step, 0))
        self.assertIsNone(executor.verify(step, 0))
        self.assertIsNone(executor.recover(step, 0, None))

    def test_fake_executor_execute_failure_attempt_0(self):
        outcomes = outcomes_to_dict([("step1", "execute", 0, StepOutcome.FAILURE)])
        executor = FakeExecutor(outcomes)
        step = make_scenario().steps[0]
        self.assertIsNotNone(executor.execute(step, 0))

    def test_fake_executor_execute_failure_attempt_1_success(self):
        """Test that attempt 0 fails and attempt 1 succeeds."""
        outcomes = outcomes_to_dict([
            ("step1", "execute", 0, StepOutcome.FAILURE),
            ("step1", "execute", 1, StepOutcome.SUCCESS),
        ])
        executor = FakeExecutor(outcomes)
        step = make_scenario().steps[0]
        self.assertIsNotNone(executor.execute(step, 0))
        self.assertIsNone(executor.execute(step, 1))

    def test_fake_executor_verify_failure(self):
        outcomes = outcomes_to_dict([("step1", "verify", 0, StepOutcome.FAILURE)])
        executor = FakeExecutor(outcomes)
        step = make_scenario().steps[0]
        self.assertIsNotNone(executor.verify(step, 0))

    def test_fake_executor_recover_failure(self):
        outcomes = outcomes_to_dict([("step1", "recover", 0, StepOutcome.FAILURE)])
        executor = FakeExecutor(outcomes)
        step = make_scenario().steps[0]
        self.assertIsNotNone(executor.recover(step, 0, None))


class TestRunnerHappyPath(unittest.TestCase):
    def test_happy_path_passes(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            executor = FakeExecutor()
            runner = Runner(executor)
            scenario = make_scenario()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            self.assertTrue(summary.passed)
            self.assertIsNone(summary.failed_step_id)
            self.assertIsNone(summary.error_code)
            self.assertIsNone(summary.phase)
        finally:
            cleanup_temp_dir(tmpdir)

    def test_task_json_written(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            executor = FakeExecutor()
            runner = Runner(executor)
            scenario = make_scenario()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                runner.run(scenario, outdir)

            task_path = outdir / "task.json"
            self.assertTrue(task_path.exists())
            with open(task_path) as f:
                task = json.load(f)
            self.assertEqual(task["task_id"], "test-task-001")
            self.assertEqual(task["session_id"], "sess-abc123")
        finally:
            cleanup_temp_dir(tmpdir)

    def test_jsonl_events_written(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            executor = FakeExecutor()
            runner = Runner(executor)
            scenario = make_scenario()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                runner.run(scenario, outdir)

            jsonl_path = outdir / "agent-actions.jsonl"
            self.assertTrue(jsonl_path.exists())
            with open(jsonl_path) as f:
                lines = f.readlines()
            self.assertGreater(len(lines), 0)
            seqs = [json.loads(l)["seq"] for l in lines]
            self.assertEqual(seqs, list(range(len(seqs))))
        finally:
            cleanup_temp_dir(tmpdir)

    def test_summary_json_written(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            executor = FakeExecutor()
            runner = Runner(executor)
            scenario = make_scenario()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            summary_path = outdir / "summary.json"
            self.assertTrue(summary_path.exists())
            with open(summary_path) as f:
                s = json.load(f)
            self.assertEqual(s["status"], "passed")
        finally:
            cleanup_temp_dir(tmpdir)

    def test_final_passed_event_emitted(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            executor = FakeExecutor()
            runner = Runner(executor)
            scenario = make_scenario()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                runner.run(scenario, outdir)

            jsonl_path = outdir / "agent-actions.jsonl"
            with open(jsonl_path) as f:
                lines = f.readlines()
            last_event = json.loads(lines[-1])
            self.assertEqual(last_event["state"], "PASSED")
            self.assertEqual(last_event["outcome"], "SUCCESS")
        finally:
            cleanup_temp_dir(tmpdir)


class TestPrecheckFailure(unittest.TestCase):
    def test_precheck_failure(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            outcomes = outcomes_to_dict([("_precheck", "precheck", 0, StepOutcome.FAILURE)])
            executor = FakeExecutor(outcomes)
            runner = Runner(executor)
            scenario = make_scenario()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            self.assertFalse(summary.passed)
            self.assertIsNone(summary.failed_step_id)
            self.assertEqual(summary.error_code, ERROR_PRECHECK)
            self.assertEqual(summary.phase, "PRECHECK")
        finally:
            cleanup_temp_dir(tmpdir)


class TestBaselineFailure(unittest.TestCase):
    def test_baseline_failure(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            outcomes = outcomes_to_dict([("_baseline", "baseline", 0, StepOutcome.FAILURE)])
            executor = FakeExecutor(outcomes)
            runner = Runner(executor)
            scenario = make_scenario()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            self.assertFalse(summary.passed)
            self.assertIsNone(summary.failed_step_id)
            self.assertEqual(summary.error_code, ERROR_BASELINE)
            self.assertEqual(summary.phase, "BASELINE")
        finally:
            cleanup_temp_dir(tmpdir)


class TestExecuteFailRecoverSuccess(unittest.TestCase):
    def test_execute_fail_then_recover_then_success(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            outcomes = outcomes_to_dict([
                ("step1", "execute", 0, StepOutcome.FAILURE),
                ("step1", "recover", 0, StepOutcome.SUCCESS),
                ("step1", "execute", 1, StepOutcome.SUCCESS),
                ("step1", "verify", 1, StepOutcome.SUCCESS),
            ])
            executor = FakeExecutor(outcomes)
            runner = Runner(executor)
            scenario = make_scenario()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            self.assertTrue(summary.passed)
        finally:
            cleanup_temp_dir(tmpdir)

    def test_execute_fail_exhausted_retry(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            data = dict(VALID_SCENARIO_DICT)
            data["steps"] = [dict(VALID_SCENARIO_DICT["steps"][0])]
            data["steps"][0]["max_retries"] = 1
            scenario = Scenario.from_dict(data)

            outcomes = outcomes_to_dict([
                ("step1", "execute", 0, StepOutcome.FAILURE),
                ("step1", "recover", 0, StepOutcome.SUCCESS),
                ("step1", "execute", 1, StepOutcome.FAILURE),
            ])
            executor = FakeExecutor(outcomes)
            runner = Runner(executor)
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            self.assertFalse(summary.passed)
            self.assertEqual(summary.failed_step_id, "step1")
            self.assertEqual(summary.error_code, ERROR_EXECUTE)
        finally:
            cleanup_temp_dir(tmpdir)


class TestVerifyFailRecoverRetry(unittest.TestCase):
    def test_verify_fail_with_rollback_then_retry_then_success(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            data = dict(VALID_SCENARIO_DICT)
            data["steps"] = [dict(VALID_SCENARIO_DICT["steps"][0])]
            data["steps"][0]["max_retries"] = 1
            data["steps"][0]["rollback"] = {
                "operation": "RECOVER",
                "arguments": {"action": "undo", "target": "current_step"},
            }
            scenario = Scenario.from_dict(data)

            outcomes = outcomes_to_dict([
                ("step1", "execute", 0, StepOutcome.SUCCESS),
                ("step1", "verify", 0, StepOutcome.FAILURE),
                ("step1", "recover", 0, StepOutcome.SUCCESS),
                ("step1", "execute", 1, StepOutcome.SUCCESS),
                ("step1", "verify", 1, StepOutcome.SUCCESS),
            ])
            executor = FakeExecutor(outcomes)
            runner = Runner(executor)
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            self.assertTrue(summary.passed)
        finally:
            cleanup_temp_dir(tmpdir)


class TestRollbackFailure(unittest.TestCase):
    def test_rollback_failure_stops(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            data = dict(VALID_SCENARIO_DICT)
            data["steps"] = [dict(VALID_SCENARIO_DICT["steps"][0])]
            data["steps"][0]["max_retries"] = 1
            data["steps"][0]["rollback"] = {
                "operation": "RECOVER",
                "arguments": {"action": "undo", "target": "current_step"},
            }
            scenario = Scenario.from_dict(data)

            outcomes = outcomes_to_dict([
                ("step1", "execute", 0, StepOutcome.FAILURE),
                ("step1", "recover", 0, StepOutcome.FAILURE),
            ])
            executor = FakeExecutor(outcomes)
            runner = Runner(executor)
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            self.assertFalse(summary.passed)
            self.assertEqual(summary.failed_step_id, "step1")
            self.assertEqual(summary.error_code, ERROR_RECOVER)
        finally:
            cleanup_temp_dir(tmpdir)


class TestNoRetryWhenMaxRetriesZero(unittest.TestCase):
    def test_no_retry_when_max_retries_zero(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            data = dict(VALID_SCENARIO_DICT)
            data["steps"] = [dict(VALID_SCENARIO_DICT["steps"][1])]
            scenario = Scenario.from_dict(data)

            outcomes = outcomes_to_dict([("step2", "execute", 0, StepOutcome.FAILURE)])
            executor = FakeExecutor(outcomes)
            runner = Runner(executor)
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            self.assertFalse(summary.passed)
            self.assertEqual(summary.failed_step_id, "step2")
            self.assertEqual(summary.error_code, ERROR_EXECUTE)
        finally:
            cleanup_temp_dir(tmpdir)


class TestVerifyFailureNoRollback(unittest.TestCase):
    def test_verify_failure_without_rollback_fails(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            data = dict(VALID_SCENARIO_DICT)
            data["steps"] = [dict(VALID_SCENARIO_DICT["steps"][0])]
            data["steps"][0]["max_retries"] = 0
            scenario = Scenario.from_dict(data)

            outcomes = outcomes_to_dict([
                ("step1", "execute", 0, StepOutcome.SUCCESS),
                ("step1", "verify", 0, StepOutcome.FAILURE),
            ])
            executor = FakeExecutor(outcomes)
            runner = Runner(executor)
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            self.assertFalse(summary.passed)
            self.assertEqual(summary.failed_step_id, "step1")
            self.assertEqual(summary.error_code, ERROR_VERIFY)
        finally:
            cleanup_temp_dir(tmpdir)


class TestAtomicTaskJson(unittest.TestCase):
    def test_task_json_is_atomic(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            executor = FakeExecutor()
            runner = Runner(executor)
            scenario = make_scenario()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            task_path = outdir / "task.json"
            self.assertTrue(task_path.exists())
        finally:
            cleanup_temp_dir(tmpdir)


class TestOutputDirExists(unittest.TestCase):
    def test_output_dir_must_not_exist(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            executor = FakeExecutor()
            runner = Runner(executor)
            scenario = make_scenario()

            with warnings.catch_warnings():
                warnings.simplefilter("error")
                runner.run(scenario, outdir)

            # Second run to same dir should raise FileExistsError
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                with self.assertRaises(FileExistsError):
                    runner.run(scenario, outdir)
        finally:
            cleanup_temp_dir(tmpdir)


class TestFinalFailedEvent(unittest.TestCase):
    def test_final_failed_event_emitted(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            data = dict(VALID_SCENARIO_DICT)
            data["steps"] = [dict(VALID_SCENARIO_DICT["steps"][0])]
            data["steps"][0]["max_retries"] = 0
            scenario = Scenario.from_dict(data)

            outcomes = outcomes_to_dict([("step1", "execute", 0, StepOutcome.FAILURE)])
            executor = FakeExecutor(outcomes)
            runner = Runner(executor)
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                summary = runner.run(scenario, outdir)

            jsonl_path = outdir / "agent-actions.jsonl"
            with open(jsonl_path) as f:
                lines = f.readlines()
            last_event = json.loads(lines[-1])
            self.assertEqual(last_event["state"], "FAILED")
        finally:
            cleanup_temp_dir(tmpdir)


if __name__ == "__main__":
    unittest.main()
