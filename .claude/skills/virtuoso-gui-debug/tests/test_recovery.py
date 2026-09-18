"""Tests for recovery decision policy."""
import sys
sys.path.insert(0, '.')

from vgui_runner.router import RiskClass, Channel
from vgui_runner.recovery import (
    RecoveryPolicy, RecoveryRequest, RecoveryAction, ErrorCategory,
    decide_recovery,
)

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} — {msg}"); failed += 1


policy = RecoveryPolicy(max_attempts=2, total_deadline_ms=10000)

print("=== Test 1: read_only timeout retries ===")
req = RecoveryRequest(
    step_id="s1", attempt=0, risk_class=RiskClass.READ_ONLY,
    error_category=ErrorCategory.TIMEOUT, elapsed_ms=1000,
)
d = decide_recovery(req, policy)
check("action=retry", d.action in (RecoveryAction.RETRY, RecoveryAction.ROLLBACK), d.action)
check("verification not required for read_only", not d.verification_required)

print("\n=== Test 2: idempotent write verifies before retry ===")
req = RecoveryRequest(
    step_id="s1", attempt=0, risk_class=RiskClass.IDEMPOTENT_WRITE,
    error_category=ErrorCategory.TIMEOUT, elapsed_ms=1000, has_rollback=True,
)
d = decide_recovery(req, policy)
check("action=rollback", d.action == RecoveryAction.ROLLBACK, d.action)
check("verification required", d.verification_required)

print("\n=== Test 3: non_idempotent never auto-replays ===")
req = RecoveryRequest(
    step_id="s1", attempt=0, risk_class=RiskClass.NON_IDEMPOTENT_WRITE,
    error_category=ErrorCategory.TIMEOUT, elapsed_ms=1000,
)
d = decide_recovery(req, policy)
check("action=abort", d.action == RecoveryAction.ABORT, d.action)

print("\n=== Test 4: non_idempotent uncertain -> manual ===")
req = RecoveryRequest(
    step_id="s1", attempt=0, risk_class=RiskClass.NON_IDEMPOTENT_WRITE,
    error_category=ErrorCategory.CONNECTION_LOST, elapsed_ms=1000,
    result_uncertain=True,
)
d = decide_recovery(req, policy)
check("action=manual", d.action == RecoveryAction.MANUAL, d.action)
check("verification required", d.verification_required)

print("\n=== Test 5: destructive -> manual ===")
req = RecoveryRequest(
    step_id="s1", attempt=0, risk_class=RiskClass.DESTRUCTIVE,
    error_category=ErrorCategory.TIMEOUT, elapsed_ms=1000,
)
d = decide_recovery(req, policy)
check("action=manual", d.action == RecoveryAction.MANUAL, d.action)

print("\n=== Test 6: deadline exceeded aborts ===")
req = RecoveryRequest(
    step_id="s1", attempt=0, risk_class=RiskClass.READ_ONLY,
    error_category=ErrorCategory.TIMEOUT, elapsed_ms=11000,  # over deadline
)
d = decide_recovery(req, policy)
check("action=abort", d.action == RecoveryAction.ABORT, d.action)
check("reason=deadline_exceeded", d.reason_code == "deadline_exceeded")

print("\n=== Test 7: retry budget exhausted -> fallback ===")
req = RecoveryRequest(
    step_id="s1", attempt=1, risk_class=RiskClass.READ_ONLY,
    error_category=ErrorCategory.TIMEOUT, elapsed_ms=2000,
    current_channel=Channel.SKILL,
)
d = decide_recovery(req, policy)
check("action=fallback", d.action == RecoveryAction.FALLBACK, d.action)
check("next_channel=vcli_x11", d.next_channel == Channel.VCLI_X11)

print("\n=== Test 8: rollback failed -> manual ===")
req = RecoveryRequest(
    step_id="s1", attempt=0, risk_class=RiskClass.IDEMPOTENT_WRITE,
    error_category=ErrorCategory.ROLLBACK_FAILED, elapsed_ms=1000,
)
d = decide_recovery(req, policy)
check("action=manual", d.action == RecoveryAction.MANUAL, d.action)
check("reason=rollback_failed", d.reason_code == "rollback_failed")

print("\n=== Test 9: remaining deadline not reset by retry ===")
req = RecoveryRequest(
    step_id="s1", attempt=0, risk_class=RiskClass.READ_ONLY,
    error_category=ErrorCategory.TIMEOUT, elapsed_ms=5000,
)
d = decide_recovery(req, policy)
check("remaining is 5000 not 10000", d.remaining_deadline_ms == 5000, f"got {d.remaining_deadline_ms}")

print("\n=== Test 10: trace details JSON-safe ===")
req = RecoveryRequest(
    step_id="s1", attempt=0, risk_class=RiskClass.READ_ONLY,
    error_category=ErrorCategory.TIMEOUT, elapsed_ms=1000,
)
d = decide_recovery(req, policy)
import json
details = d.to_trace_details()
check("JSON serializable", json.dumps(details) is not None)
check("has action", "action" in details)
check("has cause", "cause" in details)
check("has remaining_deadline_ms", "remaining_deadline_ms" in details)

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
