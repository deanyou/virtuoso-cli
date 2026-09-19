"""Failure corpus: inject failures and verify terminal state semantics."""
import sys, tempfile, json
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.engine import Runner, FakeExecutor, StepOutcome
from vgui_runner.verifier_result import VerifierResult, VerifyStatus
from vgui_runner.evidence import EvidenceManifest

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} - {msg}"); failed += 1


def make_scenario():
    from vgui_runner.model import Scenario
    steps = [
        {"id": "step-1", "operation": "WINDOW_ACTIVATE",
         "arguments": {"window_title": "CIW"},
         "verifier": {"predicate": "window_exists", "expected": True},
         "timeout_seconds": 5, "max_retries": 0},
        {"id": "step-2", "operation": "SCREENSHOT",
         "arguments": {},
         "verifier": {"predicate": "window_exists", "expected": True},
         "timeout_seconds": 5, "max_retries": 0},
    ]
    return Scenario.from_dict({
        "version": "1.0", "task_id": "corpus", "session_id": "s",
        "pid": 1, "display": ":0",
        "cellview": {"lib": "L", "cell": "C", "view": "layout"},
        "steps": steps,
    })


class FailureInjectionExecutor(FakeExecutor):
    def __init__(self, failures):
        super().__init__({})
        self._failures = failures

    def execute(self, step, attempt):
        key = (step.id, "execute", attempt)
        if key in self._failures:
            val = self._failures[key]
            if isinstance(val, dict):
                return val
        return None

    def verify(self, step, attempt):
        key = (step.id, "verify", attempt)
        predicate = dict(step.verifier or {}).get("predicate", "unknown")
        expected = dict(step.verifier or {}).get("expected")
        if key in self._failures:
            return VerifierResult(predicate=predicate, status=self._failures[key],
                expected=expected, observed="<injected>", reason_code="injected",
                step_id=step.id, attempt=attempt)
        return VerifierResult(predicate=predicate, status=VerifyStatus.PASSED,
            expected=expected, observed="<injected>", step_id=step.id, attempt=attempt)


def run_with(failures):
    s = make_scenario()
    tmp = Path(tempfile.mkdtemp()) / "run"
    runner = Runner(FailureInjectionExecutor(failures))
    return runner.run(s, tmp)


print("=== F1: verifier unavailable ===")
r = run_with({("step-2", "verify", 0): VerifyStatus.UNAVAILABLE})
check("run failed", not r.passed)
check("error recorded", r.error_code is not None, f"got {r.error_code}")

print("\n=== F2: verify failed ===")
r = run_with({("step-2", "verify", 0): VerifyStatus.FAILED})
check("run failed", not r.passed)

print("\n=== F3: execute error ===")
r = run_with({("step-1", "execute", 0): {"error": "connection reset"}})
check("run failed", not r.passed, f"passed={r.passed}")

print("\n=== F4: first step fails, second never runs ===")
r = run_with({("step-1", "execute", 0): {"error": "window gone"}})
check("run failed", not r.passed, f"passed={r.passed}")

print("\n=== F5: timeout-like error ===")
r = run_with({("step-1", "execute", 0): {"error": "deadline exceeded"}})
check("run failed", not r.passed, f"passed={r.passed}")

print("\n=== F6: skipped verifier does not fail ===")
class SkipExec(FakeExecutor):
    def verify(self, step, attempt):
        return VerifierResult(predicate="x", status=VerifyStatus.SKIPPED,
            reason_code="policy", step_id=step.id, attempt=attempt)
s = make_scenario()
tmp = Path(tempfile.mkdtemp()) / "run"
r = Runner(SkipExec()).run(s, tmp)
check("skipped passes", r.passed)

print("\n=== F7: evidence manifest records failure ===")
tmp = Path(tempfile.mkdtemp()) / "run"
tmp.mkdir()
m = EvidenceManifest(tmp, "corpus", "s", 1, ":0")
m.write_initial()
m.fail("injected")
manifest = json.loads((tmp / "manifest.json").read_text())
check("status=failed", manifest["status"] == "failed")

print("\n=== F8: unavailable blocks progression ===")
r = run_with({("step-1", "verify", 0): VerifyStatus.UNAVAILABLE})
check("unavailable blocks", not r.passed)

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
