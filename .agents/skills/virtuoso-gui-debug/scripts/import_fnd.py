#!/usr/bin/env python3
"""
Parse .fnd files and import functions into RSI SQLite database.
Supports IC231 and IC618 versions.
"""
import re
import os
import sqlite3
from pathlib import Path

API_ROOT = Path(r"E:\git\skill\docs\virtuoso_api")
DB_PATH = Path(r"E:\git\virtuoso-cli\.claude\skills\virtuoso-gui-debug\data\skill_db.sqlite3")

def parse_fnd_file(filepath, version, category):
    """Parse a .fnd file and return list of function dicts."""
    with open(filepath, encoding='utf-8', errors='replace') as f:
        text = f.read()

    functions = []
    # Pattern: ("funcName"  "syntax"  "description")
    # Entries can span multiple lines
    pattern = re.compile(
        r'\("([^"]+)"\s*\n\s*"((?:[^"\\]|\\.)*)"\s*\n\s*"((?:[^"\\]|\\.)*)"\)',
        re.MULTILINE
    )

    for m in pattern.finditer(text):
        name = m.group(1)
        syntax = m.group(2).replace('\n', ' ').strip()
        desc = m.group(3).replace('\n', ' ').strip()

        # Extract args count from syntax
        args_match = re.search(r'\(([^)]*)\)', syntax)
        args_str = args_match.group(1).strip() if args_match else ""
        args_count = len([a for a in args_str.split(',') if a.strip() and not a.strip().startswith('[')])

        functions.append({
            'name': name,
            'syntax': syntax,
            'description': desc,
            'category': category,
            'version': version,
            'args_count': args_count,
        })

    return functions

def import_version(version_dir, version_tag):
    """Import all .fnd files from a version directory."""
    all_funcs = []

    for subdir in sorted(version_dir.iterdir()):
        if not subdir.is_dir():
            continue
        category = subdir.name

        for fnd_file in subdir.glob("*.fnd"):
            funcs = parse_fnd_file(fnd_file, version_tag, category)
            all_funcs.extend(funcs)
            print(f"  {version_tag}/{category}/{fnd_file.name}: {len(funcs)} functions")

    return all_funcs

def main():
    conn = sqlite3.connect(str(DB_PATH))
    conn.row_factory = sqlite3.Row

    # Create table if not exists
    conn.execute("""CREATE TABLE IF NOT EXISTS fnd_functions (
        id INTEGER PRIMARY KEY,
        name TEXT,
        syntax TEXT,
        description TEXT,
        category TEXT,
        version TEXT,
        args_count INTEGER,
        UNIQUE(name, version)
    )""")

    # Import IC231
    print("=== Parsing IC231 ===")
    ic231_dir = API_ROOT / "ic231_skill_api"
    ic231_funcs = import_version(ic231_dir, "IC231")
    print(f"Total IC231: {len(ic231_funcs)} functions")

    # Import IC618
    print("\n=== Parsing IC618 ===")
    ic618_dir = API_ROOT / "ic618_skill_api"
    ic618_funcs = import_version(ic618_dir, "IC618")
    print(f"Total IC618: {len(ic618_funcs)} functions")

    # Insert into DB
    for func in ic231_funcs + ic618_funcs:
        conn.execute("""INSERT OR REPLACE INTO fnd_functions
            (name, syntax, description, category, version, args_count)
            VALUES (?, ?, ?, ?, ?, ?)""",
            (func['name'], func['syntax'], func['description'],
             func['category'], func['version'], func['args_count']))

    conn.commit()

    # Stats
    print(f"\n=== Database Stats ===")
    for row in conn.execute("SELECT version, COUNT(*) as cnt FROM fnd_functions GROUP BY version"):
        print(f"  {row['version']}: {row['cnt']} functions")

    # Cross-version comparison
    print(f"\n=== Cross-version Functions ===")
    for row in conn.execute("""
        SELECT a.name, a.version as v1, b.version as v2
        FROM fnd_functions a
        JOIN fnd_functions b ON a.name = b.name AND a.version < b.version
        LIMIT 20
    """):
        print(f"  {row['name']}: {row['v1']} + {row['v2']}")

    conn.close()
    print("\nDone!")

if __name__ == "__main__":
    main()
