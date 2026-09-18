"""Tests for template-based Planner."""
import sys
sys.path.insert(0, '.')

from vgui_runner.planner import Planner, TEMPLATES
from vgui_runner.router import RiskClass

passed = failed = 0
def check(name, cond, msg=""):
    global passed, failed
    if cond:
        print(f"  PASS: {name}"); passed += 1
    else:
        print(f"  FAIL: {name} — {msg}"); failed += 1


planner = Planner()

print("=== Test 1: List available templates ===")
templates = planner.available_templates()
check("has 4 templates", len(templates) == 4, f"got {len(templates)}")
names = [t["name"] for t in templates]
check("screenshot_window", "screenshot_window" in names)
check("key_press", "key_press" in names)
check("ciw_command", "ciw_command" in names)
check("window_close", "window_close" in names)

print("\n=== Test 2: screenshot_window template ===")
s = planner.plan("screenshot_window", task_id="t1", session_id="s1",
                 pid=1, display=":0", cellview={"lib": "L", "cell": "C", "view": "layout"},
                 window_title="CIW")
check("2 steps", len(s.steps) == 2)
check("step 1 activate", s.steps[0].operation.value == "WINDOW_ACTIVATE")
check("step 2 screenshot", s.steps[1].operation.value == "SCREENSHOT")
check("window_title filled", s.steps[0].arguments["window_title"] == "CIW")

print("\n=== Test 3: Missing required param raises ===")
try:
    planner.plan("key_press", task_id="t", session_id="s", pid=1, display=":0",
                 cellview={}, key="Return")  # missing window_title
    check("should raise", False, "no exception")
except ValueError as e:
    check("raises ValueError", "window_title" in str(e))

print("\n=== Test 4: Unknown template raises ===")
try:
    planner.plan("nonexistent", task_id="t", session_id="s", pid=1, display=":0",
                 cellview={})
    check("should raise", False)
except ValueError as e:
    check("raises on unknown", "Unknown template" in str(e))

print("\n=== Test 5: Risk class preserved ===")
check("screenshot is read_only", TEMPLATES["screenshot_window"].risk_class == RiskClass.READ_ONLY)
check("close is destructive", TEMPLATES["window_close"].risk_class == RiskClass.DESTRUCTIVE)
check("ciw is non_idempotent", TEMPLATES["ciw_command"].risk_class == RiskClass.NON_IDEMPOTENT_WRITE)

print("\n=== Test 6: Non-idempotent template has no auto-retry ===")
s = planner.plan("ciw_command", task_id="t", session_id="s", pid=1, display=":0",
                 cellview={"lib": "L", "cell": "C", "view": "layout"}, command="printf(\"hi\\n\")")
check("max_retries=0 for non_idempotent", s.steps[0].max_retries == 0)

print("\n=== Test 7: Destructive template requires explicit opt-in ===")
s = planner.plan("window_close", task_id="t", session_id="s", pid=1, display=":0",
                 cellview={"lib": "L", "cell": "C", "view": "layout"}, window_id="12345")
check("close has verifier expecting False", s.steps[0].verifier["expected"] == False)
check("close has no retry", s.steps[0].max_retries == 0)

print(f"\n=== Results: {passed} passed, {failed} failed ===")
sys.exit(1 if failed else 0)


