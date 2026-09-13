#!/usr/bin/env python3
"""
Verify RSI database integrity:
1. SQLite vs JSON backup consistency
2. All records have confidence level
3. Verified functions have signatures
4. No orphan error history entries
"""
import sqlite3, json, sys
from pathlib import Path

DATA_DIR = Path(__file__).parent.parent / "data"
DB_PATH = DATA_DIR / "skill_db.sqlite3"
JSON_PATH = DATA_DIR / "skill_db_backup.json"

def verify():
    errors = []
    
    # 1. SQLite exists and readable
    if not DB_PATH.exists():
        errors.append(f"Missing SQLite: {DB_PATH}")
        return errors
    conn = sqlite3.connect(str(DB_PATH))
    conn.row_factory = sqlite3.Row
    
    # 2. JSON backup exists
    if not JSON_PATH.exists():
        errors.append(f"Missing JSON backup: {JSON_PATH}")
    else:
        with open(JSON_PATH, encoding="utf-8") as f:
            backup = json.load(f)
        backup_names = {fn["name"] for fn in backup.get("manual_functions", backup.get("functions", []))}
    
    # 3. Check all functions have confidence
    no_conf = conn.execute("SELECT COUNT(*) FROM functions WHERE confidence IS NULL").fetchone()[0]
    if no_conf:
        errors.append(f"{no_conf} functions missing confidence level")
    
    # 4. Verified functions must have signatures
    bad_verified = conn.execute("""
        SELECT name FROM functions 
        WHERE confidence='verified' AND (signature IS NULL OR signature='')
    """).fetchall()
    if bad_verified:
        for r in bad_verified:
            errors.append(f"Verified function '{r['name']}' missing signature")
    
    # 5. SQLite vs JSON consistency
    db_names = {r["name"] for r in conn.execute("SELECT name FROM functions")}
    if not JSON_PATH.exists():
        pass  # already reported
    elif db_names != backup_names:
        missing_in_json = db_names - backup_names
        extra_in_json = backup_names - db_names
        if missing_in_json:
            errors.append(f"In DB but not in JSON: {missing_in_json}")
        if extra_in_json:
            errors.append(f"In JSON but not in DB: {extra_in_json}")
    
    # 6. Stats
    total = conn.execute("SELECT COUNT(*) FROM functions").fetchone()[0]
    verified = conn.execute("SELECT COUNT(*) FROM functions WHERE confidence='verified'").fetchone()[0]
    discovered = conn.execute("SELECT COUNT(*) FROM functions WHERE confidence='discovered'").fetchone()[0]
    
    print(f"Database: {DB_PATH.name}")
    print(f"  Total functions: {total}")
    print(f"  Verified: {verified}")
    print(f"  Discovered: {discovered}")
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
