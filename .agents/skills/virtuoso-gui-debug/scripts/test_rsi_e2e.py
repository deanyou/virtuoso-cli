#!/usr/bin/env python3
"""End-to-end RSI workflow test: query → call → record."""
import subprocess, json, sys, time
from pathlib import Path

# Add scripts dir to path
sys.path.insert(0, str(Path(__file__).parent))
from rsi_query import get_conn, get_function

SSH = ["ssh", "-i", str(Path.home() / ".ssh" / "id_rsa"),
       "user1@192.168.1.111"]
SESSION = "dean-user1-37749"

def vcli(skill_expr):
    escaped = skill_expr.replace('"', '\\"')
    cmd = SSH + [
        f'export HOME=/home/user1; export PATH=/usr/bin:/bin:/home/user1/.local/bin:$PATH; '
        f'vcli --session {SESSION} skill exec "{escaped}" 2>/dev/null'
    ]
    r = subprocess.run(cmd, capture_output=True, timeout=15)
    out = r.stdout.decode('utf-8', errors='replace')
    idx = out.find('{')
    if idx < 0:
        return None, out[:200], []
    data = json.loads(out[idx:])
    return data.get("status"), data.get("output", ""), data.get("errors", [])

def main():
    conn = get_conn()

    # Scenario: user wants to "draw a path"
    print("=== RSI E2E Test: draw a path ===")
    print()

    # Step 1: Query RSI
    print("Step 1: Query RSI for 'draw path'...")
    from rsi_query import search
    results = search(conn, "draw path", "IC251", 5)
    for i, r in enumerate(results[:3]):
        print(f"  {i+1}. {r['name']}: {r['description'][:60]}")

    # Step 2: Pick dbCreatePath
    path_func = get_function(conn, "dbCreatePath", "IC251")
    print(f"\nStep 2: Selected dbCreatePath")
    print(f"  Signature: {path_func['syntax']}")

    # Step 3: Call it
    print(f"\nStep 3: Execute...")
    expr = 'dbCreatePath(geGetEditCellView() list("M1" "drawing") list(60:0 80:20 100:0) 2.0)'
    status, output, errors = vcli(expr)
    print(f"  status={status}, output={output[:50]}")
    if errors:
        print(f"  errors={errors[0][:80]}")

    # Step 4: Query another function — "delete all"
    print(f"\n=== RSI E2E Test: delete shape ===")
    results = search(conn, "delete shape", "IC251", 5)
    for i, r in enumerate(results[:3]):
        print(f"  {i+1}. {r['name']}: {r['description'][:60]}")

    # Step 5: Query "save cellview"
    print(f"\n=== RSI E2E Test: save ===")
    results = search(conn, "save cellview", "IC251", 5)
    for i, r in enumerate(results[:3]):
        print(f"  {i+1}. {r['name']}: {r['description'][:60]}")

    # Step 6: Test exact lookup with known errors
    print(f"\n=== RSI: known errors check ===")
    for fname in ["dbCreateRect", "dbCreatePolygon", "dbOpenCellView"]:
        func = get_function(conn, fname, "IC251")
        if func:
            print(f"  {fname}: {func['syntax'][:70]}")

    conn.close()
    print("\n=== E2E test complete ===")

if __name__ == "__main__":
    main()
