#!/usr/bin/env python3
"""
RSI validation: test fnd_functions against live IC251.
Strategy: for each function, call it with nil args and check if it exists
(returns "undefined function" error vs argument error).
"""
import sqlite3, subprocess, json, time, sys
from pathlib import Path

DB = Path(r"E:\git\virtuoso-cli\.claude\skills\virtuoso-gui-debug\data\skill_db.sqlite3")
SSH = ["ssh", "-i", str(Path.home() / ".ssh" / "id_rsa"),
       "-o", "ConnectTimeout=5",
       "user1@192.168.1.111"]
SESSION = "dean-user1-37749"

def vcli_exec(skill_expr):
    """Execute SKILL via SSH+vcli, return (status, output, errors)."""
    cmd = SSH + [
        f'export HOME=/home/user1; export PATH=/usr/bin:/bin:/home/user1/.local/bin:$PATH; '
        f'vcli --session {SESSION} skill exec "{skill_expr}" 2>/dev/null'
    ]
    try:
        r = subprocess.run(cmd, capture_output=True, timeout=15)
        out = r.stdout.decode('utf-8', errors='replace').strip()
        # Find JSON in output (skip INFO lines)
        json_start = out.find('{')
        if json_start < 0:
            return ("no_json", out[:200], [])
        data = json.loads(out[json_start:])
        return (data.get("status", "?"), data.get("output", ""), data.get("errors", []))
    except subprocess.TimeoutExpired:
        return ("timeout", "", [])
    except Exception as e:
        return ("error", str(e)[:200], [])

def function_exists(name):
    """Check if function exists by calling it with nil."""
    # Try calling with nil args - if function exists, we get arg error;
    # if not, we get "undefined function"
    status, output, errors = vcli_exec(f"{name}(nil)")
    if status == "success":
        return True, output[:200]
    err_str = " ".join(errors) if errors else output
    if "undefined function" in err_str.lower():
        return False, err_str[:200]
    if status == "error":
        # Function exists but wrong args
        return True, err_str[:200]
    return None, err_str[:200]  # unknown

def main():
    conn = sqlite3.connect(str(DB))
    conn.row_factory = sqlite3.Row

    # Pick layout-related functions from IC251
    rows = conn.execute("""
        SELECT name, syntax, category FROM fnd_functions
        WHERE version='IC251'
        AND (name LIKE 'db%' OR name LIKE 'le%' OR name LIKE 'ge%')
        AND category IN ('Custom_Layout', 'DFII_SKILL')
        ORDER BY name
        LIMIT 50
    """).fetchall()

    print(f"Testing {len(rows)} functions from fnd_functions (IC251)...")
    print("=" * 60)

    exists_count = 0
    not_exists_count = 0
    unknown_count = 0
    results = []

    for i, row in enumerate(rows):
        name = row["name"]
        exists, detail = function_exists(name)
        if exists is True:
            exists_count += 1
            tag = "OK"
        elif exists is False:
            not_exists_count += 1
            tag = "MISS"
        else:
            unknown_count += 1
            tag = "???"

        results.append({"name": name, "exists": exists, "detail": detail, "category": row["category"]})
        print(f"[{i+1:3d}/{len(rows)}] {tag:4s} {name:30s} {detail[:80]}")

        # Small delay to avoid SSH flooding
        time.sleep(0.2)

    print("=" * 60)
    print(f"Results: {exists_count} exist, {not_exists_count} missing, {unknown_count} unknown")
    print(f"Accuracy: {exists_count/(exists_count+not_exists_count)*100:.1f}%" if (exists_count+not_exists_count) > 0 else "N/A")

    # Record results
    for r in results:
        conn.execute("""INSERT OR REPLACE INTO functions
            (name, category, func_exists, signature, confidence, version, notes)
            VALUES (?, ?, ?, ?, ?, ?, ?)""",
            (r["name"], r["category"], r["exists"], None,
             "verified" if r["exists"] else "missing",
             "IC251", r["detail"][:200]))
    conn.commit()
    conn.close()
    print("\nResults saved to functions table.")

if __name__ == "__main__":
    main()
