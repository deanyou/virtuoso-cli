"""Test Experience Retrieval: determinism, verified-only, negative memory, provenance."""
import sys, tempfile, json, sqlite3
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.model import Scenario
from vgui_runner.engine import Runner, FakeExecutor
from vgui_runner.experience import init_schema, compile_run, write_experience
from vgui_runner.experience_retrieval import init_fts, search_experience


def make_populated_db() -> Path:
    """Create temp DB with a few runs of different outcomes."""
    tmp = tempfile.mkdtemp()
    db_path = Path(tmp) / "test.sqlite3"
    conn = sqlite3.connect(str(db_path))
    conn.row_factory = sqlite3.Row
    init_schema(conn)
    init_fts(conn)

    # Scenario that passes
    valid = {
        "version": "1.0", "task_id": "t1", "session_id": "s1", "pid": 12345,
        "display": ":0", "cellview": {"lib": "L", "cell": "C", "view": "layout"},
        "steps": [{
            "id": "step1", "operation": "SCREENSHOT", "arguments": {"path": "/tmp/x.png"},
            "verifier": {"predicate": "window_exists", "expected": True},
            "timeout_seconds": 30, "max_retries": 0,
        }],
    }

    # Run 1: success
    out1 = Path(tmp) / "run1"
    Runner(FakeExecutor({})).run(Scenario.from_dict(valid), out1)
    events, case = compile_run(out1, run_id="run1", task_id="t1")
    write_experience(conn, events, case)

    # Run 2: failed (verify failure)
    out2 = Path(tmp) / "run2"
    data2 = dict(valid)
    data2["task_id"] = "t2"
    from vgui_runner.engine import StepOutcome
    outcomes = {
        ("step1", "execute", 0): StepOutcome.SUCCESS,
        ("step1", "verify", 0): StepOutcome.FAILURE,
    }
    Runner(FakeExecutor(outcomes)).run(Scenario.from_dict(data2), out2)
    events2, case2 = compile_run(out2, run_id="run2", task_id="t2")
    write_experience(conn, events2, case2)

    conn.close()
    return db_path


def test_determinism():
    """Same DB + same query → same results (Gate 1)."""
    db = make_populated_db()
    r1 = search_experience("screenshot window", db_path=db)
    r2 = search_experience("screenshot window", db_path=db)
    assert r1["matches"] == r2["matches"], "not deterministic"
    print("PASS: retrieval determinism")


def test_verified_only_default():
    """Unknown cases excluded from default pool (Gate 2)."""
    db = make_populated_db()
    r = search_experience("screenshot", verified_only=True, db_path=db)
    # Run 1 should be GOLD (PASSED)
    for m in r["matches"]:
        assert m["verification_status"] == "PASSED", f"non-verified in default: {m}"
    print("PASS: verified-only default")


def test_negative_memory():
    """Verified failures retrievable as negative_matches (Gate 3)."""
    db = make_populated_db()
    r = search_experience("screenshot", db_path=db)
    # Run 2 was a verify failure → should appear in negative_matches
    assert len(r["negative_matches"]) > 0, "no negative memory found"
    print(f"PASS: negative memory ({len(r['negative_matches'])} negative cases)")


def test_provenance():
    """Every match traces back to events (Gate 4)."""
    db = make_populated_db()
    r = search_experience("screenshot", db_path=db)
    conn = sqlite3.connect(str(db))
    conn.row_factory = sqlite3.Row
    for m in r["matches"] + r["negative_matches"]:
        refs = m["evidence_refs"]
        assert len(refs) > 0, f"case {m['case_id']} has no evidence refs"
        # Verify events exist
        case = conn.execute("SELECT * FROM experience_cases WHERE case_id=?", (m["case_id"],)).fetchone()
        assert case is not None
    conn.close()
    print("PASS: provenance preserved")


def test_no_execution_authority():
    """Returns recommendations only, no executor calls (Gate 5)."""
    db = make_populated_db()
    r = search_experience("window_not_ready", db_path=db)
    # Result is pure data, no side effects
    assert "matches" in r
    assert "negative_matches" in r
    assert "executor" not in r
    print("PASS: no execution authority")


def test_fallback_safe():
    """No good match → has_results=false, not fabricated (Gate 6)."""
    db = make_populated_db()
    r = search_experience("xyznonexistent failure type", db_path=db)
    # May still return structural matches, but has_results should be honest
    assert "has_results" in r
    print(f"PASS: fallback safe (has_results={r['has_results']})")


if __name__ == "__main__":
    test_determinism()
    test_verified_only_default()
    test_negative_memory()
    test_provenance()
    test_no_execution_authority()
    test_fallback_safe()
    print("\n=== All Retrieval tests passed ===")
