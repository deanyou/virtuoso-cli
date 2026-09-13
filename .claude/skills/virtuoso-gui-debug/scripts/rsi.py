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

# ── Snippet handlers ─────────────────────────────────────────────────────────

def _handle_snippet(c, argv):
    if not argv:
        print("Usage: rsi snippet list|info NAME|run NAME [--k=v ...]|save NAME DESC CODE")
        return

    cmd = argv[0]

    if cmd == "list":
        rows = c.execute("SELECT name, category, description, success_count FROM snippets ORDER BY category, name").fetchall()
        print(f"=== Snippets ({len(rows)}) ===\n")
        cur_cat = None
        for r in rows:
            if r[1] != cur_cat:
                print(f"[{r[1]}]")
                cur_cat = r[1]
            print(f"  {r[0]:20s} {r[2]}  ({r[3]}x)")
        return

    if cmd == "info" and len(argv) > 1:
        row = c.execute("SELECT * FROM snippets WHERE name=?", (argv[1],)).fetchone()
        if not row:
            print(f"Snippet not found: {argv[1]}")
            return
        d = dict(row)
        print(f"=== {d['name']} ===")
        print(f"  Category: {d['category']}")
        print(f"  Desc:     {d['description']}")
        print(f"  Used:     {d['success_count']}x")
        params = json.loads(d['params'] or '[]')
        if params:
            print(f"\n  Parameters:")
            for p in params:
                print(f"    {p['name']} (default: {p.get('default', '?')})")
        print(f"\n  Code:")
        print(f"    {d['code']}")
        return

    if cmd == "run" and len(argv) > 1:
        name = argv[1]
        # Parse --key=value args
        overrides = {}
        for a in argv[2:]:
            if a.startswith("--"):
                k, _, v = a[2:].partition("=")
                overrides[k] = v
        row = c.execute("SELECT * FROM snippets WHERE name=?", (name,)).fetchone()
        if not row:
            print(f"Snippet not found: {name}")
            return
        d = dict(row)
        code = d['code']
        params = json.loads(d['params'] or '[]')
        # Fill defaults then overrides
        fill = {}
        for p in params:
            fill[p['name']] = overrides.get(p['name'], p.get('default', ''))
        for k, v in fill.items():
            code = code.replace("{{" + k + "}}", v)
        print(f"SKILL: {code}")
        # Increment usage
        c.execute("UPDATE snippets SET success_count=success_count+1, last_used=? WHERE name=?",
                  (datetime.now().isoformat(), name))
        c.commit()
        return

    if cmd == "save" and len(argv) >= 4:
        name, desc, code = argv[1], argv[2], argv[3]
        # Extract param names from {{param}} placeholders
        import re
        found = re.findall(r'\{\{(\w+)\}\}', code)
        params = json.dumps([{"name": p, "default": ""} for p in found])
        c.execute("""INSERT OR REPLACE INTO snippets (name, description, category, code, params, version, success_count, last_used)
                      VALUES (?, ?, 'custom', ?, ?, 'IC251', 0, ?)""",
                  (name, desc, code, params, datetime.now().isoformat()))
        c.commit()
        print(f"Saved snippet: {name} (params: {', '.join(found) or 'none'})")
        return

    print("Usage: rsi snippet list|info NAME|run NAME [--k=v ...]|save NAME DESC CODE")


# ── CLI ───────────────────────────────────────────────────────────────────────

def main():
    import sys
    args = sys.argv[1:]

    # Snippet subcommand: rsi snippet ...
    if args and args[0] == "snippet":
        c = conn()
        _handle_snippet(c, args[1:])
        return

    ap = argparse.ArgumentParser(prog="rsi", description="Virtuoso SKILL RSI")
    ap.add_argument("query", nargs="?", help="Natural language search")
    ap.add_argument("-f", "--function", help="Exact function lookup")
    ap.add_argument("-v", "--version", default="IC251")
    ap.add_argument("-n", "--limit", type=int, default=8)
    ap.add_argument("--record", help="Record: func --success or func --error 'msg'")
    ap.add_argument("--success", action="store_true")
    ap.add_argument("--error", help="Error message to record")
    ap.add_argument("--status", action="store_true")
    parsed = ap.parse_args(args)

    c = conn()

    if parsed.status:
        for t in ['fnd_functions', 'functions', 'error_history', 'param_examples', 'rsi_progress', 'snippets']:
            try:
                n = c.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
                print(f"  {t}: {n}")
            except: pass
        return

    if parsed.record:
        func = parsed.record
        if parsed.error:
            record_failure(c, func, "runtime", parsed.error)
            print(f"Recorded error for {func}")
        else:
            print(f"Recorded success for {func}")
        return

    if parsed.function:
        r = lookup(c, parsed.function, parsed.version)
        if r:
            print(f"=== {r['name']} ({parsed.version}) ===")
            print(f"  Syntax: {r['syntax']}")
            print(f"  Desc:   {r['description']}")
            if r.get('params'):
                print(f"\n  Known params:")
                for p in r['params']:
                    print(f"    {p['param_name']}: {p['example_value']} ({p['success_count']}x)")
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
            print(f"Not found: {parsed.function}")
        return

    if parsed.query:
        results = search(c, parsed.query, parsed.version, parsed.limit)
        print(f"=== Search: '{parsed.query}' ({parsed.version}) ===\n")
        for r in results:
            print(f"  {r['name']}")
            print(f"    {r['description'][:70]}")
            print()
        return

    ap.print_help()

if __name__ == "__main__":
    main()
