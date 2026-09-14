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

def expand(c, query):
    words = set()
    for t in query.lower().split():
        words.add(t)
        stem = t.rstrip('s')
        if stem != t:
            words.add(stem)
        # Hardcoded synonyms
        for w in (t, stem):
            if w in SYNONYMS:
                words.add(SYNONYMS[w])
        # DB synonyms (canonical -> variant)
        try:
            for w in (t, stem):
                rows = c.execute("SELECT canonical, variant FROM synonyms WHERE variant=? OR canonical=?", (w, w)).fetchall()
                for canon, variant in rows:
                    words.add(canon)
                    words.add(variant)
        except: pass
    return words

def search(c, query, version="IC251", limit=8):
    words = expand(c, query)
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
                    nl = d['name'].lower()
                    pos = nl.find(word)
                    score = max(1, 20 - pos)
                    # Exact match: name without prefix == word (e.g. dbSave == save)
                    bare = nl
                    for p in PREFIXES:
                        if bare.startswith(p):
                            bare = bare[len(p):]
                            break
                    if bare == word:
                        score += 100
                    # Shorter names preferred
                    score -= len(nl) * 0.1
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
        print("Commands: list | search KEYWORD | info NAME | run NAME | pipeline NAMES | save NAME DESC CODE")
        return

    cmd = argv[0]

    if cmd == "list":
        rows = c.execute("SELECT name, category, description, success_count, fail_count FROM snippets ORDER BY category, name").fetchall()
        print(f"=== Snippets ({len(rows)}) ===\n")
        cur_cat = None
        for r in rows:
            if r[1] != cur_cat:
                print(f"[{r[1]}]")
                cur_cat = r[1]
            total = r[3] + r[4]
            rate = f" {r[3]}/{total}" if total > 0 else ""
            print(f"  {r[0]:20s} {r[2]}{rate}")
        return

    if cmd == "search" and len(argv) > 1:
        # Search snippets by keyword
        q = " ".join(argv[1:]).lower()
        rows = c.execute("SELECT name, category, description, success_count, fail_count FROM snippets").fetchall()
        matches = []
        for r in rows:
            name, cat, desc = r[0].lower(), r[1].lower(), r[2].lower()
            score = 0
            for word in q.split():
                if word in name: score += 10
                if word in desc: score += 5
                if word in cat: score += 2
            if score > 0:
                matches.append((score, r))
        matches.sort(key=lambda x: -x[0])
        print(f"=== Snippet search: '{q}' ({len(matches)} found) ===\n")
        for score, r in matches:
            total = r[3] + r[4]
            rate = f" [used {r[3]}/{total}]" if total > 0 else ""
            print(f"  {r[0]:20s} {r[2]}{rate}")
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
        overrides = {}
        do_execute = False
        use_ssh = False
        session = None
        for a in argv[2:]:
            if a == "--execute":
                do_execute = True
            elif a == "--ssh":
                use_ssh = True
            elif a.startswith("--session="):
                session = a.split("=", 1)[1]
            elif a.startswith("--"):
                k, _, v = a[2:].partition("=")
                overrides[k] = v
        row = c.execute("SELECT * FROM snippets WHERE name=?", (name,)).fetchone()
        if not row:
            print(f"Snippet not found: {name}")
            return
        d = dict(row)
        code = d['code']
        params = json.loads(d['params'] or '[]')
        fill = {}
        for p in params:
            fill[p['name']] = overrides.get(p['name'], p.get('default', ''))
        for k, v in fill.items():
            code = code.replace("{{" + k + "}}", v)

        # If promoted, show the SKILL function call instead
        if d.get('promoted_to'):
            proc = d['promoted_to']
            arg_str = " ".join(str(fill[p['name']]) for p in params)
            print(f"FUNCTION: {proc}({arg_str})")
            print(f"  (promoted from snippet, load vcli_snippets.il first)")
        else:
            print(f"SKILL: {code}")

        now = datetime.now().isoformat()
        if do_execute:
            import subprocess, time
            t0 = time.time()
            try:
                if use_ssh:
                    # Remote: SSH to compute host
                    sess = session or "dean-user1-37749"
                    escaped = code.replace('"', '\\"')
                    run_cmd = [
                        "ssh", "-i", str(Path.home() / ".ssh" / "id_rsa"),
                        "user1@192.168.1.111",
                        f'export HOME=/home/user1; export PATH=/usr/bin:/bin:/home/user1/.local/bin:$PATH; '
                        f'vcli --session {sess} skill exec "{escaped}" 2>/dev/null'
                    ]
                else:
                    # Local: direct vcli on this machine
                    sess = session or ""
                    run_cmd = ["vcli"]
                    if sess:
                        run_cmd += ["--session", sess]
                    run_cmd += ["skill", "exec", code]

                r = subprocess.run(run_cmd, capture_output=True, timeout=15)
                elapsed = int((time.time() - t0) * 1000)
                out = r.stdout.decode('utf-8', errors='replace')
                err = r.stderr.decode('utf-8', errors='replace')
                idx = out.find('{')
                if idx >= 0:
                    data = json.loads(out[idx:])
                    status = data.get("status", "")
                    errs = data.get("errors", [])
                    ok = (status == "success" and not errs)
                    result = data.get("output", "")[:80]
                else:
                    ok = False
                    result = (out or err)[:80]
            except Exception as e:
                elapsed = int((time.time() - t0) * 1000)
                ok = False
                result = str(e)[:80]

            print(f"Result: {'OK' if ok else 'FAIL'} ({elapsed}ms, {'ssh' if use_ssh else 'local'}) — {result}")

            # Record execution
            c.execute("""CREATE TABLE IF NOT EXISTS snippet_executions (
                id INTEGER PRIMARY KEY AUTOINCREMENT, snippet_name TEXT,
                success INTEGER, duration_ms INTEGER, error_message TEXT,
                params_used TEXT, executed_at TEXT)""")
            c.execute("""INSERT INTO snippet_executions
                (snippet_name, success, duration_ms, error_message, params_used, executed_at)
                VALUES (?, ?, ?, ?, ?, ?)""",
                (name, int(ok), elapsed, None if ok else result,
                 json.dumps(fill), now))
            if ok:
                c.execute("UPDATE snippets SET success_count=success_count+1, last_used=? WHERE name=?",
                          (now, name))
                # Auto-learn: record used params to param_examples
                for k, v in fill.items():
                    if v:  # only record non-empty values
                        c.execute("""INSERT OR IGNORE INTO param_examples
                            (function_name, param_name, example_value, success_count, last_used)
                            VALUES (?, ?, ?, 1, ?)""",
                            (name, k, v, now))
                        c.execute("""UPDATE param_examples SET success_count=success_count+1, last_used=?
                            WHERE function_name=? AND param_name=? AND example_value=?""",
                            (now, name, k, v))
            else:
                c.execute("UPDATE snippets SET fail_count=fail_count+1, last_used=? WHERE name=?",
                          (now, name))
                # Auto-learn: record failure in error_history
                c.execute("""CREATE TABLE IF NOT EXISTS error_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, function_name TEXT,
                    error_type TEXT, error_message TEXT, fixed INTEGER DEFAULT 0,
                    created_at TEXT)""")
                c.execute("""INSERT OR IGNORE INTO error_history
                    (function_name, error_type, error_message, created_at)
                    VALUES (?, 'snippet_failure', ?, ?)""",
                    (name, result[:200], now))
            c.commit()
        else:
            c.execute("UPDATE snippets SET last_used=? WHERE name=?", (now, name))
            c.commit()
        return

    if cmd == "pipeline" and len(argv) > 1:
        # Execute multiple snippets in sequence
        names = argv[1:]
        overrides = {}
        do_execute = False
        use_ssh = False
        session = None
        # Separate flags from snippet names
        clean_names = []
        for a in names:
            if a == "--execute":
                do_execute = True
            elif a == "--ssh":
                use_ssh = True
            elif a.startswith("--session="):
                session = a.split("=", 1)[1]
            elif a.startswith("--"):
                k, _, v = a[2:].partition("=")
                overrides[k] = v
            else:
                clean_names.append(a)

        print(f"=== Pipeline: {' -> '.join(clean_names)} ===\n")
        all_ok = True
        for snip_name in clean_names:
            row = c.execute("SELECT * FROM snippets WHERE name=?", (snip_name,)).fetchone()
            if not row:
                print(f"  [SKIP] {snip_name} not found")
                all_ok = False
                continue
            d = dict(row)
            code = d['code']
            params = json.loads(d['params'] or '[]')
            fill = {}
            for p in params:
                fill[p['name']] = overrides.get(p['name'], p.get('default', ''))
            for k, v in fill.items():
                code = code.replace("{{" + k + "}}", v)
            print(f"  {snip_name}: {code}")
        print()
        if do_execute:
            print("  (execution of pipeline not yet implemented — run snippets individually)")
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

    if cmd == "pipeline" and not argv[1:]:
        print("Usage: rsi snippet pipeline NAME1 NAME2 ... [--k=v ...]")
        return

    if cmd == "run" and len(argv) < 2:
        print("Usage: rsi snippet run NAME [--k=v ...] [--execute] [--ssh]")
        return

    if cmd == "record" and len(argv) > 1:
        sub = argv[1]
        if sub == "start":
            c.execute("""CREATE TABLE IF NOT EXISTS recordings (
                id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT,
                started_at TEXT, active INTEGER DEFAULT 1)""")
            c.execute("INSERT INTO recordings (name, started_at, active) VALUES (?, ?, 1)",
                      (argv[2] if len(argv) > 2 else "session", datetime.now().isoformat()))
            rid = c.execute("SELECT last_insert_rowid()").fetchone()[0]
            c.commit()
            print(f"Recording started (id={rid})")
            print("  Execute snippets with --execute, then: rsi snippet record stop")
        elif sub == "stop":
            row = c.execute("SELECT * FROM recordings WHERE active=1 ORDER BY id DESC LIMIT 1").fetchone()
            if not row:
                print("No active recording. Start with: rsi snippet record start")
                return
            rid = row[0]
            c.execute("UPDATE recordings SET active=0 WHERE id=?", (rid,))
            c.commit()
            evts = c.execute("""SELECT snippet_name, success, duration_ms, executed_at
                FROM snippet_executions WHERE executed_at >= ? ORDER BY id""",
                (row[2],)).fetchall()
            print(f"\nRecording '{row[1]}' (id={rid}): {len(evts)} operations")
            ok_count = sum(1 for e in evts if e[1])
            for e in evts:
                print(f"  {'OK' if e[1] else 'FAIL'} {e[0]:20s} {e[2]}ms  {e[3][:19]}")
            print(f"\n  Success: {ok_count}/{len(evts)}")
            # Auto-generate pipeline preview
            ok_names = [e[0] for e in evts if e[1]]
            if ok_names:
                print(f"  Pipeline: rsi snippet pipeline {' '.join(ok_names)}")
            print(f"  View:   rsi snippet record show {rid}")
            print(f"  Replay: rsi snippet record play {rid}")
        elif sub == "list":
            rows = c.execute("SELECT id, name, started_at, active FROM recordings ORDER BY id DESC LIMIT 10").fetchall()
            print(f"=== Recordings ({len(rows)}) ===")
            for r in rows:
                status = "REC" if r[3] else "done"
                print(f"  #{r[0]:<3d} {r[1]:20s} {r[2][:19]}  [{status}]")
        elif sub == "show" and len(argv) > 2:
            try:
                rid = int(argv[2])
            except ValueError:
                print("Error: recording ID must be a number")
                return
            row = c.execute("SELECT * FROM recordings WHERE id=?", (rid,)).fetchone()
            if not row:
                print(f"Recording #{rid} not found")
                return
            evts = c.execute("""SELECT snippet_name, success, duration_ms, params_used
                FROM snippet_executions WHERE executed_at >= ? ORDER BY id""",
                (row[2],)).fetchall()
            print(f"=== Recording #{rid}: {row[1]} ===")
            print(f"Started: {row[2]}")
            print(f"Operations: {len(evts)}\n")
            for i, e in enumerate(evts, 1):
                mark = "OK" if e[1] else "FAIL"
                print(f"  {i}. {e[0]:20s} [{mark}] {e[2]}ms")
                if e[3]:
                    print(f"     params: {e[3][:80]}")
        elif sub == "play" and len(argv) > 2:
            try:
                rid = int(argv[2])
            except ValueError:
                print("Error: recording ID must be a number")
                return
            row = c.execute("SELECT * FROM recordings WHERE id=?", (rid,)).fetchone()
            if not row:
                print(f"Recording #{rid} not found")
                return
            evts = c.execute("SELECT snippet_name FROM snippet_executions WHERE executed_at >= ? ORDER BY id",
                (row[2],)).fetchall()
            names = [e[0] for e in evts]
            print(f"=== Replay #{rid}: {' → '.join(names)} ===")
            print("  (replay not yet implemented — run snippets individually)")
        else:
            print("Usage: rsi snippet record start [NAME] | stop | list | show ID | play ID")
        return

    print("Commands: list | search KEYWORD | info NAME | run NAME | pipeline NAMES | save NAME DESC CODE | record start|stop")


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
            # Show version differences
            vers = c.execute(
                "SELECT version, syntax FROM fnd_functions WHERE name=? ORDER BY version",
                (parsed.function,)).fetchall()
            if len(vers) > 1:
                syns = {v: s for v, s in vers}
                cur = syns.get(parsed.version, "")
                others = [(v, s) for v, s in vers if v != parsed.version and s != cur]
                if others:
                    print(f"\n  Version diff:")
                    for v, s in others:
                        print(f"    {v}: {s[:80]}")
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
        # Search functions
        results = search(c, parsed.query, parsed.version, parsed.limit)
        print(f"=== Functions: '{parsed.query}' ===\n")
        if not results:
            print("  (no functions found)")
        for r in results:
            print(f"  {r['name']}")
            print(f"    {r['description'][:70]}")
            print()

        # Search snippets
        q = parsed.query.lower()
        snippet_matches = []
        for r in c.execute("SELECT name, category, description, success_count, fail_count FROM snippets").fetchall():
            name, cat, desc = r[0].lower(), r[1].lower(), r[2].lower()
            score = 0
            for word in q.split():
                if word in name: score += 10
                if word in desc: score += 5
                if word in cat: score += 2
            if score > 0:
                snippet_matches.append((score, r))
        snippet_matches.sort(key=lambda x: -x[0])

        if snippet_matches:
            print(f"=== Snippets: {len(snippet_matches)} found ===\n")
            for score, r in snippet_matches[:5]:
                total = r[3] + r[4]
                usage = f" [used {r[3]}/{total}]" if total > 0 else ""
                print(f"  {r[0]:20s} {r[2]}{usage}")
                print(f"    → rsi snippet run {r[0]}")
            print()
        return

    ap.print_help()

if __name__ == "__main__":
    main()


