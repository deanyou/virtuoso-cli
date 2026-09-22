#!/usr/bin/env python3
"""Stability test: run core RSI operations 3 times, verify consistent results."""
import subprocess, sys, time, sqlite3
from pathlib import Path

PY = sys.executable
DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"
SCRIPTS = Path(__file__).parent

def run(args):
    t0 = time.time()
    r = subprocess.run([PY] + args, capture_output=True, text=True, timeout=30, cwd=str(SCRIPTS))
    dt = (time.time() - t0) * 1000
    return r.stdout, r.stderr, dt, r.returncode

def check(round_num, name, condition, detail=""):
    status = "PASS" if condition else "FAIL"
    print(f"  R{round_num} [{status}] {name} {detail}")
    return condition

def test_round(n):
    print(f"\n=== Round {n} ===")
    results = []

    # 1. --status
    out, err, dt, rc = run(["rsi.py", "--status"])
    results.append(check(n, "status", rc == 0 and "fnd" in out.lower(), f"({dt:.0f}ms)"))

    # 2. Search
    out, _, dt, rc = run(["rsi.py", "draw polygon", "-n", "1"])
    results.append(check(n, "search 'draw polygon'", "dbCreatePolygon" in out, f"({dt:.0f}ms)"))

    # 3. Exact lookup
    out, _, dt, rc = run(["rsi.py", "-f", "dbCreateRect"])
    results.append(check(n, "lookup dbCreateRect", "dbCreateRect" in out and "Syntax" in out, f"({dt:.0f}ms)"))

    # 4. Snippet list
    out, _, dt, rc = run(["rsi.py", "snippet", "list"])
    results.append(check(n, "snippet list", "draw_rect" in out, f"({dt:.0f}ms)"))

    # 5. Snippet search
    out, _, dt, rc = run(["rsi.py", "snippet", "search", "rect"])
    results.append(check(n, "snippet search rect", "draw_rect" in out, f"({dt:.0f}ms)"))

    # 6. Snippet info
    out, _, dt, rc = run(["rsi.py", "snippet", "info", "draw_rect"])
    results.append(check(n, "snippet info draw_rect", "draw_rect" in out, f"({dt:.0f}ms)"))

    # 7. Snippet pipeline
    out, _, dt, rc = run(["rsi.py", "snippet", "pipeline", "zoom_fit", "get_bbox"])
    results.append(check(n, "pipeline", "zoom_fit" in out and "get_bbox" in out, f"({dt:.0f}ms)"))

    # 8. Record list
    out, _, dt, rc = run(["rsi.py", "snippet", "record", "list"])
    results.append(check(n, "record list", "Recording" in out or "record" in out.lower(), f"({dt:.0f}ms)"))

    # 9. Version switch
    out, _, dt, rc = run(["rsi.py", "-f", "dbCreateRect", "-v", "IC618"])
    results.append(check(n, "version IC618", "IC618" in out, f"({dt:.0f}ms)"))

    # 10. rsi_query
    out, _, dt, rc = run(["rsi_query.py", "draw rect", "-n", "1"])
    results.append(check(n, "rsi_query draw rect", "dbCreateRect" in out, f"({dt:.0f}ms)"))

    # 11. DB consistency
    conn = sqlite3.connect(str(DB))
    fnd = conn.execute("SELECT COUNT(*) FROM fnd_functions").fetchone()[0]
    snip = conn.execute("SELECT COUNT(*) FROM snippets").fetchone()[0]
    conn.close()
    results.append(check(n, "DB counts", fnd == 28045 and snip >= 20, f"(fnd={fnd}, snip={snip})"))

    passed = sum(results)
    print(f"  Round {n}: {passed}/{len(results)} passed")
    return passed, len(results)

# Run 5 rounds
total_passed = 0
total_tests = 0
for i in range(1, 6):
    p, t = test_round(i)
    total_passed += p
    total_tests += t
    time.sleep(0.5)

print(f"\n{'='*50}")
print(f"STABILITY: {total_passed}/{total_tests} across 3 rounds")
if total_passed == total_tests:
    print("ALL STABLE ✓")
else:
    print(f"{total_tests - total_passed} failures")

