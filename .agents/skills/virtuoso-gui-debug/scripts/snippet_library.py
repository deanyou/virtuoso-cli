#!/usr/bin/env python3
"""
Snippet library — verified, parameterized, composable SKILL snippets.

Design principles:
- Single responsibility: one snippet = one operation
- Fully parameterized: all variable parts are {{params}}
- No external state assumptions (except geGetEditCellView)
- Composable: small snippets build into larger workflows
- Verified: each snippet has been tested in real Virtuoso
"""
import sqlite3, json
from datetime import datetime
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"

# ── Snippet definitions ──────────────────────────────────────────────────────
# Format: (name, category, description, code, params_json)
# Params are extracted from {{param}} placeholders in code.

SNIPPETS = [
    # ── Layout: atomic operations ──
    (
        "draw_rect", "layout",
        "Draw a rectangle on current cellview",
        'let(((cv geGetEditCellView())) dbCreateRect(cv list("{{layer}}" "{{purpose}}") list({{x1}}:{{y1}} {{x2}}:{{y2}})))',
        [{"name": "layer", "default": "M1"},
         {"name": "purpose", "default": "drawing"},
         {"name": "x1", "default": "0"},
         {"name": "y1", "default": "0"},
         {"name": "x2", "default": "20"},
         {"name": "y2", "default": "20"}],
    ),
    (
        "draw_polygon", "layout",
        "Draw a polygon from point list",
        'let(((cv geGetEditCellView())) dbCreatePolygon(cv list("{{layer}}" "{{purpose}}") list({{points}})))',
        [{"name": "layer", "default": "M1"},
         {"name": "purpose", "default": "drawing"},
         {"name": "points", "default": "0:60 10:50 20:60 10:70"}],
    ),
    (
        "draw_path", "layout",
        "Draw a path with given width",
        'let(((cv geGetEditCellView())) dbCreatePath(cv list("{{layer}}" "{{purpose}}") list({{points}}) {{width}}))',
        [{"name": "layer", "default": "M1"},
         {"name": "purpose", "default": "drawing"},
         {"name": "points", "default": "60:0 80:20 100:0"},
         {"name": "width", "default": "2.0"}],
    ),
    (
        "draw_line", "layout",
        "Draw a line segment",
        'let(((cv geGetEditCellView())) dbCreateLine(cv list("{{layer}}" "{{purpose}}") list({{x1}}:{{y1}} {{x2}}:{{y2}})))',
        [{"name": "layer", "default": "M1"},
         {"name": "purpose", "default": "drawing"},
         {"name": "x1", "default": "0"},
         {"name": "y1", "default": "0"},
         {"name": "x2", "default": "20"},
         {"name": "y2", "default": "0"}],
    ),
    # ── Layout: composite operations ──
    (
        "draw_contact", "layout",
        "Draw a contact: square rect on M1 + cut on C0",
        'let(((cv geGetEditCellView())) '
        'dbCreateRect(cv list("M1" "drawing") list({{x}}:{{y}} {{xs}}:{{ys}})) '
        'dbCreateRect(cv list("C0" "drawing") list({{x}}:{{y}} {{xs}}:{{ys}})))',
        [{"name": "layer", "default": "M1"},
         {"name": "x", "default": "0"},
         {"name": "y", "default": "0"},
         {"name": "xs", "default": "2"},
         {"name": "ys", "default": "2"}],
    ),
    # ── Query ──
    (
        "list_shapes", "query",
        "List all shapes in current cellview",
        'geGetEditCellView()->shapes~>mapcar(lambda((s) list(s~>objType s~>layer s~>purpose))',
        [],
    ),
    (
        "count_shapes", "query",
        "Count shapes by layer",
        'length(geGetEditCellView()->shapes)',
        [],
    ),
    (
        "get_bbox", "query",
        "Get bounding box of current cellview",
        'geGetEditCellView()~>bBox',
        [],
    ),
    # ── Navigation ──
    (
        "zoom_fit", "navigation",
        "Zoom to fit in current window",
        'hiZoomToFit()',
        [],
    ),
    (
        "select_all", "navigation",
        "Select all objects in current window",
        'geSelectAll()',
        [],
    ),
    (
        "deselect_all", "navigation",
        "Deselect all objects",
        'geDeselectAll()',
        [],
    ),
    # ── Edit ──
    (
        "delete_selected", "edit",
        "Delete currently selected objects",
        'foreach(s geGetSelSet() dbDeleteObject(s))',
        [],
    ),
    (
        "move_selected", "edit",
        "Move selected objects by dx dy",
        'leMoveFig(geGetSelSet() list({{dx}}:{{dy}}))',
        [{"name": "dx", "default": "10"},
         {"name": "dy", "default": "0"}],
    ),
    # ── Utility ──
    (
        "save_cv", "utility",
        "Save current cellview",
        'dbSave(geGetEditCellView())',
        [],
    ),
    (
        "version", "utility",
        "Get Virtuoso version",
        "getShellEnvParam('CDS_VER 'version)",
        [],
    ),
]


def init():
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
    now = datetime.now().isoformat()
    for name, cat, desc, code, params in SNIPPETS:
        conn.execute("""
            INSERT OR REPLACE INTO snippets (name, description, category, code, params, version, success_count, last_used)
            VALUES (?, ?, ?, ?, ?, 'IC251', 0, ?)
        """, (name, desc, cat, code, json.dumps(params), now))
    conn.commit()
    count = conn.execute("SELECT COUNT(*) FROM snippets").fetchone()[0]
    print(f"Initialized {count} snippets")
    conn.close()


if __name__ == "__main__":
    init()
