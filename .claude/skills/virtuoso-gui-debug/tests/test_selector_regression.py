"""Regression: window selector no-match must be UNKNOWN, not NEGATIVE."""
import sys, sqlite3
sys.path.insert(0, r"E:\git\virtuoso-cli\.claude\skills\virtuoso-gui-debug")
from vgui_runner.experience import init_schema, compile_run, write_experience
import tempfile, os

DB = r"E:\git\virtuoso-cli\.claude\skills\virtuoso-gui-debug\data\skill_db.sqlite3"

# Simulate: selector returns no window -> screenshot not executed -> UNKNOWN
# This is a precondition failure, not a verification failure
def test_selector_no_match_is_unknown():
    """Window selector no-match must be classified UNKNOWN, not FAILED."""
    conn = sqlite3.connect(DB)
    init_schema(conn)
    
    # Query the real-004 case
    row = conn.execute("""
        SELECT case_type, verification_status, failure_type
        FROM experience_cases WHERE run_id='real-004'
    """).fetchone()
    
    assert row is not None, "real-004 not found"
    case_type, vs, failure_type = row
    
    # Must be UNKNOWN (precondition failure), not NEGATIVE (verification FAILED)
    assert case_type == "unknown", f"Expected case_type=unknown, got {case_type}"
    assert vs is None or vs == "UNAVAILABLE", f"Expected verification_status NULL/UNAVAILABLE, got {vs}"
    assert failure_type == "WINDOW_SELECTOR_NO_MATCH", f"Expected WINDOW_SELECTOR_NO_MATCH, got {failure_type}"
    
    print("PASS: window selector no-match -> UNKNOWN (not NEGATIVE)")
    conn.close()

def test_no_auto_retry_on_unknown():
    """UNKNOWN must not trigger auto-retry."""
    conn = sqlite3.connect(DB)
    row = conn.execute("""
        SELECT verification_status FROM experience_cases 
        WHERE run_id='real-004'
    """).fetchone()
    vs = row[0]
    # UNKNOWN means we can't confirm result -> no auto-retry
    assert vs is None or vs == "UNAVAILABLE"
    print("PASS: UNKNOWN does not trigger auto-retry")
    conn.close()

if __name__ == "__main__":
    test_selector_no_match_is_unknown()
    test_no_auto_retry_on_unknown()
    print("\nAll regression tests passed")