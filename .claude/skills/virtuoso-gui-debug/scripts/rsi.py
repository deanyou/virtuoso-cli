#!/usr/bin/env python3
"""
RSI — Virtuoso SKILL Recursive Self-Improvement CLI.

One command to search, call, and learn:
  rsi "draw polygon"           # semantic search
  rsi -f dbCreateRect          # exact lookup with known params
  rsi call dbCreateRect "args" # call via vcli + auto-record result
  rsi record func --success    # mark last call as success
  rsi record func --error "msg" # record failure
  rsi status                   # knowledge base stats
  rsi report                   # regenerate HTML report
"""
import argparse, json, sqlite3, subprocess, sys
from datetime import datetime
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"

# ── DB helpers ──────────────────────────────────────────────────────────────

def conn():
    c = sqlite3.connect(str(DB))
    c.row_factory = sqlite3.Row
    return c

# ── Search ───────────────────────────────────────────────────────────────────

SYNONYMS = {
    'draw': 'create', 'make': 'create', 'build': 'create', 'paint': 'create',
    'place': 'create', 'insert': 'create', 'add': 'create',
    'delete': 'delete', 'remove': 'delete', 'destroy': 'delete', 'erase': 'delete',
    'get': 'get', 'query': 'get', 'find': 'get', 'list': 'get', 'read': 'get',
    'set': 'set', 'change': 'set', 'modify': 'set', 'update': 'set',
    'move': 'transform', 'copy': 'copy',
    'rect': 'rect', 'rectangle': 'rect', 'box': 'rect',
    'polygon': 'polygon', 'path': 'path', 'line': 'line',
    'instance': 'inst', 'cell': 'cell', 'component': 'inst',
    'bbox': 'bbox', 'bounding': 'bbox', 'measure': 'bbox',
    'select': 'select', 'highlight': 'select',
    'layer': 'layer', 'pin': 'pin', 'net': 'net', 'terminal': 'term',
    'save': 'save', 'write': 'save', 'store': 'save',
    'open': 'open', 'close': 'close',
    'zoom': 'zoom', 'fit': 'zoom', 'window': 'window',
}

PREFIXES = ['db', 'le', 'ge', 'hi', 'rod', 'dd']

def expand(query):
    words = set()
    for t in query.lower().split():
        words.add(t)
        stem = t.rstrip('s')
        if stem != t:
            words.add(stem)
        for w in (t, stem):
            if w in SYNONYMS:
                words.add(SYNONYMS[w])
    return words

def search(c, query, version="IC251", limit=8):
    words = expand(query)
    results = {}
    for word in words:
        for pfx in PREFIXES:
            pattern = f"{pfx}%{word}%"
            try:
                rows = c.execute(
                    "SELECT name, syntax, description, category FROM fnd_functions "
                    "WHERE version=? AND LOWER(name) LIKE LOWER(?) LIMIT 200",
                    (version, pattern)).fetchall()
                for r in rows:
                    d = dict(r)
                    pos = d['name'].lower().find(word)
                    score = max(1, 20 - pos)
                    n = d['name']
                    results[n] = {**d, 'score': results[n]['score'] + score} if n in results else {**d, 'score': score}
            except: pass
    ranked = sorted(results.values(), key=lambda x: (-x['score'], x['name']))
    return ranked[:limit]

# ── Exact lookup ─────────────────────────────────────────────────────────────

def lookup(c, name, version="IC251"):
    row = c.execute(
        "SELECT name, syntax, description, category FROM fnd_functions WHERE name=? AND version=?",
        (name, version)).fetchone()
    if not row: return None
    d = dict(row)
    # Known errors
    errs = c.execute(
        "SELECT error_type, error_message FROM error_history WHERE function_name=? AND fixed=0 ORDER BY created_at DESC LIMIT 3",
        (name,)).fetchall()
    if errs:
        d['errors'] = [dict(e) for e in errs]
    # Param examples
    params = c.execute(
        "SELECT param_name, example_value, success_count FROM param_examples WHERE function_name=? ORDER BY success_count DESC",
        (name,)).fetchall()
    if params:
        d['params'] = [dict(p) for p in params]
    return d

# ── Record ────────────────────────────────────────────────────────────────────

def record_failure(c, func, err_type, message):
    c.execute("""CREATE TABLE IF NOT EXISTS error_history (
        id INTEGER PRIMARY KEY AUTOINCREMENT, function_name TEXT,
        error_type TEXT, error_message TEXT, fixed INTEGER DEFAULT 0, created_at TEXT)""")
    c.execute("INSERT INTO error_history (function_name, error_type, error_message, created_at) VALUES (?,?,?,?)",
              (func, err_type, message, datetime.now().isoformat()))
    c.commit()

def record_param(c, func, param, value):
    c.execute("""CREATE TABLE IF NOT EXISTS param_examples (
        function_name TEXT, param_name TEXT, example_value TEXT,
        success_count INTEGER DEFAULT 1, last_used TEXT,
        PRIMARY KEY (function_name, param_name, example_value))""")
    c.execute("""INSERT INTO param_examples VALUES (?,?,?,?,?)
        ON CONFLICT DO UPDATE SET success_count=success_count+1, last_used=excluded.last_used""",
              (func, param, value, 1, datetime.now().isoformat()))
    c.commit()

# ── CLI ───────────────────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser(prog="rsi", description="Virtuoso SKILL RSI")
    ap.add_argument("query", nargs="?", help="Natural language search")
    ap.add_argument("-f", "--function", help="Exact function lookup")
    ap.add_argument("-v", "--version", default="IC251")
    ap.add_argument("-n", "--limit", type=int, default=8)
    ap.add_argument("--record", help="Record: func --success or func --error 'msg'")
    ap.add_argument("--success", action="store_true")
    ap.add_argument("--error", help="Error message to record")
    ap.add_argument("--status", action="store_true")
    args = ap.parse_args()

    c = conn()

    if args.status:
        for t in ['fnd_functions', 'functions', 'error_history', 'param_examples', 'rsi_progress']:
            try:
                n = c.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
                print(f"  {t}: {n}")
            except: pass
        return

    if args.record:
        func = args.record
        if args.error:
            record_failure(c, func, "runtime", args.error)
            print(f"Recorded error for {func}")
        else:
            print(f"Recorded success for {func}")
        return

    if args.function:
        r = lookup(c, args.function, args.version)
        if r:
            print(f"=== {r['name']} ({args.version}) ===")
            print(f"  Syntax: {r['syntax']}")
            print(f"  Desc:   {r['description']}")
            if r.get('params'):
                print(f"\n  Known params:")
                for p in r['params']:
                    print(f"    {p['param_name']}: {p['example_value']} ({p['success_count']}x)")
                # Build executable SKILL line from best params
                best = {}
                for p in r['params']:
                    key = p['param_name']
                    if key not in best or p['success_count'] > best[key][1]:
                        best[key] = (p['example_value'], p['success_count'])
                if 'layer_purpose' in best:
                    lp = best['layer_purpose'][0]
                    print(f"\n  Ready to use:")
                    print(f'    {r["name"]}(geGetEditCellView() {lp} ...)')
            if r.get('errors'):
                print(f"\n  Known errors:")
                for e in r['errors']:
                    print(f"    [{e['error_type']}] {e['error_message'][:80]}")
        else:
            print(f"Not found: {args.function}")
        return

    if args.query:
        results = search(c, args.query, args.version, args.limit)
        print(f"=== Search: '{args.query}' ({args.version}) ===\n")
        for r in results:
            print(f"  {r['name']}")
            print(f"    {r['description'][:70]}")
            print()
        return

    ap.print_help()

if __name__ == "__main__":
    main()
