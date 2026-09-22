#!/usr/bin/env python3
"""Batch verify IC251 functions against remote Virtuoso.

Tests functions via `funcName()` call over SSH:
- If returns without "undefined" -> verified
- If "undefined" -> not_exist
- If "too few arguments" -> exists but signature differs
"""
import sqlite3, subprocess, sys, time
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"
SSH = ["ssh", "-i", str(Path.home() / ".ssh" / "id_rsa"),
       "user1@192.168.1.111",
       'export HOME=/home/user1; export PATH=/usr/bin:/bin:/home/user1/.local/bin:$PATH; '
       'vcli --session dean-user1-37749 skill exec "{}" 2>/dev/null']

def check_func(name):
    """Call funcName() and return status."""
    cmd = SSH[0]
    full = [SSH[0]] + SSH[1:]
    full[-1] = full[-1].format(f"{name}()")
    try:
        r = subprocess.run(full, capture_output=True, timeout=15)
        out = r.stdout.decode('utf-8', errors='replace')
        idx = out.find('{')
        if idx < 0:
            return 'error', out[:100]
        import json
        data = json.loads(out[idx:])
        output = data.get('output', '')
        errors = data.get('errors', [])
        if 'undefined' in output.lower() or 'undefined' in str(errors).lower():
            return 'not_exist', output[:100]
        if 'too few' in output.lower() or 'too many' in output.lower() or 'argument' in output.lower():
            return 'exists', output[:100]
        if data.get('status') == 'success':
            return 'verified', output[:100]
        return 'exists', output[:100]
    except Exception as e:
        return 'timeout', str(e)[:100]

def main():
    conn = sqlite3.connect(str(DB))
    # Get IC251 functions not yet verified (confidence='ported')
    funcs = [r[0] for r in conn.execute(
        "SELECT name FROM fnd_functions WHERE version='IC251' "
        "AND confidence='ported' ORDER BY name LIMIT 200"
    ).fetchall()]
    print(f"Verifying {len(funcs)} IC251 functions...")

    stats = {'verified': 0, 'exists': 0, 'not_exist': 0, 'error': 0, 'timeout': 0}
    for i, name in enumerate(funcs, 1):
        status, detail = check_func(name)
        stats[status] += 1
        conn.execute(
            "UPDATE fnd_functions SET confidence=? WHERE name=? AND version='IC251'",
            (status, name))
        if i % 20 == 0 or status == 'not_exist':
            print(f"  [{i}/{len(funcs)}] {name}: {status}")
        time.sleep(0.1)  # don't hammer SSH
    conn.commit()
    conn.close()

    print(f"\nResults: {stats}")
    total = sum(stats.values())
    print(f"Verified {stats['verified']+stats['exists']}/{total} exist in IC251")

if __name__ == '__main__':
    main()
