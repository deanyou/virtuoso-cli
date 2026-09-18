"""Integration test: Runner actually consumes VerifierResult."""
import sys, json, tempfile
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.model import Scenario
from vgui_runner.engine import Runner, FakeExecutor, StepOutcome
from vgui_runner.verifier_result import VerifierResult, VerifyStatus

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} — {msg}"); failed += 1


def make_scenario(with_rollback=False):
    step = {
        "id": "step-1",
        "operation": "SCREENSHOT",
        "arguments": {},
        "verifier": {"predicate": "window_exists", "expected": True},
        "timeout_seconds": 5,
        "max_retries": 1,
    }
    if with_rollback:
        step["rollback"] = {"operation": "SCREENSHOT", "arguments": {}}
    return Scenario.from_dict({
        "version": "1.0",
        "task_id": "test-task",
        "session_id": "test-sess",
        "pid": 12345,
        "display": ":5.0",
        "cellview": {"lib": "LIB", "cell": "CELL", "view": "layout"},
        "steps": [step],
    })


def run_executor(outcomes):
    """Run FakeExecutor and return (summary, trace events)."""
    tmpdir = Path(tempfile.mkdtemp())
    outdir = tmpdir / "run"
    executor = FakeExecutor(outcomes)
    runner = Runner(executor)
    summary = runner.run(make_scenario(), outdir)
    trace_events = []
    trace_path = outdir / "agent-actions.jsonl"
    if trace_path.exists():
        for line in trace_path.read_text().strip().split("\n"):
            if line:
                trace_events.append(json.loads(line))
    with open(outdir / "summary.json") as f:
        summary_json = json.load(f)
    return summary, summary_json, trace_events


print("=== Test 1: verify PASSED -> run passed ===")
s, sj, events = run_executor({})  # all default success
check("summary.passed", s.passed)
check("summary.json status=passed", sj["status"] == "passed")
# Find VERIFY events
verify_events = [e for e in events if e["state"] == "VERIFY" and e.get("details")]
check("has VERIFY result event", len(verify_events) >= 1)
if verify_events:
    check("verify status=passed", verify_events[0]["outcome"] == "PASSED")
    check("verify details.status=passed", verify_events[0]["details"]["status"] == "passed")

print("\n=== Test 2: verify FAILED -> run failed with VERIFY_ERROR ===")
s, sj, events = run_executor({("step-1", "verify", 0): StepOutcome.FAILURE})
check("summary not passed", not s.passed)
check("error_code=VERIFY_ERROR", s.error_code == "VERIFY_ERROR", f"got {s.error_code}")
verify_events = [e for e in events if e["state"] == "VERIFY" and e.get("details")]
if verify_events:
    check("verify outcome=FAILED", verify_events[0]["outcome"] == "FAILED")
    check("verify details.status=failed", verify_events[0]["details"]["status"] == "failed")

print("\n=== Test 3: retry - first verify fails, second passes ===")
# Need custom executor that returns unavailable on first attempt
class RetryExecutor(FakeExecutor):
    def verify(self, step, attempt):
        if attempt == 0:
            return VerifierResult(
                predicate="window_exists",
                status=VerifyStatus.UNAVAILABLE,
                reason_code="channel_down",
                step_id=step.id,
                attempt=attempt,
            )
        return VerifierResult(
            predicate="window_exists",
            status=VerifyStatus.PASSED,
            expected=True, observed=True,
            step_id=step.id, attempt=attempt,
        )

tmpdir = Path(tempfile.mkdtemp())
outdir = tmpdir / "run"
runner = Runner(RetryExecutor({}))
s = runner.run(make_scenario(with_rollback=True), outdir)
check("retry: eventually passed", s.passed, f"error={s.error_code}")
trace_path = outdir / "agent-actions.jsonl"
events = [json.loads(l) for l in trace_path.read_text().strip().split("\n") if l]
verify_events = [e for e in events if e["state"] == "VERIFY" and e.get("details")]
check("retry: 2 verify result events", len(verify_events) == 2, f"got {len(verify_events)}")
if len(verify_events) == 2:
    check("retry attempt 0 unavailable", verify_events[0]["details"]["status"] == "unavailable")
    check("retry attempt 1 passed", verify_events[1]["details"]["status"] == "passed")
    check("retry attempt fields distinct", verify_events[0]["attempt"] == 0 and verify_events[1]["attempt"] == 1)

print("\n=== Test 4: unavailable with no retry -> VERIFY_UNAVAILABLE ===")
class UnavailableExecutor(FakeExecutor):
    def verify(self, step, attempt):
        return VerifierResult(
            predicate="window_exists",
            status=VerifyStatus.UNAVAILABLE,
            reason_code="channel_down",
            step_id=step.id, attempt=attempt,
        )

tmpdir = Path(tempfile.mkdtemp())
outdir = tmpdir / "run"
# max_retries=0 means no retry
scenario = make_scenario()
runner = Runner(UnavailableExecutor({}))
s = runner.run(scenario, outdir)
check("unavailable: not passed", not s.passed)
check("unavailable: error_code distinct", s.error_code == "VERIFY_UNAVAILABLE", f"got {s.error_code}")

print("\n=== Test 5: legacy dict executor still works ===")
class LegacyExecutor(FakeExecutor):
    """Old-style executor returning Optional[Dict]."""
    def verify(self, step, attempt):
        return None  # legacy: None = passed

tmpdir = Path(tempfile.mkdtemp())
outdir = tmpdir / "run"
runner = Runner(LegacyExecutor({}))
s = runner.run(make_scenario(), outdir)
check("legacy: passed", s.passed)
trace_path = outdir / "agent-actions.jsonl"
events = [json.loads(l) for l in trace_path.read_text().strip().split("\n") if l]
verify_events = [e for e in events if e["state"] == "VERIFY" and e.get("details")]
if verify_events:
    check("legacy: verify details.status=passed", verify_events[0]["details"]["status"] == "passed")

print("\n=== Test 6: skipped does not fail run ===")
class SkippedExecutor(FakeExecutor):
    def verify(self, step, attempt):
        return VerifierResult(
            predicate="window_exists",
            status=VerifyStatus.SKIPPED,
            reason_code="policy_skip",
            step_id=step.id, attempt=attempt,
        )

tmpdir = Path(tempfile.mkdtemp())
outdir = tmpdir / "run"
runner = Runner(SkippedExecutor({}))
s = runner.run(make_scenario(), outdir)
check("skipped: run passed", s.passed)

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
