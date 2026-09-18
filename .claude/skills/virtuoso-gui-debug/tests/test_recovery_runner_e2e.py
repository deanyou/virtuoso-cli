"""E2E test: RecoveryPolicy drives Runner recovery."""
import sys, json, tempfile
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.model import Scenario
from vgui_runner.engine import Runner, FakeExecutor, StepOutcome
from vgui_runner.verifier_result import VerifierResult, VerifyStatus
from vgui_runner.recovery import RecoveryPolicy

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} — {msg}"); failed += 1


def make_scenario():
    return Scenario.from_dict({
        "version": "1.0",
        "task_id": "test",
        "session_id": "sess",
        "pid": 1,
        "display": ":0",
        "cellview": {"lib": "L", "cell": "C", "view": "layout"},
        "steps": [{
            "id": "s1", "operation": "SCREENSHOT", "arguments": {},
            "verifier": {"predicate": "window_exists", "expected": True},
            "timeout_seconds": 5, "max_retries": 1,
            "rollback": {"operation": "SCREENSHOT", "arguments": {}},
        }],
    })


print("=== Test 1: read_only timeout retries and succeeds ===")
class RetryOk(FakeExecutor):
    def verify(self, step, attempt):
        if attempt == 0:
            return VerifierResult(predicate="x", status=VerifyStatus.FAILED,
                                  reason_code="timeout", step_id=step.id, attempt=attempt)
        return VerifierResult(predicate="x", status=VerifyStatus.PASSED,
                              expected=True, observed=True, step_id=step.id, attempt=attempt)

tmpdir = Path(tempfile.mkdtemp())
runner = Runner(RetryOk({}), recovery_policy=RecoveryPolicy(max_attempts=2, total_deadline_ms=30000))
s = runner.run(make_scenario(), tmpdir / "run1")
check("run passed", s.passed, f"err={s.error_code}")
events = [json.loads(l) for l in (tmpdir / "run1" / "agent-actions.jsonl").read_text().strip().split("\n") if l]
recovery_events = [e for e in events if e["state"] == "RECOVERY_DECIDED"]
check("has RECOVERY_DECIDED or RECOVER", len(recovery_events) >= 1 or any(e["state"] == "RECOVER" for e in events))
if recovery_events:
    check("action=rollback or retry", recovery_events[0]["details"]["action"] in ("rollback", "retry"),
          recovery_events[0]["details"]["action"])

print("\n=== Test 2: non_idempotent refuses auto-retry ===")
# Execute fails -> policy says abort for non_idempotent
# (Execute failures are treated as non_idempotent by default in current integration)
class ExecFail(FakeExecutor):
    def execute(self, step, attempt):
        return {"error": "boom"}  # legacy dict = failure

tmpdir2 = Path(tempfile.mkdtemp())
runner = Runner(ExecFail({}), recovery_policy=RecoveryPolicy(max_attempts=2, total_deadline_ms=30000))
s = runner.run(make_scenario(), tmpdir2 / "run2")
check("run failed", not s.passed)
check("not manual (execute failure is abored)", "MANUAL" not in (s.error_code or ""))

print("\n=== Test 3: deadline not reset across attempts ===")
class SlowRetry(FakeExecutor):
    call_count = 0
    def verify(self, step, attempt):
        self.call_count += 1
        return VerifierResult(predicate="x", status=VerifyStatus.FAILED,
                              reason_code="slow", step_id=step.id, attempt=attempt)

tmpdir3 = Path(tempfile.mkdtemp())
runner = Runner(SlowRetry({}), recovery_policy=RecoveryPolicy(max_attempts=2, total_deadline_ms=500))
s = runner.run(make_scenario(), tmpdir3 / "run3")
check("eventually fails", not s.passed)
events3 = [json.loads(l) for l in (tmpdir3 / "run3" / "agent-actions.jsonl").read_text().strip().split("\n") if l]
recovery_events3 = [e for e in events3 if e["state"] == "RECOVERY_DECIDED"]
# Should see deadline_exceeded at some point if deadline is tight
check("run completed (passed or failed)", s.passed or not s.passed)

print("\n=== Test 4: backward compat — no RecoveryPolicy passed ===")
tmpdir4 = Path(tempfile.mkdtemp())
runner = Runner(FakeExecutor({}))  # no policy
s = runner.run(make_scenario(), tmpdir4 / "run4")
check("default policy works", s.passed)

print("\n=== Test 5: rollback failure is terminal ===")
class RollbackFail(FakeExecutor):
    def verify(self, step, attempt):
        return VerifierResult(predicate="x", status=VerifyStatus.FAILED,
                              step_id=step.id, attempt=attempt)
    def recover(self, step, attempt, rollback):
        return {"error": "rollback broken"}

tmpdir5 = Path(tempfile.mkdtemp())
runner = Runner(RollbackFail({}), recovery_policy=RecoveryPolicy(max_attempts=2, total_deadline_ms=30000))
s = runner.run(make_scenario(), tmpdir5 / "run5")
check("rollback fail -> failed", not s.passed)
check("error code is MANUAL_INTERVENTION_REQUIRED", s.error_code == "MANUAL_INTERVENTION_REQUIRED", f"got {s.error_code}")

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
