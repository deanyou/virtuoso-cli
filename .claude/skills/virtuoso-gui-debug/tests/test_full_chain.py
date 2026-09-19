"""Full-chain integration: Planner -> Runner -> Verifier -> Recovery -> Manifest."""
import sys, json, tempfile
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.planner import Planner
from vgui_runner.engine import Runner, FakeExecutor
from vgui_runner.verifier_result import VerifierResult, VerifyStatus
from vgui_runner.evidence import EvidenceManifest

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} — {msg}"); failed += 1


print("=== Chain 1: Planner screenshot_window -> Runner -> pass ===")
planner = Planner()
scenario = planner.plan("screenshot_window", task_id="chain-1", session_id="s1",
                        pid=1, display=":0", cellview={"lib": "L", "cell": "C", "view": "layout"},
                        window_title="CIW")
tmp = Path(tempfile.mkdtemp())
runner = Runner(FakeExecutor({}))
s = runner.run(scenario, tmp / "run1")
check("run passed", s.passed)

print("\n=== Chain 2: Planner key_press -> no auto-retry on failure ===")
class FailingKey(FakeExecutor):
    def verify(self, step, attempt):
        return VerifierResult(predicate="x", status=VerifyStatus.FAILED,
                              reason_code="boom", step_id=step.id, attempt=attempt)

scenario2 = planner.plan("key_press", task_id="chain-2", session_id="s2",
                         pid=1, display=":0", cellview={"lib": "L", "cell": "C", "view": "layout"},
                         window_title="CIW", key="Return")
tmp2 = Path(tempfile.mkdtemp())
runner2 = Runner(FailingKey({}))
s2 = runner2.run(scenario2, tmp2 / "run2")
check("run failed (non_idempotent, no retry)", not s2.passed)
check("error is VERIFY_ERROR or MANUAL", s2.error_code in ("VERIFY_ERROR", "MANUAL_INTERVENTION_REQUIRED"),
      f"got {s2.error_code}")

print("\n=== Chain 3: Planner window_close -> has 2 steps ===")
scenario3 = planner.plan("window_close", task_id="chain-3", session_id="s3",
                         pid=1, display=":0", cellview={"lib": "L", "cell": "C", "view": "layout"},
                         window_id="12345")
check("2 steps (close + wait)", len(scenario3.steps) == 2)
check("step 2 verifier expects False", scenario3.steps[1].verifier["expected"] is False)

print("\n=== Chain 4: Evidence manifest records run ===")
tmp4 = Path(tempfile.mkdtemp()) / "run4"
tmp4.mkdir(parents=True)
m = EvidenceManifest(tmp4, "chain-4", "s4", 1, ":0")
m.write_initial()
m.add_artifact("trace", "trace", tmp4 / "agent-actions.jsonl")
m.finalize("passed")
manifest = json.loads((tmp4 / "manifest.json").read_text())
check("manifest status=passed", manifest["status"] == "passed")
check("manifest has run_id", "run_id" in manifest)
check("manifest has started_at", "started_at" in manifest)

print("\n=== Chain 5: Template params cannot change risk ===")
# Even with different params, risk class stays fixed
s5a = planner.plan("ciw_command", task_id="t", session_id="s", pid=1, display=":0",
                   cellview={"lib": "L", "cell": "C", "view": "layout"}, command="getCurrentTime()")
s5b = planner.plan("ciw_command", task_id="t", session_id="s", pid=1, display=":0",
                   cellview={"lib": "L", "cell": "C", "view": "layout"}, command="dbCreateRect(...)")
check("both ciw steps have max_retries=0", s5a.steps[0].max_retries == 0 and s5b.steps[0].max_retries == 0)

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
