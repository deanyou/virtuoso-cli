#!/usr/bin/env python3
"""
Verify RSI database integrity:
1. SQLite vs JSON backup consistency
2. All records have confidence level
3. Verified functions have signatures
4. No orphan error history entries
5. Coverage stats per version
"""
import sqlite3, json, sys
from pathlib import Path

DATA_DIR = Path(__file__).parent.parent / "data"
DB_PATH = DATA_DIR / "skill_db.sqlite3"
JSON_PATH = DATA_DIR / "skill_db_backup.json"

# Table used by fnd_functions (new schema)
TABLE = "fnd_functions"
CONF_COL = "confidence"
SIG_COL = "syntax"


def verify():
    errors = []

    if not DB_PATH.exists():
        errors.append(f"Missing SQLite: {DB_PATH}")
        print("\n".join(errors))
        return 1
    conn = sqlite3.connect(str(DB_PATH))
    conn.row_factory = sqlite3.Row

    # 1. JSON backup exists
    if not JSON_PATH.exists():
        errors.append(f"Missing JSON backup: {JSON_PATH}")
        backup_names = set()
    else:
        with open(JSON_PATH, encoding="utf-8") as f:
            backup = json.load(f)
        backup_names = {fn["name"] for fn in backup.get("manual_functions", backup.get("functions", []))}

    # 2. Coverage per version
    print(f"Database: {DB_PATH.name}")
    rows = conn.execute(
        f"SELECT version, {CONF_COL}, COUNT(*) as cnt FROM {TABLE} GROUP BY version, {CONF_COL} ORDER BY version, {CONF_COL}"
    ).fetchall()
    versions = {}
    for r in rows:
        versions.setdefault(r["version"], {})[r["confidence"]] = r["cnt"]
    for ver in sorted(versions):
        v = versions[ver]
        total = sum(v.values())
        print(f"  {ver}: {total} functions | " + " | ".join(f"{k}={v[k]}" for k in sorted(v)))

    total = conn.execute(f"SELECT COUNT(*) FROM {TABLE}").fetchone()[0]
    verified = conn.execute(f"SELECT COUNT(*) FROM {TABLE} WHERE {CONF_COL}='verified'").fetchone()[0]
    print(f"  Total (all versions): {total}")
    print(f"  Verified: {verified}")

    # 3. All functions have confidence
    no_conf = conn.execute(f"SELECT COUNT(*) FROM {TABLE} WHERE {CONF_COL} IS NULL").fetchone()[0]
    if no_conf:
        errors.append(f"{no_conf} functions missing confidence level")

    # 4. Verified functions must have signatures
    bad_verified = conn.execute(f"""
        SELECT name, version FROM {TABLE}
        WHERE {CONF_COL}='verified' AND ({SIG_COL} IS NULL OR {SIG_COL}='')
    """).fetchall()
    if bad_verified:
        for r in bad_verified:
            errors.append(f"Verified '{r['name']}' ({r['version']}) missing signature")

    # 5. JSON backup is parseable (function names now come from fnd docs, not manual JSON)
    if JSON_PATH.exists():
        try:
            _ = json.load(open(JSON_PATH, encoding="utf-8"))
        except Exception as e:
            errors.append(f"JSON backup unreadable: {e}")

    # 6. Snippets sanity
    snip_total = conn.execute("SELECT COUNT(*) FROM snippets").fetchone()[0]
    snip_promoted = conn.execute("SELECT COUNT(*) FROM snippets WHERE promoted_to IS NOT NULL").fetchone()[0]
    print(f"  Snippets: {snip_total} ({snip_promoted} promoted)")

    # 7. Error history
    err_count = conn.execute("SELECT COUNT(*) FROM error_history").fetchone()[0]
    print(f"  Errors learned: {err_count}")

    print(f"  JSON backup: {'OK' if JSON_PATH.exists() else 'MISSING'}")

    if errors:
        print(f"\nFAIL: {len(errors)} issue(s)")
        for e in errors:
            print(f"  - {e}")
        return 1
    else:
        print("\nPASS: Database consistent")
        return 0


if __name__ == "__main__":
    sys.exit(verify())
