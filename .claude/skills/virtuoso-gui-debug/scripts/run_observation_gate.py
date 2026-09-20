"""Observation Gate test: run 10 fake workloads, check data quality."""
import sys, tempfile, json, sqlite3
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.model import Scenario
from vgui_runner.engine import Runner, FakeExecutor, StepOutcome
from vgui_runner.experience import init_schema, compile_run, write_experience

DB = Path("data/skill_db.sqlite3")

VALID = {
    "version": "1.0", "session_id": "s1", "pid": 12345,
    "display": ":0", "cellview": {"lib": "L", "cell": "C", "view": "layout"},
    "steps": [{
        "id": "step1", "operation": "SCREENSHOT", "arguments": {"path": "/tmp/x.png"},
        "verifier": {"predicate": "window_exists", "expected": True},
        "timeout_seconds": 30, "max_retries": 0,
    }],
}

# 10 runs: 6 success, 3 verify-fail, 1 execute-fail
scenarios = []
for i in range(10):
    data = dict(VALID)
    data["task_id"] = f"obs-run-{i:03d}"
    if i < 6:
        # success
        outcomes = {("step1", "execute", 0): StepOutcome.SUCCESS,
                    ("step1", "verify", 0): StepOutcome.SUCCESS}
    elif i < 9:
        # verify fail
        outcomes = {("step1", "execute", 0): StepOutcome.SUCCESS,
                    ("step1", "verify", 0): StepOutcome.FAILURE}
    else:
        # execute fail
        outcomes = {("step1", "execute", 0): StepOutcome.FAILURE}
    scenarios.append((data, outcomes, f"run-{i:03d}"))

# Run all
tmp = tempfile.mkdtemp()
conn = sqlite3.connect(str(DB))
conn.row_factory = sqlite3.Row
init_schema(conn)

for i, (data, outcomes, run_id) in enumerate(scenarios):
    outdir = Path(tmp) / run_id
    Runner(FakeExecutor(outcomes)).run(Scenario.from_dict(data), outdir)
    events, case = compile_run(outdir, run_id=run_id, task_id=data["task_id"])
    write_experience(conn, events, case)

print("Ran 10 fake workloads")

# ── Gate checks ──
print("\n=== Gate Checks ===")

# G1: Verified ratio >= 60%
total = conn.execute("SELECT COUNT(*) FROM experience_cases").fetchone()[0]
verified = conn.execute("SELECT COUNT(*) FROM experience_cases WHERE verification_status IS NOT NULL AND verification_status != 'UNKNOWN'").fetchone()[0]
g1 = verified / total * 100 if total else 0
print(f"G1 Verified ratio: {verified}/{total} = {g1:.0f}% (need >=60%)")

# G2: UNKNOWN <= 30%
unknown = conn.execute("SELECT COUNT(*) FROM experience_cases WHERE verification_status IS NULL OR verification_status = 'UNKNOWN'").fetchone()[0]
g2 = unknown / total * 100 if total else 0
print(f"G2 UNKNOWN ratio: {unknown}/{total} = {g2:.0f}% (need <=30%)")

# G3: duplicate < 10%
dup = conn.execute("SELECT COUNT(*) FROM (SELECT run_id, COUNT(*) c FROM experience_cases GROUP BY run_id HAVING c > 1)").fetchone()[0]
g3 = dup / total * 100 if total else 0
print(f"G3 Duplicate cases: {dup}/{total} = {g3:.0f}% (need <10%)")

# G4: taxonomy split < 20% (same failure_type should be consistent)
failure_types = conn.execute("SELECT DISTINCT failure_type FROM experience_cases WHERE failure_type IS NOT NULL").fetchall()
g4 = len(failure_types)
print(f"G4 Distinct failure_types: {g4} (need stable, not fragmented)")
for ft in failure_types:
    cnt = conn.execute("SELECT COUNT(*) FROM experience_cases WHERE failure_type=?", (ft["failure_type"],)).fetchone()[0]
    print(f"    {ft['failure_type']}: {cnt}")

# G5: provenance closure = 100%
orphan = 0
cases = conn.execute("SELECT case_id, event_refs FROM experience_cases").fetchall()
for c in cases:
    refs = json.loads(c["event_refs"]) if c["event_refs"] else []
    for eid in refs:
        row = conn.execute("SELECT 1 FROM experience_events WHERE event_id=?", (eid,)).fetchone()
        if not row:
            orphan += 1
g5 = (len(cases) * 100 - orphan) / (len(cases) * 100) * 100 if cases else 100
print(f"G5 Provenance closure: {len(cases)*100-orphan}/{len(cases)*100} refs valid = {g5:.0f}%")

# G6: deterministic replay = 100% (compile same run twice)
outdir0 = Path(tmp) / "run-000"
e1, c1 = compile_run(outdir0, run_id="run-000", task_id="t")
e2, c2 = compile_run(outdir0, run_id="run-000", task_id="t")
g6 = 100 if e1 == e2 and c1 == c2 else 0
print(f"G6 Deterministic replay: {g6}%")

# Pool distribution
gold = conn.execute("SELECT COUNT(*) FROM experience_cases WHERE verification_status='PASSED'").fetchone()[0]
neg = conn.execute("SELECT COUNT(*) FROM experience_cases WHERE verification_status='FAILED'").fetchone()[0]
unk = total - gold - neg
print(f"\n=== Distribution ===")
print(f"GOLD (PASSED):   {gold}")
print(f"NEGATIVE (FAIL): {neg}")
print(f"UNKNOWN:         {unk}")

conn.close()
print("\n=== Done ===")
