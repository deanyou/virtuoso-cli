"""Simple test runner for router (no pytest dependency)."""
import sys
sys.path.insert(0, '.')

from vgui_runner.model import Operation
from vgui_runner.router import (
    ActionRequest, CapabilitySnapshot, Channel, RoutePolicy, RiskClass, route,
)

passed = 0
failed = 0

def check(name, condition, msg=""):
    global passed, failed
    if condition:
        print(f"  PASS: {name}")
        passed += 1
    else:
        print(f"  FAIL: {name} — {msg}")
        failed += 1

def full_caps():
    return CapabilitySnapshot(
        vcli_available=True, session_alive=True, pid_valid=True,
        display_available=True, window_identity_known=True,
        remote_x11_allowed=True, local_x11_available=True,
        vision_enabled=False, ssh_budget_remaining=4, daemon_healthy=True,
    )

print("=== Test 1: Stable routing ===")
req = ActionRequest(Operation.SCREENSHOT, "step1")
d1 = route(req, full_caps(), RoutePolicy())
d2 = route(req, full_caps(), RoutePolicy())
check("same input -> same channel", d1.channel == d2.channel == Channel.SKILL)

print("\n=== Test 2: VCLI down fallback ===")
req = ActionRequest(Operation.CLICK_REL, "step1", {"x": 10, "y": 20})
caps = CapabilitySnapshot(
    vcli_available=True, session_alive=True, pid_valid=True,
    display_available=True, window_identity_known=True,
    remote_x11_allowed=True, local_x11_available=False,
    vision_enabled=False, ssh_budget_remaining=4, daemon_healthy=True,
)
d = route(req, caps, RoutePolicy())
check("GUI op routes to vcli_x11", d.channel == Channel.VCLI_X11, f"got {d.channel}")

print("\n=== Test 3: No window identity rejects ===")
caps_no_id = CapabilitySnapshot(
    vcli_available=True, session_alive=True, pid_valid=True,
    display_available=True, window_identity_known=False,
    remote_x11_allowed=True, local_x11_available=True,
    vision_enabled=False, ssh_budget_remaining=4, daemon_healthy=True,
)
d = route(req, caps_no_id, RoutePolicy(require_window_identity=True))
check("rejected without identity", d.channel == Channel.REJECTED, f"got {d.channel}: {d.reason}")

print("\n=== Test 4: Risk classification ===")
d = route(ActionRequest(Operation.SCREENSHOT, "s"), full_caps(), RoutePolicy())
check("screenshot is read_only", d.risk_class == RiskClass.READ_ONLY)
d = route(ActionRequest(Operation.CLICK_REL, "c", {"x":1,"y":1}), full_caps(), RoutePolicy())
check("click is non_idempotent", d.risk_class == RiskClass.NON_IDEMPOTENT_WRITE)
d = route(ActionRequest(Operation.CLOSE, "cl"), full_caps(), RoutePolicy())
check("close is destructive", d.risk_class == RiskClass.DESTRUCTIVE)

print("\n=== Test 5: Vision opt-in ===")
caps_minimal = CapabilitySnapshot(
    vcli_available=False, session_alive=False, pid_valid=False,
    display_available=True, window_identity_known=True,
    remote_x11_allowed=False, local_x11_available=False,
    vision_enabled=True, ssh_budget_remaining=0, daemon_healthy=False,
)
d = route(req, caps_minimal, RoutePolicy(allow_vision=False))
check("rejected without vision opt-in", d.channel == Channel.REJECTED)
d = route(req, caps_minimal, RoutePolicy(allow_vision=True))
check("vision allowed with opt-in", d.channel == Channel.VISION)

print("\n=== Test 6: SSH budget exhausted ===")
caps_no_budget = CapabilitySnapshot(
    vcli_available=True, session_alive=True, pid_valid=True,
    display_available=True, window_identity_known=True,
    remote_x11_allowed=True, local_x11_available=False,
    vision_enabled=False, ssh_budget_remaining=0, daemon_healthy=True,
)
d = route(req, caps_no_budget, RoutePolicy())
check("rejected with SSH budget 0", d.channel == Channel.REJECTED, f"got {d.channel}")
check("reason mentions budget", "budget" in d.reason.lower() or "ssh" in d.reason.lower(), d.reason)

print("\n=== Test 7: VCLI_CALL guard ===")
req_vcli = ActionRequest(Operation.VCLI_CALL, "vc", {"function": "dbCreateRect", "args": []})
d = route(req_vcli, full_caps(), RoutePolicy(allow_vcli_call=False))
check("VCLI_CALL rejected by default", d.channel == Channel.REJECTED)
d = route(req_vcli, full_caps(), RoutePolicy(allow_vcli_call=True))
check("VCLI_CALL allowed with opt-in", d.channel == Channel.SKILL)

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
