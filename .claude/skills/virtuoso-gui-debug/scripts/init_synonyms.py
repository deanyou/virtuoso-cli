#!/usr/bin/env python3
"""Initialize synonyms table with SKILL abbreviation mappings."""
import sqlite3
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"

# Canonical -> variants (abbreviations, common misspellings)
SYNONYMS = {
    "rect": ["rectangle", "rectangular", "rct"],
    "polygon": ["poly", "gon"],
    "instance": ["inst", "cell_instance", "comp"],
    "path": ["wire", "route", "net"],
    "line": ["ln", "segment", "seg"],
    "layer": ["lyr", "level"],
    "purpose": ["purp", "purpose_layer"],
    "window": ["win", "view"],
    "zoom": ["fit", "view", "scale"],
    "save": ["write", "store", "commit"],
    "delete": ["del", "remove", "rm", "kill"],
    "copy": ["dup", "duplicate", "clone"],
    "move": ["translate", "shift", "position"],
    "select": ["sel", "pick", "choose"],
    "cellview": ["cv", "cell_view", "view"],
    "export": ["save_image", "screenshot", "dump"],
    "query": ["list", "get", "show", "find"],
    "create": ["new", "add", "make", "draw"],
    "draw": ["create", "make", "add", "sketch"],
    "shape": ["figure", "object", "fig"],
}

def main():
    conn = sqlite3.connect(str(DB))
    conn.execute("""CREATE TABLE IF NOT EXISTS synonyms (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        canonical TEXT NOT NULL,
        variant TEXT NOT NULL,
        UNIQUE(canonical, variant))""")
    count = 0
    for canon, variants in SYNONYMS.items():
        for v in variants:
            conn.execute("INSERT OR IGNORE INTO synonyms (canonical, variant) VALUES (?, ?)", (canon, v))
            count += 1
    conn.commit()
    total = conn.execute("SELECT COUNT(*) FROM synonyms").fetchone()[0]
    print(f"Synonyms: {count} inserted, {total} total")
    conn.close()

if __name__ == "__main__":
    main()
