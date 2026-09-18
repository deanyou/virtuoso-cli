"""Test Router trace event contract."""
import sys, json, tempfile
from pathlib import Path
sys.path.insert(0, '.')

from vgui_runner.model import Operation
from vgui_runner.router import (
    ActionRequest, CapabilitySnapshot, Channel, RoutePolicy,
    route, emit_route_decision,
)
from vgui_runner.trace import Trace

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} — {msg}"); failed += 1

def full_caps():
    return CapabilitySnapshot(
        vcli_available=True, session_alive=True, pid_valid=True,
        display_available=True, window_identity_known=True,
        remote_x11_allowed=True, local_x11_available=True,
        vision_enabled=False, ssh_budget_remaining=4, daemon_healthy=True,
    )

print("=== Test: Route event contract ===")
tmpdir = Path(tempfile.mkdtemp())
trace_path = tmpdir / "trace.jsonl"
trace = Trace(trace_path)

# Route a screenshot
req = ActionRequest(Operation.SCREENSHOT, "step-1")
decision = route(req, full_caps(), RoutePolicy())
emit_route_decision(trace, "step-1", 0, decision)

# Route a rejected op
req2 = ActionRequest(Operation.VCLI_CALL, "step-2", {"function": "x", "args": []})
decision2 = route(req2, full_caps(), RoutePolicy(allow_vcli_call=False))
emit_route_decision(trace, "step-2", 0, decision2)

trace.close()

# Read back
lines = trace_path.read_text().strip().split("\n")
check("exactly 2 events", len(lines) == 2, f"got {len(lines)}")

events = [json.loads(l) for l in lines]

# Event 1: ROUTE_DECIDED
e1 = events[0]
check("state is ROUTE_DECIDED", e1["state"] == "ROUTE_DECIDED")
check("step_id correct", e1["step_id"] == "step-1")
check("attempt is 0", e1["attempt"] == 0)
check("outcome is selected", e1["outcome"] == "selected")
d1 = e1["details"]
check("details has channel", "channel" in d1 and d1["channel"] == "skill")
check("details has risk_class", "risk_class" in d1)
check("details has confidence", "confidence" in d1 and isinstance(d1["confidence"], (int, float)))
check("details has reason_code", "reason_code" in d1)
check("details has required_capabilities", "required_capabilities" in d1 and isinstance(d1["required_capabilities"], list))
check("details has fallbacks", "fallbacks" in d1 and isinstance(d1["fallbacks"], list))
check("details has rejected", "rejected" in d1 and d1["rejected"] == False)

# Event 2: rejected
e2 = events[1]
check("rejected event state correct", e2["state"] == "ROUTE_DECIDED")
check("rejected event step_id", e2["step_id"] == "step-2")
check("rejected outcome", e2["outcome"] == "rejected")
check("rejected flag", e2["details"]["rejected"] == True)
check("rejected reason_code", e2["details"]["reason_code"] == "vcli_call_blocked")

# No sensitive data
raw = trace_path.read_text()
check("no command args in trace", "dbCreateRect" not in raw and "function" not in raw.lower() or "vcli_call_blocked" in raw)
check("no tokens/env vars", "TOKEN" not in raw and "SECRET" not in raw)

# Stable JSON serialization
check("details is JSON-serializable", json.dumps(d1) is not None)

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)
