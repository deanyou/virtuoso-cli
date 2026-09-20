"""Test Experience Compiler: determinism, idempotency, provenance."""
import sys, tempfile, json
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.model import Scenario
from vgui_runner.engine import Runner, FakeExecutor
from vgui_runner.experience import init_schema, compile_run, write_experience
import sqlite3

VALID = {
    "version": "1.0", "task_id": "t1", "session_id": "s1", "pid": 12345,
    "display": ":0", "cellview": {"lib": "L", "cell": "C", "view": "layout"},
    "steps": [{
        "id": "step1", "operation": "SCREENSHOT", "arguments": {"path": "/tmp/x.png"},
        "verifier": {"predicate": "window_exists", "expected": True},
        "timeout_seconds": 30, "max_retries": 0,
    }],
}


def test_determinism():
    """Same artifacts → same events/case (Gate A)."""
    tmpdir = tempfile.mkdtemp()
    outdir = Path(tmpdir) / "run"
    runner = Runner(FakeExecutor({}))
    runner.run(Scenario.from_dict(VALID), outdir)

    events1, case1 = compile_run(outdir, run_id="test-run", task_id="t1")
    events2, case2 = compile_run(outdir, run_id="test-run", task_id="t1")

    assert events1 == events2, "events not deterministic"
    assert case1 == case2, "case not deterministic"
    print(f"PASS: determinism ({len(events1)} events, case={case1['case_type']})")


def test_idempotent_write():
    """Same run compiled twice → DB rows unchanged (Gate: idempotent)."""
    tmpdir = tempfile.mkdtemp()
    outdir = Path(tmpdir) / "run"
    runner = Runner(FakeExecutor({}))
    runner.run(Scenario.from_dict(VALID), outdir)

    # Use in-memory DB
    conn = sqlite3.connect(":memory:")
    conn.row_factory = sqlite3.Row
    init_schema(conn)

    events, case = compile_run(outdir, run_id="idem-test", task_id="t1")
    e1, c1 = write_experience(conn, events, case)
    e2, c2 = write_experience(conn, events, case)  # second time

    total_e = conn.execute("SELECT COUNT(*) FROM experience_events").fetchone()[0]
    total_c = conn.execute("SELECT COUNT(*) FROM experience_cases").fetchone()[0]
    assert total_e == len(events), f"events duplicated: {total_e} vs {len(events)}"
    assert total_c == 1, f"cases duplicated: {total_c}"
    assert e2 == 0 and c2 == 0, f"second write should insert 0, got e={e2} c={c2}"
    print(f"PASS: idempotent write ({total_e} events, {total_c} case, second insert=0)")


def test_provenance():
    """Every event traces to trace line, case traces to events (Gate B)."""
    tmpdir = tempfile.mkdtemp()
    outdir = Path(tmpdir) / "run"
    runner = Runner(FakeExecutor({}))
    runner.run(Scenario.from_dict(VALID), outdir)

    events, case = compile_run(outdir, run_id="prov-test", task_id="t1")
    assert case is not None
    event_ids = {e["event_id"] for e in events}
    referenced = set(json.loads(case["event_refs"]))
    assert referenced == event_ids, "case event_refs mismatch"

    # Every event has a trace reference
    for e in events:
        refs = json.loads(e["evidence_refs"])
        assert any(r.startswith("trace#") for r in refs), f"event {e['event_id']} missing trace ref"

    print(f"PASS: provenance ({len(event_ids)} events all traceable)")


def test_value_sources():
    """Events have correct value_source classification."""
    tmpdir = tempfile.mkdtemp()
    outdir = Path(tmpdir) / "run"
    runner = Runner(FakeExecutor({}))
    runner.run(Scenario.from_dict(VALID), outdir)

    events, case = compile_run(outdir, run_id="vs-test", task_id="t1")
    sources = set(e["value_source"] for e in events)
    assert "OBSERVED" in sources, "should have OBSERVED events"
    assert "RULE_INFERRED" in sources, "should have RULE_INFERRED (ROUTE_DECIDED)"
    print(f"PASS: value sources = {sources}")


if __name__ == "__main__":
    test_determinism()
    test_idempotent_write()
    test_provenance()
    test_value_sources()
    print("\n=== All Experience Compiler tests passed ===")
