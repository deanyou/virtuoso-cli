#!/usr/bin/env python3
"""Export/import RSI knowledge as JSON for team sharing."""
import sqlite3, json, sys
from pathlib import Path
from datetime import datetime

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"

def export(path):
    conn = sqlite3.connect(str(DB))
    conn.row_factory = sqlite3.Row
    data = {
        "version": "1.0",
        "exported": datetime.now().isoformat(),
        "synonyms": [dict(r) for r in conn.execute("SELECT canonical, variant FROM synonyms")],
        "snippets": [dict(r) for r in conn.execute(
            "SELECT name, description, category, code, params, version, success_count, fail_count FROM snippets")],
        "error_history": [dict(r) for r in conn.execute(
            "SELECT function_name, error_type, error_message, fixed, created_at FROM error_history WHERE fixed=0")],
        "param_examples": [dict(r) for r in conn.execute(
            "SELECT function_name, param_name, example_value, success_count FROM param_examples")],
        "verified_functions": [dict(r) for r in conn.execute(
            "SELECT name, version, confidence, syntax FROM fnd_functions WHERE confidence IN ('verified','exists','not_exist')")],
    }
    conn.close()
    Path(path).write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"Exported: {path}")
    print(f"  synonyms: {len(data['synonyms'])}")
    print(f"  snippets: {len(data['snippets'])}")
    print(f"  errors: {len(data['error_history'])}")
    print(f"  params: {len(data['param_examples'])}")
    print(f"  verified: {len(data['verified_functions'])}")

def import_data(path):
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    conn = sqlite3.connect(str(DB))
    # Merge synonyms
    for s in data.get("synonyms", []):
        conn.execute("INSERT OR IGNORE INTO synonyms (canonical, variant) VALUES (?, ?)",
                     (s["canonical"], s["variant"]))
    # Merge verified functions
    for v in data.get("verified_functions", []):
        conn.execute("""UPDATE fnd_functions SET confidence=?, syntax=?
            WHERE name=? AND version=?""",
            (v["confidence"], v["syntax"], v["name"], v["version"]))
    conn.commit()
    print(f"Imported: {path}")
    print(f"  synonyms: {len(data.get('synonyms',[]))}")
    print(f"  verified: {len(data.get('verified_functions',[]))}")
    conn.close()

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("Usage: rsi_export.py <export|import> <file.json>")
        sys.exit(1)
    if sys.argv[1] == "export":
        export(sys.argv[2])
    else:
        import_data(sys.argv[2])
