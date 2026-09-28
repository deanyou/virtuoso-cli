"""Tests for engine P0.5 trace wiring - 经验校准 support 写入 trace。

验证：注入校准后 ROUTE_DECIDED / RECOVERY_DECIDED 的 details 携带 empirical；
未注入时（或校准不可用）trace 保持原样、run 正常降级。
"""
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from vgui_runner.empirical_calibration import EmpiricalCalibration
from vgui_runner.engine import FakeExecutor, Runner, StepOutcome
from vgui_runner.model import Scenario

SCENARIO = {
    "version": "1.0",
    "task_id": "t",
    "session_id": "s",
    "pid": 123,
    "display": ":0",
    "cellview": {"lib": "L", "cell": "C", "view": "layout"},
    "steps": [
        {
            "id": "step1",
            "operation": "SCREENSHOT",
            "arguments": {"path": "/tmp/x.png"},
            "verifier": {"predicate": "window_exists", "expected": True},
            "timeout_seconds": 30,
            "max_retries": 0,
        }
    ],
}

FAILING_SCENARIO = {
    "version": "1.0",
    "task_id": "t-fail",
    "session_id": "s",
    "pid": 123,
    "display": ":0",
    "cellview": {"lib": "L", "cell": "C", "view": "layout"},
    "steps": [
        {
            "id": "step1",
            "operation": "SCREENSHOT",
            "arguments": {"path": "/tmp/x.png"},
            "verifier": {"predicate": "window_exists", "expected": True},
            "timeout_seconds": 30,
            "max_retries": 0,
        }
    ],
}


def calibration():
    return EmpiricalCalibration(
        exact={("skill", "", 0): (22, 16)},
        channel_failure={("skill", ""): (22, 16)},
        channel={"skill": (22, 16)},
        global_stats=(22, 16),
        n_min=5,
        version="trace-v1",
    )


def load_events(outdir):
    lines = (outdir / "agent-actions.jsonl").read_text().strip().split("\n")
    return [json.loads(l) for l in lines if l]


class TestRouteEmpiricalTrace(unittest.TestCase):
    def test_route_event_carries_empirical_support(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            runner = Runner(FakeExecutor({}), calibration=calibration())
            summary = runner.run(Scenario.from_dict(SCENARIO), outdir)
            self.assertTrue(summary.passed)
            events = load_events(outdir)
            route_events = [e for e in events if e["state"] == "ROUTE_DECIDED"]
            self.assertEqual(len(route_events), 1)
            emp = route_events[0]["details"].get("empirical")
            self.assertIsNotNone(emp, "ROUTE_DECIDED should carry empirical support")
            self.assertEqual(emp["sample_count"], 38)
            self.assertEqual(emp["calibration_version"], "trace-v1")
            self.assertEqual(emp["grouping_level"], "exact")
        finally:
            import shutil
            shutil.rmtree(tmpdir, ignore_errors=True)

    def test_recovery_event_carries_empirical_support(self):
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        try:
            outcomes = {("step1", "execute", 0): StepOutcome.FAILURE}
            runner = Runner(FakeExecutor(outcomes), calibration=calibration())
            summary = runner.run(Scenario.from_dict(FAILING_SCENARIO), outdir)
            self.assertFalse(summary.passed)
            events = load_events(outdir)
            recovery_events = [e for e in events if e["state"] == "RECOVERY_DECIDED"]
            self.assertGreaterEqual(len(recovery_events), 1)
            emp = recovery_events[0]["details"].get("empirical")
            self.assertIsNotNone(emp, "RECOVERY_DECIDED should carry empirical support")
            self.assertEqual(emp["grouping_level"], "channel")
        finally:
            import shutil
            shutil.rmtree(tmpdir, ignore_errors=True)


class TestNoCalibrationDegrades(unittest.TestCase):
    def test_empty_calibration_has_no_empirical_key(self):
        # 校准表无匹配分组（如全新环境）→ estimate 为 None → trace 不带 empirical
        tmpdir = tempfile.mkdtemp()
        outdir = Path(tmpdir) / "run"
        empty = EmpiricalCalibration(n_min=5, version="empty-v1")
        try:
            runner = Runner(FakeExecutor({}), calibration=empty)
            summary = runner.run(Scenario.from_dict(SCENARIO), outdir)
            self.assertTrue(summary.passed)
            events = load_events(outdir)
            route_events = [e for e in events if e["state"] == "ROUTE_DECIDED"]
            self.assertNotIn("empirical", route_events[0]["details"])
        finally:
            import shutil
            shutil.rmtree(tmpdir, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
