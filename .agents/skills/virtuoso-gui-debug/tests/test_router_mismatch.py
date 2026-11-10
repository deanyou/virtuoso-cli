"""Test route/executor mismatch and capability source tracing."""
import sys, tempfile
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.model import Scenario, Operation
from vgui_runner.engine import Runner, FakeExecutor, StepOutcome
from vgui_runner.router import CapabilitySnapshot, RoutePolicy, Channel

VALID = {
    "version": "1.0", "task_id": "t1", "session_id": "s1", "pid": 12345,
    "display": ":0", "cellview": {"lib": "L", "cell": "C", "view": "layout"},
    "steps": [{
        "id": "step1", "operation": "SCREENSHOT", "arguments": {"path": "/tmp/x.png"},
        "verifier": {"predicate": "window_exists", "expected": True},
        "timeout_seconds": 30, "max_retries": 0,
    }],
}


def outcomes_to_dict(pairs):
    return {(sid, phase, att): outcome for sid, phase, att, outcome in pairs}


def test_rejected_does_not_call_executor():
    """Route rejected -> executor.execute never called."""
    tmpdir = tempfile.mkdtemp()
    outdir = Path(tmpdir) / "run"
    called = []

    class CountingExecutor(FakeExecutor):
        def execute(self, step, attempt):
            called.append(step.id)
            return None

    caps = CapabilitySnapshot(vcli_available=False, session_alive=False, pid_valid=False,
                              display_available=False, window_identity_known=False,
                              remote_x11_allowed=False, local_x11_available=False,
                              ssh_budget_remaining=0, daemon_healthy=False)
    runner = Runner(CountingExecutor({}), caps=caps)
    scenario = Scenario.from_dict(VALID)
    summary = runner.run(scenario, outdir)
    assert not summary.passed
    assert summary.error_code == "ROUTE_REJECTED"
    assert called == [], f"executor should not be called, got {called}"
    print("PASS: rejected route does not call executor")


def test_capability_source_in_trace():
    """Default caps are labeled legacy_default in trace."""
    tmpdir = tempfile.mkdtemp()
    outdir = Path(tmpdir) / "run"
    runner = Runner(FakeExecutor({}))  # default caps
    scenario = Scenario.from_dict(VALID)
    summary = runner.run(scenario, outdir)
    trace = (outdir / "agent-actions.jsonl").read_text()
    assert "capability_source" in trace, "trace should record capability_source"
    assert "legacy_default" in trace, "default caps should be labeled legacy_default"
    print("PASS: capability_source recorded in trace")


def test_strict_mode_rejects_missing_caps():
    """Strict mode with empty caps rejects all operations."""
    tmpdir = tempfile.mkdtemp()
    outdir = Path(tmpdir) / "run"
    caps = CapabilitySnapshot()  # all False
    runner = Runner(FakeExecutor({}), caps=caps)
    scenario = Scenario.from_dict(VALID)
    summary = runner.run(scenario, outdir)
    assert not summary.passed
    assert summary.error_code == "ROUTE_REJECTED"
    print("PASS: strict mode rejects missing capabilities")


if __name__ == "__main__":
    test_rejected_does_not_call_executor()
    test_capability_source_in_trace()
    test_strict_mode_rejects_missing_caps()
    print("\n=== All mismatch tests passed ===")
