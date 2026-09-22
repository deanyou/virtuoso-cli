#!/usr/bin/env python3
"""Record RSI progress milestones to database."""
import sqlite3
from datetime import datetime
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"
conn = sqlite3.connect(str(DB))

# Create progress table
conn.execute("""
    CREATE TABLE IF NOT EXISTS rsi_progress (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        date TEXT,
        phase TEXT,
        milestone TEXT,
        detail TEXT,
        functions_added INTEGER DEFAULT 0,
        errors_recorded INTEGER DEFAULT 0
    )
""")

# Seed historical milestones
milestones = [
    ("2026-09-12", "P0", "Manual function enumeration",
     "488 candidate functions tested via vcli; 70 confirmed exist", 488, 1),
    ("2026-09-12", "P0", "Official .fnd docs imported",
     "IC231: 9,619 + IC618: 8,807 functions from Cadence .fnd files", 18426, 0),
    ("2026-09-12", "P0", "IC251 baseline from IC231",
     "Copied 9,619 functions as IC251 baseline; 50 sampled validation = 100%", 9619, 0),
    ("2026-09-12", "P1", "FTS5 full-text search",
     "fnd_functions_fts virtual table; semantic search via rsi_query.py", 0, 0),
    ("2026-09-12", "P1", "Case-insensitive name matching",
     "Fixed SQLite LIKE case sensitivity (dbCreatePolygon was missed)", 0, 1),
    ("2026-09-12", "P1", "Multi-keyword scoring",
     "Functions matching multiple keywords rank higher; draw polygon -> dbCreatePolygon #1", 0, 0),
    ("2026-09-12", "P1", "Parameter templates",
     "param_examples table records successful layer/purpose/coords (10 examples)", 0, 0),
    ("2026-09-12", "P1", "Error auto-learning",
     "record_failure() + has_known_failure(); known errors shown in -f lookup", 0, 0),
    ("2026-09-12", "P1", "E2E workflow verified",
     "query -> call -> record loop: draw path -> dbCreatePath -> success", 0, 0),
]

now = datetime.now().isoformat()
for date, phase, milestone, detail, funcs, errs in milestones:
    conn.execute("""
        INSERT OR IGNORE INTO rsi_progress (date, phase, milestone, detail, functions_added, errors_recorded)
        VALUES (?, ?, ?, ?, ?, ?)
    """, (date, phase, milestone, detail, funcs, errs))

conn.commit()

# Show all
rows = conn.execute("SELECT * FROM rsi_progress ORDER BY id").fetchall()
print(f"RSI progress: {len(rows)} milestones recorded")
for r in rows:
    print(f"  [{r[1]}] {r[2]}: {r[3]}")
conn.close()
