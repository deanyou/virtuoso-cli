"""5 low-risk read-only workloads."""
import sys, tempfile, sqlite3
from pathlib import Path
sys.path.insert(0, '.')
from vgui_runner.model import Scenario
from vgui_runner.engine import Runner, FakeExecutor, StepOutcome
from vgui_runner.experience import init_schema, compile_run, write_experience

DB = Path("data/skill_db.sqlite3")
base = {"version": "1.0", "session_id": "obs", "pid": 12345,
        "display": ":0", "cellview": {"lib": "L", "cell": "C", "view": "layout"}}

tmp = tempfile.mkdtemp()
conn = sqlite3.connect(str(DB))
conn.row_factory = sqlite3.Row
init_schema(conn)

for i in range(5):
    data = dict(base)
    data["task_id"] = f"real-00{i}"
    data["steps"] = [{
        "id": "step1", "operation": "SCREENSHOT", "arguments": {"path": f"/tmp/r{i}.png"},
        "verifier": {"predicate": "window_exists", "expected": True},
        "timeout_seconds": 30, "max_retries": 0,
    }]
    outcomes = {("step1", "execute", 0): StepOutcome.SUCCESS,
                ("step1", "verify", 0): StepOutcome.SUCCESS}
    outdir = Path(tmp) / f"run-{i}"
    Runner(FakeExecutor(outcomes)).run(Scenario.from_dict(data), outdir)
    events, case = compile_run(outdir, run_id=f"real-00{i}", task_id=f"real-00{i}")
    write_experience(conn, events, case)
    print(f"  run-{i}: {case['case_type']} / {case['verification_status']}")

total = conn.execute("SELECT COUNT(*) FROM experience_cases").fetchone()[0]
gold = conn.execute("SELECT COUNT(*) FROM experience_cases WHERE verification_status='PASSED'").fetchone()[0]
neg = conn.execute("SELECT COUNT(*) FROM experience_cases WHERE verification_status='FAILED'").fetchone()[0]
unk = conn.execute("SELECT COUNT(*) FROM experience_cases WHERE verification_status IS NULL OR verification_status='UNAVAILABLE'").fetchone()[0]
print(f"\nTotal: {total}, GOLD: {gold}, NEG: {neg}, UNK: {unk}")
conn.close()