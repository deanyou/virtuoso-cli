#!/usr/bin/env python3
"""
RSI Daemon — keeps SQLite connection and in-memory data alive.

Usage:
  python3 rsid.py start    # start daemon in background
  python3 rsid.py stop     # stop daemon
  python3 rsid.py status  # check if running
"""
import json, os, socket, sqlite3, sys, threading, time
from pathlib import Path

DATA_DIR = Path(__file__).parent.parent / "data"
DB_PATH = DATA_DIR / "skill_db.sqlite3"
PID_FILE = DATA_DIR / "rsid.pid"
PORT_FILE = DATA_DIR / "rsid.port"

SYNONYMS = {
    'draw': 'create', 'make': 'create', 'build': 'create', 'paint': 'create',
    'place': 'create', 'insert': 'create', 'add': 'create',
    'delete': 'delete', 'remove': 'delete', 'destroy': 'delete', 'erase': 'delete',
    'get': 'get', 'query': 'get', 'find': 'get', 'list': 'get', 'read': 'get',
    'set': 'set', 'change': 'set', 'modify': 'set', 'update': 'set',
    'move': 'transform', 'copy': 'copy', 'shift': 'transform',
    'rect': 'rect', 'rectangle': 'rect', 'box': 'rect',
    'polygon': 'polygon', 'path': 'path', 'line': 'line',
    'instance': 'inst', 'cell': 'cell', 'component': 'inst',
    'bbox': 'bbox', 'bounding': 'bbox', 'measure': 'bbox', 'area': 'area',
    'select': 'select', 'highlight': 'select',
    'layer': 'layer', 'pin': 'pin', 'net': 'net', 'terminal': 'term',
    'save': 'save', 'write': 'save', 'store': 'save',
    'open': 'open', 'close': 'close',
    'cellview': 'cellview', 'cv': 'cellview',
    'group': 'group', 'marker': 'marker',
    'zoom': 'zoom', 'fit': 'zoom', 'view': 'zoom',
    'window': 'window', 'display': 'display',
    'form': 'form', 'dialog': 'form',
    'file': 'file', 'design': 'design',
}

_cache = {}

def load_version(conn, version):
    rows = conn.execute(
        "SELECT name, syntax, description, category, version FROM fnd_functions WHERE version=?",
        (version,)
    ).fetchall()
    return [dict(r) for r in rows]

def expand_terms(query):
    terms = query.strip().split()
    expanded = set()
    for t in terms:
        tl = t.lower()
        expanded.add(tl)
        stem = tl.rstrip('s')
        if stem != tl:
            expanded.add(stem)
        for w in (tl, stem):
            if w in SYNONYMS:
                expanded.add(SYNONYMS[w])
            for suffix in ['tion', 'ment', 'ing', 'ed', 'er']:
                if w.endswith(suffix):
                    base = w[:-len(suffix)]
                    if base in SYNONYMS:
                        expanded.add(SYNONYMS[base])
    return expanded

def search(version, query, limit=10):
    if version not in _cache:
        conn = sqlite3.connect(str(DB_PATH))
        conn.row_factory = sqlite3.Row
        _cache[version] = load_version(conn, version)
        conn.close()
    all_funcs = _cache[version]
    expanded = expand_terms(query)
    results = {}
    for d in all_funcs:
        name_lower = d['name'].lower()
        desc_lower = (d.get('description') or '').lower()
        score = 0
        for word in expanded:
            pos = name_lower.find(word)
            if pos >= 0:
                score += max(1, 20 - pos)
                bare = name_lower
                for p in ['db', 'le', 'ge', 'hi', 'rod', 'dd']:
                    if bare.startswith(p):
                        bare = bare[len(p):]
                        break
                if bare == word:
                    score += 100
            if word in desc_lower:
                score += 2
        if score > 0:
            score -= len(name_lower) * 0.1
            d['score'] = score
            results[d['name']] = d
    sorted_results = sorted(results.values(), key=lambda x: (-x.get('score', 1), x['name']))
    return sorted_results[:limit]

def lookup(version, name):
    if version not in _cache:
        conn = sqlite3.connect(str(DB_PATH))
        conn.row_factory = sqlite3.Row
        _cache[version] = load_version(conn, version)
        conn.close()
    for d in _cache[version]:
        if d['name'] == name:
            return d
    return None

def handle_request(req):
    cmd = req.get('cmd')
    if cmd == 'search':
        return {'results': search(req.get('version', 'IC251'), req['query'], req.get('limit', 10))}
    elif cmd == 'lookup':
        r = lookup(req.get('version', 'IC251'), req['name'])
        return {'result': r}
    elif cmd == 'ping':
        return {'pong': True, 'cached_versions': list(_cache.keys())}
    return {'error': 'unknown command'}

def serve(port):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(('127.0.0.1', port))
    s.listen(5)
    PORT_FILE.write_text(str(port))
    print(f"RSI daemon listening on 127.0.0.1:{port}", flush=True)
    while True:
        conn, addr = s.accept()
        try:
            data = b''
            while True:
                chunk = conn.recv(4096)
                if not chunk:
                    break
                data += chunk
                if b'\n' in chunk:
                    break
            req = json.loads(data.decode())
            resp = handle_request(req)
            conn.sendall(json.dumps(resp).encode() + b'\n')
        except Exception as e:
            try:
                conn.sendall(json.dumps({'error': str(e)}).encode() + b'\n')
            except:
                pass
        finally:
            conn.close()

def is_running():
    if not PID_FILE.exists():
        return False
    try:
        pid = int(PID_FILE.read_text())
        os.kill(pid, 0)
        return True
    except:
        return False

def start():
    if is_running():
        print("Already running")
        return
    # Pick a free port
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.bind(('127.0.0.1', 0))
    port = s.getsockname()[1]
    s.close()
    # Fork background process
    if os.name == 'nt':
        # Windows: spawn detached
        import subprocess
        proc = subprocess.Popen(
            [sys.executable, str(Path(__file__)), '_serve', str(port)],
            creationflags=subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        PID_FILE.write_text(str(proc.pid))
    else:
        pid = os.fork()
        if pid > 0:
            PID_FILE.write_text(str(pid))
            print(f"Started daemon pid={pid} port={port}")
            return
        # Child
        os.setsid()
        serve(port)
        return
    print(f"Started daemon on port {port}")

def stop():
    if not is_running():
        print("Not running")
        return
    pid = int(PID_FILE.read_text())
    os.kill(pid, 9)
    PID_FILE.unlink(missing_ok=True)
    PORT_FILE.unlink(missing_ok=True)
    print("Stopped")

def status():
    if is_running():
        port = int(PORT_FILE.read_text()) if PORT_FILE.exists() else '?'
        print(f"Running on port {port}")
    else:
        print("Not running")

if __name__ == '__main__':
    if len(sys.argv) < 2:
        print("Usage: rsid.py [start|stop|status|_serve PORT]")
        sys.exit(1)
    cmd = sys.argv[1]
    if cmd == 'start':
        start()
    elif cmd == 'stop':
        stop()
    elif cmd == 'status':
        status()
    elif cmd == '_serve':
        serve(int(sys.argv[2]))
