#!/usr/bin/env python3
"""
SKILL Function Database — persistent knowledge store for Virtuoso SKILL API.

Stores discovered functions, their signatures, and test results in SQLite.
Enables recursive self-improvement: each session queries known functions
before experimenting, and records new findings for future use.
"""

import sqlite3
import json
import os
from datetime import datetime
from pathlib import Path

DB_PATH = Path(__file__).parent.parent / "skill_db.sqlite3"


def get_db():
    """Open database, creating schema if needed."""
    conn = sqlite3.connect(str(DB_PATH))
    conn.row_factory = sqlite3.Row
    conn.execute("""
        CREATE TABLE IF NOT EXISTS functions (
            name TEXT PRIMARY KEY,
            category TEXT,
            func_exists INTEGER DEFAULT 0,
            signature TEXT,
            description TEXT,
            last_tested TEXT,
            test_count INTEGER DEFAULT 0,
            notes TEXT
        )
    """)
    conn.execute("""
        CREATE TABLE IF NOT EXISTS layers (
            layer TEXT,
            purpose TEXT,
            drawable INTEGER DEFAULT 0,
            last_tested TEXT,
            PRIMARY KEY (layer, purpose)
        )
    """)
    conn.execute("""
        CREATE TABLE IF NOT EXISTS shapes (
            obj_type TEXT,
            property_name TEXT,
            works INTEGER DEFAULT 0,
            example_value TEXT,
            last_tested TEXT,
            PRIMARY KEY (obj_type, property_name)
        )
    """)
    conn.commit()
    return conn


def record_function(conn, name, exists, signature=None, description=None, notes=None):
    """Insert or update a function record."""
    category = name.split("_")[0] if "_" in name else name[:8]
    conn.execute("""
        INSERT INTO functions (name, category, func_exists, signature, description,
                               last_tested, test_count, notes)
        VALUES (?, ?, ?, ?, ?, ?, 1, ?)
        ON CONFLICT(name) DO UPDATE SET
            func_exists=excluded.func_exists,
            signature=COALESCE(excluded.signature, signature),
            description=COALESCE(excluded.description, description),
            last_tested=excluded.last_tested,
            test_count=test_count+1,
            notes=COALESCE(excluded.notes, notes)
    """, (name, category, int(exists), signature, description,
          datetime.now().isoformat(), notes))
    conn.commit()


def get_known_functions(conn, category=None):
    """Query known functions."""
    if category:
        rows = conn.execute(
            "SELECT * FROM functions WHERE category=? AND func_exists=1 ORDER BY name",
            (category,)).fetchall()
    else:
        rows = conn.execute(
            "SELECT * FROM functions WHERE func_exists=1 ORDER BY category, name").fetchall()
    return [dict(r) for r in rows]


def get_function(conn, name):
    """Get one function by name."""
    row = conn.execute("SELECT * FROM functions WHERE name=?", (name,)).fetchone()
    return dict(row) if row else None


def record_layer(conn, layer, purpose, drawable):
    """Record whether a layer/purpose combination is drawable."""
    conn.execute("""
        INSERT INTO layers (layer, purpose, drawable, last_tested)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(layer, purpose) DO UPDATE SET
            drawable=excluded.drawable, last_tested=excluded.last_tested
    """, (layer, purpose, int(drawable), datetime.now().isoformat()))
    conn.commit()


def record_shape_property(conn, obj_type, prop, works, example=None):
    """Record a shape property that works or doesn't."""
    conn.execute("""
        INSERT INTO shapes (obj_type, property_name, works, example_value, last_tested)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(obj_type, property_name) DO UPDATE SET
            works=excluded.works,
            example_value=COALESCE(excluded.example_value, example_value),
            last_tested=excluded.last_tested
    """, (obj_type, prop, int(works), example, datetime.now().isoformat()))
    conn.commit()


def stats(conn):
    """Database statistics."""
    total = conn.execute("SELECT COUNT(*) FROM functions").fetchone()[0]
    existing = conn.execute("SELECT COUNT(*) FROM functions WHERE func_exists=1").fetchone()[0]
    layers_count = conn.execute("SELECT COUNT(*) FROM layers WHERE drawable=1").fetchone()[0]
    props_count = conn.execute("SELECT COUNT(*) FROM shapes WHERE works=1").fetchone()[0]
    return {
        "total_functions_tested": total,
        "functions_exist": existing,
        "drawable_layers": layers_count,
        "verified_properties": props_count,
        "db_path": str(DB_PATH),
    }


if __name__ == "__main__":
    conn = get_db()
    print(json.dumps(stats(conn), indent=2))
    print("\nKnown functions:")
    for f in get_known_functions(conn):
        print(f"  {f['name']}: {f.get('signature') or '(unknown)'}")
