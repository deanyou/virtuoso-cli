"""Tests for VerifierResult model."""
import sys, json, tempfile
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.verifier_result import (
    VerifierResult, VerifyStatus, from_legacy_error, map_to_error_code, _redact,
)

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} — {msg}"); failed += 1

print("=== Test 1: Status semantics ===")
r = VerifierResult(predicate="window_exists", status=VerifyStatus.PASSED, expected=True, observed=True)
check("passed.ok is True", r.ok)
check("passed.is_failure is False", not r.is_failure)

r = VerifierResult(predicate="window_exists", status=VerifyStatus.FAILED, expected=True, observed=False)
check("failed.ok is False", not r.ok)
check("failed.is_failure is True", r.is_failure)

r = VerifierResult(predicate="window_exists", status=VerifyStatus.SKIPPED)
check("skipped.ok is False", not r.ok)
check("skipped not is_failure", not r.is_failure)

r = VerifierResult(predicate="ciw_eval", status=VerifyStatus.UNAVAILABLE)
check("unavailable.ok is False", not r.ok)
check("unavailable not is_failure (channel down)", not r.is_failure)

print("\n=== Test 2: Legacy adapter ===")
# None => passed
r = from_legacy_error("window_exists", expected="CIW", legacy_err=None)
check("None -> passed", r.status == VerifyStatus.PASSED)

# dict error -> failed
r = from_legacy_error("window_exists", expected="CIW", legacy_err={"error": "window not found"})
check("dict -> failed", r.status == VerifyStatus.FAILED)
check("reason_code extracted", r.reason_code == "window not found")

print("\n=== Test 3: Error code mapping ===")
r = VerifierResult(predicate="x", status=VerifyStatus.PASSED, expected=1, observed=1)
check("passed -> no error", map_to_error_code(r) is None)

r = VerifierResult(predicate="x", status=VerifyStatus.SKIPPED)
check("skipped -> no error", map_to_error_code(r) is None)

r = VerifierResult(predicate="x", status=VerifyStatus.FAILED)
check("failed -> ERROR_VERIFY", map_to_error_code(r) == "VERIFY_ERROR")

r = VerifierResult(predicate="x", status=VerifyStatus.UNAVAILABLE)
check("unavailable -> ERROR_VERIFY (backward compat)", map_to_error_code(r) == "VERIFY_ERROR")

print("\n=== Test 4: Sensitive redaction ===")
d = {"password": "secret123", "title": "CIW", "nested": {"token": "abc", "ok": True}}
redacted = _redact(d)
check("password redacted", redacted["password"] == "<redacted>")
check("token redacted", redacted["nested"]["token"] == "<redacted>")
check("title preserved", redacted["title"] == "CIW")
check("nested ok preserved", redacted["nested"]["ok"] is True)

print("\n=== Test 5: Trace serialization ===")
r = VerifierResult(
    predicate="ciw_eval",
    status=VerifyStatus.FAILED,
    expected="42",
    observed="41",
    reason_code="mismatch",
    evidence_refs=("screenshots/step-1-before.png", "screenshots/step-1-after.png"),
    step_id="step-1",
    attempt=0,
    route_event_seq=5,
)
details = r.to_trace_details()
check("JSON serializable", json.dumps(details) is not None)
check("predicate in details", details["predicate"] == "ciw_eval")
check("status in details", details["status"] == "failed")
check("evidence_refs is list", isinstance(details["evidence_refs"], list))
check("step_id linked", details["step_id"] == "step-1")
check("route_event_seq linked", details["route_event_seq"] == 5)

print("\n=== Test 6: Retry independence ===")
results = []
for attempt in range(3):
    r = VerifierResult(
        predicate="window_exists",
        status=VerifyStatus.FAILED if attempt < 2 else VerifyStatus.PASSED,
        expected=True,
        observed=False if attempt < 2 else True,
        step_id="step-1",
        attempt=attempt,
    )
    results.append(r)
check("3 independent results", len(results) == 3)
check("attempt 0 failed", results[0].status == VerifyStatus.FAILED)
check("attempt 2 passed", results[2].status == VerifyStatus.PASSED)
check("attempt field distinct", results[0].attempt == 0 and results[2].attempt == 2)

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
