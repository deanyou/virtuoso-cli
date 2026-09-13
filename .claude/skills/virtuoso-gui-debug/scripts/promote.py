#!/usr/bin/env python3
"""
Promote high-frequency snippets to SKILL .il functions.

Reads snippets from SQLite, generates procedure definitions,
and writes them to a .il file that can be loaded in Virtuoso.
"""
import sqlite3, json, re
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"
IL_OUT = Path(__file__).parent.parent / "data" / "vcli_snippets.il"

# Snippets to promote (name in DB → SKILL procedure name)
# Convention: vcliCamelCase(name)
PROMOTE = {
    "draw_rect": "vcliDrawRect",
    "draw_polygon": "vcliDrawPolygon",
    "draw_path": "vcliDrawPath",
    "draw_line": "vcliDrawLine",
    "save_cv": "vcliSaveCv",
    "zoom_fit": "vcliZoomFit",
    "select_all": "vcliSelectAll",
    "deselect_all": "vcliDeselectAll",
    "delete_selected": "vcliDeleteSelected",
    "list_shapes": "vcliListShapes",
}


def name_to_proc(name):
    """draw_rect → vcliDrawRect"""
    parts = name.split("_")
    return "vcli" + "".join(p.capitalize() for p in parts)


def generate_il(snippets, conn):
    """Generate real SKILL procedure from snippet code."""
    lines = [
        "; vcli_snippets.il — Auto-promoted from RSI knowledge base",
        "; Load in Virtuoso: load(\"<path>/vcli_snippets.il\")",
        "",
    ]
    for db_name, proc_name in snippets:
        row = conn.execute("SELECT * FROM snippets WHERE name=?", (db_name,)).fetchone()
        code = row['code']
        params = json.loads(row['params'] or '[]')
        param_names = [p['name'] for p in params]

        # Replace {{param}} placeholders with parameter names
        # Strip quotes around layer/purpose since they become variables
        proc_body = code
        for p in params:
            proc_body = proc_body.replace("{{" + p['name'] + "}}", p['name'])
        # Fix: "layer" "purpose" should be layer purpose (variables)
        proc_body = proc_body.replace('"layer" "purpose"', 'layer purpose')

        # Build procedure signature
        if param_names:
            sig = f"procedure({proc_name}({', '.join(param_names)}))"
        else:
            sig = f"procedure({proc_name}())"

        lines.append(f"; {db_name} → {proc_name}()")
        lines.append(sig)
        lines.append(f"  ; {row['description']}")
        lines.append(f"  {proc_body}")
        lines.append(f"endprocedure")
        lines.append("")
    return "\n".join(lines)


def main():
    conn = sqlite3.connect(str(DB))
    conn.row_factory = sqlite3.Row

    promoted = []
    for db_name, proc_name in PROMOTE.items():
        row = conn.execute("SELECT * FROM snippets WHERE name=?", (db_name,)).fetchone()
        if row:
            # Mark as promoted
            conn.execute("UPDATE snippets SET promoted_to=? WHERE name=?", (proc_name, db_name))
            promoted.append((db_name, proc_name))
            print(f"  {db_name:20s} → {proc_name}")
        else:
            print(f"  SKIP {db_name}: not found in DB")

    conn.commit()

    # Generate .il file
    il_content = generate_il(promoted, conn)
    IL_OUT.write_text(il_content, encoding="utf-8")
    print(f"\nGenerated: {IL_OUT} ({IL_OUT.stat().st_size} bytes)")
    conn.close()


if __name__ == "__main__":
    main()
