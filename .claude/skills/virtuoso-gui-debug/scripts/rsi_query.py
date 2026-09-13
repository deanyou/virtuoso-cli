#!/usr/bin/env python3
"""
RSI Query CLI — search Virtuoso SKILL functions from the knowledge base.

Usage:
  python3 rsi_query.py "draw rectangle"        # semantic search
  python3 rsi_query.py --function dbCreateRect  # exact function lookup
  python3 rsi_query.py --category Custom_Layout   # list by category
  python3 rsi_query.py --prefix dbCreate         # prefix search
  python3 rsi_query.py --version IC251 "create path"
"""
import argparse
import sqlite3
import sys
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"

def get_conn():
    conn = sqlite3.connect(str(DB))
    conn.row_factory = sqlite3.Row
    return conn

def search(conn, query, version="IC251", limit=10):
    """Semantic search via FTS5."""
    # Use OR between terms for better recall
    terms = query.strip().split()
    fts_query = " OR ".join(terms)
    try:
        rows = conn.execute("""
            SELECT name, syntax, description, category, version
            FROM fnd_functions_fts
            WHERE fnd_functions_fts MATCH ? AND version = ?
            LIMIT ?
        """, (fts_query, version, limit)).fetchall()
    except Exception:
        # Fallback: LIKE search
        like = f"%{query}%"
        rows = conn.execute("""
            SELECT name, syntax, description, category, version
            FROM fnd_functions
            WHERE version = ? AND (name LIKE ? OR description LIKE ?)
            LIMIT ?
        """, (version, like, like, limit)).fetchall()
    return [dict(r) for r in rows]

def get_function(conn, name, version="IC251"):
    """Exact function lookup."""
    row = conn.execute("""
        SELECT name, syntax, description, category, version
        FROM fnd_functions WHERE name = ? AND version = ?
    """, (name, version)).fetchone()
    return dict(row) if row else None

def list_category(conn, category, version="IC251", limit=20):
    """List functions by category."""
    rows = conn.execute("""
        SELECT name, syntax, description
        FROM fnd_functions WHERE category = ? AND version = ?
        ORDER BY name LIMIT ?
    """, (category, version, limit)).fetchall()
    return [dict(r) for r in rows]

def prefix_search(conn, prefix, version="IC251", limit=20):
    """Search by function name prefix."""
    rows = conn.execute("""
        SELECT name, syntax, description
        FROM fnd_functions WHERE name LIKE ? AND version = ?
        ORDER BY name LIMIT ?
    """, (prefix + "%", version, limit)).fetchall()
    return [dict(r) for r in rows]

def check_errors(conn, name):
    """Check if this function has known failures."""
    rows = conn.execute("""
        SELECT error_type, error_message, created_at
        FROM error_history WHERE function_name = ? AND fixed = 0
        ORDER BY created_at DESC LIMIT 5
    """, (name,)).fetchall()
    return [dict(r) for r in rows]

def main():
    parser = argparse.ArgumentParser(description="RSI: Virtuoso SKILL function knowledge base")
    parser.add_argument("query", nargs="?", help="Search query (natural language)")
    parser.add_argument("--function", "-f", help="Exact function name lookup")
    parser.add_argument("--category", "-c", help="List functions by category")
    parser.add_argument("--prefix", "-p", help="Function name prefix search")
    parser.add_argument("--version", "-v", default="IC251", help="Virtuoso version (default: IC251)")
    parser.add_argument("--limit", "-n", type=int, default=10, help="Max results")
    args = parser.parse_args()

    conn = get_conn()

    if args.function:
        # Exact lookup + error history
        result = get_function(conn, args.function, args.version)
        if result:
            print(f"=== {result['name']} ({result['version']}) ===")
            print(f"  Category: {result['category']}")
            print(f"  Syntax:   {result['syntax']}")
            print(f"  Desc:     {result['description']}")
            errors = check_errors(conn, args.function)
            if errors:
                print(f"\n  ⚠ Known errors ({len(errors)}):")
                for e in errors:
                    print(f"    [{e['error_type']}] {e['error_message'][:100]}")
        else:
            print(f"Function '{args.function}' not found in {args.version}")
            # Try other versions
            others = conn.execute("""
                SELECT DISTINCT version FROM fnd_functions WHERE name=?
            """, (args.function,)).fetchall()
            if others:
                print(f"  Found in: {', '.join(r['version'] for r in others)}")

    elif args.category:
        results = list_category(conn, args.category, args.version, args.limit)
        print(f"=== {args.category} ({args.version}, {len(results)} results) ===")
        for r in results:
            print(f"  {r['name']:35s} {r['description'][:60]}")

    elif args.prefix:
        results = prefix_search(conn, args.prefix, args.version, args.limit)
        print(f"=== Prefix '{args.prefix}' ({args.version}, {len(results)} results) ===")
        for r in results:
            print(f"  {r['name']:35s} {r['description'][:60]}")

    elif args.query:
        results = search(conn, args.query, args.version, args.limit)
        print(f"=== Search: '{args.query}' ({args.version}, {len(results)} results) ===")
        for r in results:
            print(f"\n  {r['name']}")
            print(f"    Syntax: {r['syntax'][:100]}")
            print(f"    Desc:   {r['description'][:80]}")

    else:
        # Show stats
        total = conn.execute("SELECT COUNT(*) FROM fnd_functions WHERE version=?", (args.version,)).fetchone()[0]
        print(f"RSI Knowledge Base — {args.version}: {total:,} functions")
        print(f"\nUsage:")
        print(f"  {sys.argv[0]} 'draw rectangle'     # semantic search")
        print(f"  {sys.argv[0]} -f dbCreateRect     # exact lookup")
        print(f"  {sys.argv[0]} -c Custom_Layout    # list category")
        print(f"  {sys.argv[0]} -p dbCreate         # prefix search")

    conn.close()

if __name__ == "__main__":
    main()
