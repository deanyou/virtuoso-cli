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

DB_PATH = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"


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
    # Ensure error_history exists (may have been created earlier)
    conn.execute("""
        CREATE TABLE IF NOT EXISTS error_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            function_name TEXT,
            error_type TEXT,
            error_message TEXT,
            fixed BOOLEAN DEFAULT 0,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
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


def record_failure(conn, function_name, error_type, error_message):
    """Record a function failure for RSI learning. Never repeat the same mistake."""
    conn.execute("""
        INSERT INTO error_history (function_name, error_type, error_message, fixed, created_at)
        VALUES (?, ?, ?, 0, ?)
    """, (function_name, error_type, error_message[:500], datetime.now().isoformat()))
    conn.commit()


def has_known_failure(conn, function_name):
    """Check if a function has unresolved known failures. Returns dict or None."""
    row = conn.execute("""
        SELECT error_type, error_message, created_at
        FROM error_history WHERE function_name = ? AND fixed = 0
        ORDER BY created_at DESC LIMIT 1
    """, (function_name,)).fetchone()
    return dict(row) if row else None


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


def record_param_example(conn, function_name, param_name, example_value):
    """Record a successful parameter value for a function."""
    conn.execute("""
        CREATE TABLE IF NOT EXISTS param_examples (
            function_name TEXT,
            param_name TEXT,
            example_value TEXT,
            success_count INTEGER DEFAULT 1,
            last_used TEXT,
            PRIMARY KEY (function_name, param_name, example_value)
        )
    """)
    conn.execute("""
        INSERT INTO param_examples (function_name, param_name, example_value, success_count, last_used)
        VALUES (?, ?, ?, 1, ?)
        ON CONFLICT(function_name, param_name, example_value) DO UPDATE SET
            success_count = success_count + 1,
            last_used = excluded.last_used
    """, (function_name, param_name, example_value, datetime.now().isoformat()))
    conn.commit()


def get_param_examples(conn, function_name):
    """Get known successful parameter examples for a function."""
    conn.execute("""
        CREATE TABLE IF NOT EXISTS param_examples (
            function_name TEXT,
            param_name TEXT,
            example_value TEXT,
            success_count INTEGER DEFAULT 1,
            last_used TEXT,
            PRIMARY KEY (function_name, param_name, example_value)
        )
    """)
    rows = conn.execute("""
        SELECT param_name, example_value, success_count
        FROM param_examples WHERE function_name = ?
        ORDER BY success_count DESC
    """, (function_name,)).fetchall()
    return [dict(r) for r in rows]


def stats(conn):
    """Database statistics."""
    total = conn.execute("SELECT COUNT(*) FROM functions").fetchone()[0]
    existing = conn.execute("SELECT COUNT(*) FROM functions WHERE func_exists=1").fetchone()[0]
    layers_count = conn.execute("SELECT COUNT(*) FROM layers WHERE drawable=1").fetchone()[0]
    props_count = conn.execute("SELECT COUNT(*) FROM shapes WHERE works=1").fetchone()[0]
    fnd_total = conn.execute("SELECT COUNT(*) FROM fnd_functions").fetchone()[0]
    errors_open = conn.execute("SELECT COUNT(*) FROM error_history WHERE fixed=0").fetchone()[0]
    return {
        "total_functions_tested": total,
        "functions_exist": existing,
        "drawable_layers": layers_count,
        "verified_properties": props_count,
        "fnd_functions": fnd_total,
        "open_errors": errors_open,
        "db_path": str(DB_PATH),
    }


if __name__ == "__main__":
    conn = get_db()
    print(json.dumps(stats(conn), indent=2))
    print("\nKnown functions:")
    for f in get_known_functions(conn):
        print(f"  {f['name']}: {f.get('signature') or '(unknown)'}")
