#!/usr/bin/env python3
"""Add FTS5 full-text search to RSI database for semantic function lookup."""
import sqlite3
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"
conn = sqlite3.connect(str(DB))
conn.row_factory = sqlite3.Row

# Check if FTS5 is available
try:
    conn.execute("CREATE VIRTUAL TABLE IF NOT EXISTS test_fts USING fts5(content)")
    conn.execute("DROP TABLE test_fts")
    print("FTS5 available ✅")
except Exception as e:
    print(f"FTS5 not available: {e}")
    exit(1)

# Create FTS5 virtual table for fnd_functions
print("\nCreating FTS5 index on fnd_functions...")
conn.execute("""
    CREATE VIRTUAL TABLE IF NOT EXISTS fnd_functions_fts USING fts5(
        name,
        description,
        category,
        version,
        syntax,
        content='fnd_functions',
        content_rowid='id'
    )
""")

# Populate FTS index from existing data
print("Populating FTS index...")
conn.execute("""
    INSERT INTO fnd_functions_fts(rowid, name, description, category, version, syntax)
    SELECT id, name, description, category, version, syntax FROM fnd_functions
""")

# Test search
print("\n=== Search Test: 'draw rectangle' ===")
rows = conn.execute("""
    SELECT name, syntax, description, version
    FROM fnd_functions_fts
    WHERE fnd_functions_fts MATCH 'draw rectangle'
    AND version = 'IC251'
    LIMIT 5
""").fetchall()
for r in rows:
    print(f"  {r['name']}: {r['description'][:80]}")

print("\n=== Search Test: 'create path' ===")
rows = conn.execute("""
    SELECT name, description
    FROM fnd_functions_fts
    WHERE fnd_functions_fts MATCH 'create path'
    AND version = 'IC251'
    LIMIT 5
""").fetchall()
for r in rows:
    print(f"  {r['name']}: {r['description'][:80]}")

print("\n=== Search Test: 'select object' ===")
rows = conn.execute("""
    SELECT name, description
    FROM fnd_functions_fts
    WHERE fnd_functions_fts MATCH 'select object'
    AND version = 'IC251'
    LIMIT 5
""").fetchall()
for r in rows:
    print(f"  {r['name']}: {r['description'][:80]}")

conn.commit()
conn.close()
print("\nFTS5 setup complete!")
