#!/usr/bin/env python3
"""Initialize snippets table and seed with verified snippets."""
import sqlite3, json
from datetime import datetime
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"
conn = sqlite3.connect(str(DB))

conn.execute("""
    CREATE TABLE IF NOT EXISTS snippets (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT UNIQUE NOT NULL,
        description TEXT,
        category TEXT,
        code TEXT NOT NULL,
        params TEXT,
        version TEXT DEFAULT 'IC251',
        success_count INTEGER DEFAULT 0,
        last_used TEXT
    )
""")

# Verified snippets from real testing
snippets = [
    {
        "name": "draw_rect",
        "description": "Draw a rectangle on current cellview",
        "category": "layout",
        "code": 'let(((cv geGetEditCellView())) dbCreateRect(cv list("{{layer}}" "{{purpose}}") list({{x1}}:{{y1}} {{x2}}:{{y2}})))',
        "params": json.dumps([
            {"name": "layer", "default": "M1"},
            {"name": "purpose", "default": "drawing"},
            {"name": "x1", "default": "0"},
            {"name": "y1", "default": "0"},
            {"name": "x2", "default": "20"},
            {"name": "y2", "default": "20"},
        ]),
    },
    {
        "name": "draw_polygon",
        "description": "Draw a polygon from point list",
        "category": "layout",
        "code": 'let(((cv geGetEditCellView())) dbCreatePolygon(cv list("{{layer}}" "{{purpose}}") list({{points}})))',
        "params": json.dumps([
            {"name": "layer", "default": "M1"},
            {"name": "purpose", "default": "drawing"},
            {"name": "points", "default": "0:60 10:50 20:60 10:70"},
        ]),
    },
    {
        "name": "draw_path",
        "description": "Draw a path with given width",
        "category": "layout",
        "code": 'let(((cv geGetEditCellView())) dbCreatePath(cv list("{{layer}}" "{{purpose}}") list({{points}}) {{width}}))',
        "params": json.dumps([
            {"name": "layer", "default": "M1"},
            {"name": "purpose", "default": "drawing"},
            {"name": "points", "default": "60:0 80:20 100:0"},
            {"name": "width", "default": "2.0"},
        ]),
    },
    {
        "name": "save_cv",
        "description": "Save current cellview",
        "category": "utility",
        "code": 'let(((cv geGetEditCellView())) dbSave(cv))',
        "params": json.dumps([]),
    },
    {
        "name": "zoom_fit",
        "description": "Zoom to fit in current window",
        "category": "navigation",
        "code": 'hiZoomToFit()',
        "params": json.dumps([]),
    },
    {
        "name": "select_all",
        "description": "Select all objects in current window",
        "category": "layout",
        "code": 'geSelectAll()',
        "params": json.dumps([]),
    },
    {
        "name": "delete_selected",
        "description": "Delete currently selected objects",
        "category": "layout",
        "code": 'let(((sel geGetSelSet())) foreach(s sel dbDeleteObject(s)))',
        "params": json.dumps([]),
    },
    {
        "name": "list_shapes",
        "description": "List all shapes in current cellview",
        "category": "query",
        "code": 'geGetEditCellView()->shapes~>mapcar(lambda((s) list(s~>objType s~>layer s~>purpose)))',
        "params": json.dumps([]),
    },
]

now = datetime.now().isoformat()
for s in snippets:
    conn.execute("""
        INSERT OR REPLACE INTO snippets (name, description, category, code, params, version, success_count, last_used)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
    """, (s["name"], s["description"], s["category"], s["code"], s["params"],
          "IC251", 1, now))

conn.commit()

# Show
rows = conn.execute("SELECT name, category, description FROM snippets ORDER BY category, name").fetchall()
print(f"Seeded {len(rows)} snippets:")
for r in rows:
    print(f"  [{r[1]}] {r[0]} — {r[2]}")
conn.close()
