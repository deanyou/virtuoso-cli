#!/usr/bin/env python3
"""Generate RSI status report HTML with tabs."""
import sqlite3, json, sys
from pathlib import Path

# Add evidence scripts path for mine_candidates and decisions
EVIDENCE_DIR = Path(__file__).parent / "evidence"
if str(EVIDENCE_DIR) not in sys.path:
    sys.path.insert(0, str(EVIDENCE_DIR))

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"
OUT = Path(__file__).parent.parent / "report" / "rsi_report.html"

conn = sqlite3.connect(str(DB))
conn.row_factory = sqlite3.Row

# Stats
total_fnd = conn.execute("SELECT COUNT(*) FROM fnd_functions").fetchone()[0]
by_version = {}
for r in conn.execute("SELECT version, COUNT(*) as c FROM fnd_functions GROUP BY version"):
    by_version[r["version"]] = r["c"]

manual_total = conn.execute("SELECT COUNT(*) FROM functions").fetchone()[0]
manual_verified = conn.execute("SELECT COUNT(*) FROM functions WHERE confidence='verified'").fetchone()[0]

# IC251 confidence breakdown
ic251_conf = {}
try:
    for r in conn.execute("SELECT confidence, COUNT(*) as c FROM fnd_functions WHERE version='IC251' GROUP BY confidence"):
        ic251_conf[r["confidence"]] = r["c"]
except: pass

syn_count = conn.execute("SELECT COUNT(*) FROM synonyms").fetchone()[0]

param_count = conn.execute("SELECT COUNT(*) FROM param_examples").fetchone()[0]
errors_open = conn.execute("SELECT COUNT(*) FROM error_history WHERE fixed=0").fetchone()[0]
errors_fixed = conn.execute("SELECT COUNT(*) FROM error_history WHERE fixed=1").fetchone()[0]

# Categories
cats = []
for r in conn.execute("SELECT category, COUNT(*) as cnt FROM fnd_functions GROUP BY category ORDER BY cnt DESC LIMIT 20"):
    cats.append((r["category"], r["cnt"]))

# Verified functions
verified_list = [dict(r) for r in conn.execute(
    "SELECT name, signature FROM functions WHERE confidence='verified' ORDER BY name"
)]

# Parameter examples by function
param_by_func = {}
for r in conn.execute("SELECT function_name, param_name, example_value, success_count FROM param_examples ORDER BY function_name, success_count DESC"):
    f = r["function_name"]
    if f not in param_by_func:
        param_by_func[f] = []
    param_by_func[f].dict() if False else param_by_func[f].append(dict(r))

# Recent errors
errors = [dict(r) for r in conn.execute(
    "SELECT function_name, error_type, error_message, created_at FROM error_history ORDER BY created_at DESC LIMIT 15"
)]

# RSI progress history
progress = [dict(r) for r in conn.execute(
    "SELECT date, phase, milestone, detail, functions_added FROM rsi_progress ORDER BY id"
)]

# Snippets with execution stats
snippets = []
for r in conn.execute("""
    SELECT name, category, description, success_count, fail_count, last_used, promoted_to
    FROM snippets ORDER BY category, name
"""):
    d = dict(r)
    total = d['success_count'] + d['fail_count']
    d['total'] = total
    d['rate'] = round(d['success_count'] / total * 100) if total > 0 else None
    snippets.append(d)

# Recordings
recordings = [dict(r) for r in conn.execute("""
    SELECT id, name, started_at, active FROM recordings ORDER BY id DESC LIMIT 10
""")]

# Recent snippet executions
snippet_runs = [dict(r) for r in conn.execute("""
    SELECT snippet_name, success, duration_ms, error_message, executed_at
    FROM snippet_executions ORDER BY executed_at DESC LIMIT 10
""")]

# Replay insights (Dream-RSI)
insights = [dict(r) for r in conn.execute("""
    SELECT insight_type, pattern, context, success_rate, sample_count, recommendation, created_at
    FROM replay_insights ORDER BY sample_count DESC
""")]

# All functions for search (IC251 only, top 2000 by name)
search_funcs = [dict(r) for r in conn.execute("""
    SELECT name, syntax, description, category, confidence
    FROM fnd_functions WHERE version='IC251' ORDER BY name LIMIT 2000
""")]
# All snippets for search
search_snippets = [dict(r) for r in conn.execute("""
    SELECT name, description, category, code, promoted_to
    FROM snippets ORDER BY name
""")]

# Experience Platform: cases + events (Phase B.2)
exp_cases = []
exp_stats = {"gold": 0, "negative": 0, "unknown": 0, "total": 0}
exp_top_failures = []
try:
    for r in conn.execute("SELECT * FROM experience_cases ORDER BY created_at DESC LIMIT 200"):
        d = dict(r)
        vs = d.get("verification_status", "")
        ct = d.get("case_type", "unknown")
        if vs == "PASSED" and ct in ("success", "recovery"):
            pool = "GOLD"; exp_stats["gold"] += 1
        elif vs == "FAILED":
            pool = "NEGATIVE"; exp_stats["negative"] += 1
        else:
            pool = "UNKNOWN"; exp_stats["unknown"] += 1
        d["pool"] = pool
        exp_cases.append(d)
    exp_stats["total"] = exp_stats["gold"] + exp_stats["negative"] + exp_stats["unknown"]
    for r in conn.execute("SELECT failure_type, COUNT(*) as cnt FROM experience_cases WHERE failure_type IS NOT NULL GROUP BY failure_type ORDER BY cnt DESC LIMIT 10"):
        exp_top_failures.append((r["failure_type"], r["cnt"]))
except Exception:
    pass

# Common pitfalls (hardcoded from experience)
pitfalls = [
    ("SQLite reserved word", "Column 'exists' causes syntax error. Use 'func_exists'."),
    ("Python 3.6 compat", "No capture_output kwarg. Use stdout=PIPE, stderr=PIPE."),
    ("Motif/Xt input", "Finder search field ignores XTest events. GUI input limited."),
    ("F1 != Help", "Opens Find/Replace. Help via startFinder() only."),
    ("dbCreateRect args", "layer+purpose MUST be list('L' 'P'), not two strings."),
    ("let double parens", "let(((v e)) body) — always double parens."),
    ("Instance ~>insts lag", "dbCreateInst returns but cv~>insts may show 0."),
    ("Zero-area rect", "Silently returns nil. Always check bbox != 0."),
    ("vcli timeout", "Dead session hangs. Check port alive first."),
    ("SSH quoting", "Double quotes in SKILL need \\\" escape over SSH."),
    ("LIKE case-sensitive", "SQLite LIKE is case-sensitive. Use LOWER()."),
    ("errors[] not output[]", "vcli errors are in errors[] array, not output."),
]

# P3: Candidate decisions for review
import html

def _esc(s):
    """Escape HTML special characters."""
    if s is None:
        return ""
    return html.escape(str(s), quote=True)

candidates_data = {"candidates": [], "adopted": [], "total_cands": 0}
try:
    from mine_candidates import mine
    from record_intervention import get_adopted_candidates, list_candidate_decisions
    
    result = mine(min_verified=0, db_path=DB)
    decisions = list_candidate_decisions(include_revoked=True, db_path=DB)
    adopted = get_adopted_candidates(db_path=DB)
    
    # Build decision map
    decision_map = {}
    for d in decisions:
        key = (d["candidate_id"], d["snapshot_hash"])
        decision_map[key] = d
    
    # Attach decision status to each candidate
    snapshot_hash = result.get("h", "")
    
    # Count adopted in CURRENT snapshot only
    adopted_current = [d for d in adopted if d.get("snapshot_hash") == snapshot_hash]
    
    for c in result.get("candidates", []):
        cand_id = c.get("candidate_id", "")
        dec = decision_map.get((cand_id, snapshot_hash), {})
        if dec:
            c["decision_status"] = dec["decision"]
            c["decision_revoked"] = dec.get("revoked_at")
            c["decision_reason"] = dec.get("reason")
        else:
            c["decision_status"] = "PENDING"
            c["decision_revoked"] = None
            c["decision_reason"] = None
    
    candidates_data = {
        "candidates": result.get("candidates", []),
        "adopted": adopted_current,  # Only current snapshot
        "total_cands": result.get("total_cands", 0),
        "snapshot_hash": snapshot_hash,
    }
except Exception as e:
    candidates_data = {"error": str(e), "candidates": [], "adopted": [], "total_cands": 0}

conn.close()

# Build HTML with tabs
html = f"""<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>RSI Report — Virtuoso SKILL Knowledge Base</title>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{ font-family: system-ui, -apple-system, sans-serif; background: #0f1117; color: #e4e4e7; padding: 20px; }}
.container {{ max-width: 1100px; margin: 0 auto; }}
h1 {{ color: #8b5cf6; font-size: 22px; margin-bottom: 4px; }}
.subtitle {{ color: #6b7280; font-size: 13px; margin-bottom: 20px; }}

/* Tabs */
.tab-nav {{ display: flex; gap: 4px; border-bottom: 1px solid #2a2a4a; margin-bottom: 20px; flex-wrap: wrap; }}
.tab-btn {{ padding: 10px 18px; background: transparent; border: none; color: #9ca3af; cursor: pointer; font-size: 13px; border-bottom: 2px solid transparent; transition: all 0.2s; }}
.tab-btn:hover {{ color: #e4e4e7; }}
.tab-btn.active {{ color: #8b5cf6; border-bottom-color: #8b5cf6; }}
.tab-panel {{ display: none; }}
.tab-panel.active {{ display: block; }}

/* Cards */
.card {{ background: #1a1a2e; border: 1px solid #2a2a4a; border-radius: 12px; padding: 16px; margin-bottom: 16px; }}
.card h2 {{ font-size: 13px; color: #9ca3af; text-transform: uppercase; letter-spacing: 1px; margin-bottom: 12px; }}

/* Stats */
.stats {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: 12px; margin-bottom: 16px; }}
.stat {{ background: #1a1a2e; border: 1px solid #2a2a4a; border-radius: 10px; padding: 14px; text-align: center; }}
.stat .n {{ font-size: 24px; font-weight: 700; color: #8b5cf6; }}
.stat .n.green {{ color: #52c41a; }}
.stat .n.yellow {{ color: #faad14; }}
.stat .n.red {{ color: #ef4444; }}
.stat .l {{ font-size: 11px; color: #9ca3af; margin-top: 2px; }}

/* Bars */
.bar {{ display: flex; align-items: center; gap: 8px; margin: 4px 0; }}
.bar .n {{ width: 120px; font-size: 11px; text-align: right; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
.bar .b {{ flex: 1; height: 14px; background: #2a2a4a; border-radius: 3px; overflow: hidden; }}
.bar .f {{ height: 100%; background: linear-gradient(90deg, #8b5cf6, #6366f1); border-radius: 3px; }}
.bar .c {{ width: 40px; font-size: 11px; text-align: right; }}

/* Tables */
table {{ width: 100%; border-collapse: collapse; font-size: 12px; }}
td, th {{ padding: 6px 10px; border-bottom: 1px solid #2a2a4a; text-align: left; }}
th {{ color: #9ca3af; font-weight: 400; font-size: 11px; text-transform: uppercase; }}
code {{ color: #a5b4fc; font-family: 'Cascadia Code', monospace; }}

/* Pitfalls */
.pit {{ padding: 8px 10px; background: #1a0a0a; border-left: 3px solid #ef4444; border-radius: 4px; margin: 6px 0; font-size: 12px; }}
.pit b {{ color: #f87171; }}

/* Param examples */
.param {{ padding: 6px 10px; background: #0a1a0a; border-left: 3px solid #52c41a; border-radius: 4px; margin: 4px 0; font-size: 12px; }}
.param b {{ color: #52c41a; }}

/* Search */
.search-box {{ margin-bottom: 16px; }}
.search-box input {{ width: 100%; padding: 10px 14px; background: #1a1a2e; border: 1px solid #2a2a4a; border-radius: 8px; color: #e4e4e7; font-size: 13px; }}
.search-box input:focus {{ outline: none; border-color: #8b5cf6; }}

/* Candidates */
.cand-card {{ background: #1a1a2e; border: 1px solid #2a2a4a; border-radius: 12px; padding: 14px; margin-bottom: 12px; }}
.cand-header {{ display: flex; justify-content: space-between; align-items: center; margin-bottom: 10px; }}
.cand-id {{ font-family: monospace; color: #a5b4fc; font-size: 11px; }}
.cand-status {{ padding: 3px 10px; border-radius: 12px; font-size: 11px; font-weight: 600; }}
.cand-status.adopted {{ background: #052c16; color: #52c41a; }}
.cand-status.rejected {{ background: #2d0000; color: #ef4444; }}
.cand-status.pending {{ background: #2a2010; color: #faad14; }}
.cand-stats {{ display: flex; gap: 16px; font-size: 12px; color: #9ca3af; }}
.cand-stats span {{ color: #e4e4e7; font-weight: 600; }}
.cand-refs {{ margin-top: 10px; font-size: 11px; color: #6b7280; }}
.cand-refs code {{ color: #8b5cf6; }}

/* Expandable candidate cards */
details.cand-card {{ background: #1a1a2e; border: 1px solid #2a2a4a; border-radius: 12px; padding: 14px; margin-bottom: 12px; }}
details.cand-card summary {{ list-style: none; cursor: pointer; }}
details.cand-card summary::-webkit-details-marker {{ display: none; }}
</style></head><body>
<div class="container">
<h1>Virtuoso SKILL RSI</h1>
<p class="subtitle">Recursive Self-Improvement Knowledge Base — Auto-generated</p>

<div class="tab-nav">
  <button class="tab-btn active" onclick="showTab('overview')">Overview</button>
  <button class="tab-btn" onclick="showTab('progress')">Progress</button>
  <button class="tab-btn" onclick="showTab('snippets')">Snippets</button>
  <button class="tab-btn" onclick="showTab('insights')">Insights</button>
  <button class="tab-btn" onclick="showTab('search')">Search</button>
  <button class="tab-btn" onclick="showTab('recordings')">Recordings</button>
  <button class="tab-btn" onclick="showTab('categories')">Categories</button>
  <button class="tab-btn" onclick="showTab('params')">Param Templates</button>
  <button class="tab-btn" onclick="showTab('pitfalls')">Pitfalls</button>
  <button class="tab-btn" onclick="showTab('verified')">Verified</button>
  <button class="tab-btn" onclick="showTab('experience')">Experience</button>
  <button class="tab-btn" onclick="showTab('candidates')">Candidates</button>
</div>

<div id="tab-overview" class="tab-panel active">
  <div class="stats">
    <div class="stat"><div class="n">{total_fnd:,}</div><div class="l">FND Functions</div></div>
    <div class="stat"><div class="n green">{manual_verified}</div><div class="l">Manually Verified</div></div>
    <div class="stat"><div class="n yellow">{param_count}</div><div class="l">Param Templates</div></div>
    <div class="stat"><div class="n" style="color:#8b5cf6">{len(snippets)}</div><div class="l">Snippets</div></div>
    <div class="stat"><div class="n red">{errors_open}</div><div class="l">Open Errors</div></div>
  </div>
  <div class="card">
    <h2>By Version</h2>
    <table>
      <tr><th>Version</th><th>Functions</th><th>Status</th></tr>
      {"".join(f'<tr><td><code>{v}</code></td><td>{c:,}</td><td>{"Active" if v=="IC251" else "Baseline"}</td></tr>' for v, c in sorted(by_version.items()))}
    </table>
  </div>
  <div class="card">
    <h2>IC251 Verification Progress</h2>
    <table>
      <tr><th>Confidence</th><th>Count</th></tr>
      {''.join(f'<tr><td>{k}</td><td>{v:,}</td></tr>' for k, v in sorted(ic251_conf.items()))}
      <tr><td><b>Synonyms</b></td><td>{syn_count}</td></tr>
    </table>
  </div>
  <div class="card">
    <h2>Quick Search Suggestions</h2>
    <div style="display:flex;flex-wrap:wrap;gap:8px;margin-top:8px;">
      <span style="padding:6px 12px;background:#1a1a2e;border:1px solid #2a2a4a;border-radius:20px;font-size:12px;color:#8b5cf6;cursor:pointer;" onclick="document.getElementById('searchBox').value='draw polygon';document.getElementById('searchBox').dispatchEvent(new Event('input'));showTab('search');">draw polygon</span>
      <span style="padding:6px 12px;background:#1a1a2e;border:1px solid #2a2a4a;border-radius:20px;font-size:12px;color:#8b5cf6;cursor:pointer;" onclick="document.getElementById('searchBox').value='get cell';document.getElementById('searchBox').dispatchEvent(new Event('input'));showTab('search');">get cell</span>
      <span style="padding:6px 12px;background:#1a1a2e;border:1px solid #2a2a4a;border-radius:20px;font-size:12px;color:#8b5cf6;cursor:pointer;" onclick="document.getElementById('searchBox').value='create rect';document.getElementById('searchBox').dispatchEvent(new Event('input'));showTab('search');">create rect</span>
      <span style="padding:6px 12px;background:#1a1a2e;border:1px solid #2a2a4a;border-radius:20px;font-size:12px;color:#8b5cf6;cursor:pointer;" onclick="document.getElementById('searchBox').value='list layers';document.getElementById('searchBox').dispatchEvent(new Event('input'));showTab('search');">list layers</span>
      <span style="padding:6px 12px;background:#1a1a2e;border:1px solid #2a2a4a;border-radius:20px;font-size:12px;color:#8b5cf6;cursor:pointer;" onclick="document.getElementById('searchBox').value='select shape';document.getElementById('searchBox').dispatchEvent(new Event('input'));showTab('search');">select shape</span>
      <span style="padding:6px 12px;background:#1a1a2e;border:1px solid #2a2a4a;border-radius:20px;font-size:12px;color:#8b5cf6;cursor:pointer;" onclick="document.getElementById('searchBox').value='save design';document.getElementById('searchBox').dispatchEvent(new Event('input'));showTab('search');">save design</span>
    </div>
  </div>
  <div class="card">
    <h2>RSI Efficiency</h2>
    <table>
      <tr><th>Metric</th><th>Value</th></tr>
      <tr><td>First run (enumeration)</td><td>~300s</td></tr>
      <tr><td>Second run (query)</td><td>~0.02s</td></tr>
      <tr><td>Speedup</td><td>~15,000x</td></tr>
      <tr><td>Search</td><td><code>python3 rsi_query.py "draw polygon"</code></td></tr>
      <tr><td>Exact lookup</td><td><code>python3 rsi_query.py -f dbCreateRect</code></td></tr>
    </table>
  </div>
</div>

<div id="tab-progress" class="tab-panel">
  <div class="card">
    <h2>RSI Improvement Timeline</h2>
    <table>
      <tr><th>Date</th><th>Phase</th><th>Milestone</th><th>Detail</th><th>+Funcs</th></tr>
      {"".join(f'<tr><td style="white-space:nowrap">{p["date"]}</td><td><span style="color:{"#8b5cf6" if p["phase"]=="P0" else "#faad14"}">{p["phase"]}</span></td><td><b>{p["milestone"]}</b></td><td style="font-size:11px;color:#9ca3af">{p["detail"]}</td><td>{p["functions_added"]:,}</td></tr>' for p in progress)}
    </table>
  </div>
</div>

<div id="tab-snippets" class="tab-panel">
  <div class="card">
    <h2>SKILL Code Snippets — Execution Tracking</h2>
    <table>
      <tr><th>Snippet</th><th>Category</th><th>Desc</th><th>Promoted</th><th>Used</th><th>Success</th><th>Fail</th><th>Rate</th></tr>
      {"".join(f'''
      <tr>
        <td><code>{s["name"]}</code></td>
        <td>{s["category"]}</td>
        <td style="font-size:11px;color:#9ca3af">{s["description"][:50]}</td>
        <td style="color:#8b5cf6">{"→ " + s["promoted_to"] if s.get("promoted_to") else "—"}</td>
        <td>{s["total"]}</td>
        <td style="color:#52c41a">{s["success_count"]}</td>
        <td style="color:#ef4444">{s["fail_count"]}</td>
        <td>{f"{s['rate']}%" if s['rate'] is not None else "—"}</td>
      </tr>''' for s in snippets)}
    </table>
  </div>
  {"".join(f'''
  <div class="card">
    <h2>Recent Executions</h2>
    <table>
      <tr><th>Snippet</th><th>Result</th><th>Duration</th><th>Error</th><th>Time</th></tr>
      {"".join(f'<tr><td><code>{r["snippet_name"]}</code></td><td style="color:{"#52c41a" if r["success"] else "#ef4444"}">{"OK" if r["success"] else "FAIL"}</td><td>{r["duration_ms"] or "—"}ms</td><td style="font-size:11px">{(r["error_message"] or "")[:60]}</td><td style="font-size:11px">{r["executed_at"][:19]}</td></tr>' for r in snippet_runs)}
    </table>
  </div>''' if snippet_runs else '')}
</div>

<div id="tab-insights" class="tab-panel">
  <div class="card">
    <h2>Dream-RSI: Auto-Learned Insights ({len(insights)})</h2>
    <p style="font-size:12px;color:#6b7280;margin-bottom:12px;">Automatically discovered from exploration history via replay simulator.</p>
    {''.join(f'''
    <div style="padding:10px;background:{"#0a1a0a" if "pattern" in i["insight_type"] else "#1a0a0a" if "blocker" in i["insight_type"] else "#1a1a2e"};border-left:3px solid {"#52c41a" if "pattern" in i["insight_type"] else "#ef4444" if "blocker" in i["insight_type"] else "#8b5cf6"};border-radius:4px;margin:8px 0;">
      <div style="font-size:12px;"><b style="color:{"#52c41a" if "pattern" in i["insight_type"] else "#ef4444" if "blocker" in i["insight_type"] else "#8b5cf6"}">{i["pattern"]}</b>
      <span style="color:#6b7280;font-size:11px;"> in {i["context"]} · {i["sample_count"]} samples · {int(i["success_rate"]*100)}% success</span></div>
      <div style="font-size:11px;color:#9ca3af;margin-top:4px;">{i["recommendation"]}</div>
    </div>''' for i in insights) if insights else '<p style="color:#6b7280">No insights yet. Run dream_rsi.py to generate.</p>'}
  </div>
</div>

<div id="tab-search" class="tab-panel">
  <div class="card">
    <h2>Search Functions & Snippets</h2>
    <input type="text" id="searchBox" placeholder="Type to search..." style="width:100%;padding:10px;border-radius:8px;border:1px solid #3a3a5a;background:#1a1a2e;color:#e4e4e7;font-size:14px;box-sizing:border-box;">
    <div id="searchResults" style="margin-top:12px;max-height:500px;overflow-y:auto;"></div>
  </div>
  <script>
  (function() {{
    var funcs = __FUNCS_JSON__;
    var snips = __SNIPS_JSON__;
    var box = document.getElementById('searchBox');
    var out = document.getElementById('searchResults');
    if (!box || !out) return;
    function render(q) {{
      q = q.toLowerCase();
      if (!q) {{ out.innerHTML = '<div style="color:#6b7280;font-size:13px;">Type to search {len(search_funcs)} functions + {len(search_snippets)} snippets...</div>'; return; }}
      var html = '';
      var fc = 0, sc = 0;
      funcs.forEach(function(f) {{
        if (fc >= 20) return;
        if (f.name.toLowerCase().indexOf(q) >= 0 || (f.description||'').toLowerCase().indexOf(q) >= 0) {{
          html += '<div style="padding:6px 0;border-bottom:1px solid #2a2a4a;">';
          html += '<code style="color:#8b5cf6;">' + f.name + '</code> ';
          html += '<span style="font-size:11px;color:#6b7280;">[' + (f.confidence||'?') + ']</span><br>';
          html += '<span style="font-size:11px;color:#9ca3af;">' + (f.syntax||'') + '</span>';
          html += '</div>';
          fc++;
        }}
      }});
      snips.forEach(function(s) {{
        if (sc >= 10) return;
        if (s.name.toLowerCase().indexOf(q) >= 0 || (s.description||'').toLowerCase().indexOf(q) >= 0) {{
          html += '<div style="padding:6px 0;border-bottom:1px solid #2a2a4a;">';
          html += '<code style="color:#52c41a;">snippet:' + s.name + '</code> ';
          if (s.promoted_to) html += '<span style="font-size:11px;color:#8b5cf6;">→ ' + s.promoted_to + '</span>';
          html += '<br><span style="font-size:11px;color:#9ca3af;">' + (s.description||'') + '</span>';
          if (s.code) html += '<br><code style="font-size:10px;color:#6b7280;display:block;margin-top:4px;padding:4px;background:#0f1117;border-radius:4px;">' + s.code.substring(0,100) + (s.code.length>100?'...':'') + '</code>';
          html += '</div>';
          sc++;
        }}
      }});
      if (!html) html = '<div style="color:#6b7280;font-size:13px;">No results for "' + q + '"</div>';
      out.innerHTML = html;
    }}
    box.addEventListener('input', function() {{ render(box.value); }});
    render('');
  }})();
  </script>
</div>

<div id="tab-recordings" class="tab-panel">
  <div class="card">
    <h2>Recording Sessions</h2>
    <table>
      <tr><th>ID</th><th>Name</th><th>Started</th><th>Status</th></tr>
      {"".join(f'<tr><td>{r["id"]}</td><td>{r["name"]}</td><td>{r["started_at"][:19]}</td><td>{"REC" if r["active"] else "done"}</td></tr>' for r in recordings)}
    </table>
  </div>
</div>

<div id="tab-categories" class="tab-panel">
  <div class="card">
    <h2>Function Categories (top 20)</h2>
    {"".join(f'<div class="bar"><span class="n">{k}</span><div class="b"><div class="f" style="width:{v/max(c[1] for c in cats)*100}%"></div></div><span class="c">{v:,}</span></div>' for k, v in cats)}
  </div>
</div>

<div id="tab-params" class="tab-panel">
  <div class="card">
    <h2>Known Working Parameter Templates</h2>
    {"".join(
        f'<div class="param"><b>{func}</b><br>' +
        "<br>".join(
            f"&nbsp;&nbsp;{p['param_name']}: {p['example_value']} ({p['success_count']}x)"
            for p in plist
        ) +
        "</div>"
        for func, plist in param_by_func.items()
    )}
  </div>
</div>

<div id="tab-pitfalls" class="tab-panel">
  <div class="card">
    <h2>Never Repeat These Mistakes</h2>
    {"".join(f'<div class="pit"><b>{t}</b><br>{d}</div>' for t, d in pitfalls)}
  </div>
  <div class="card">
    <h2>Recent Errors</h2>
    {"".join(f'<div class="pit"><b>{e["function_name"]}</b> [{e["error_type"]}]<br>{e["error_message"][:120]}<br><span style="color:#6b7280;font-size:10px">{e["created_at"][:10]}</span></div>' for e in errors) if errors else '<p style="color:#6b7280">No errors recorded.</p>'}
  </div>
</div>

<div id="tab-verified" class="tab-panel">
  <div class="card">
    <h2>Manually Verified Functions ({len(verified_list)})</h2>
    <div class="search-box"><input type="text" id="searchVerified" placeholder="Filter..." onkeyup="filterTable()"></div>
    <table id="verifiedTable">
      <tr><th>Function</th><th>Signature</th></tr>
      {"".join(f'<tr><td><code>{f["name"]}</code></td><td style="font-size:11px">{f["signature"] or "—"}</td></tr>' for f in verified_list)}
    </table>
  </div>
</div>

<div id="tab-experience" class="tab-panel">
  <div class="stats">
    <div class="stat"><div class="n">{exp_stats["total"]}</div><div class="l">Total Cases</div></div>
    <div class="stat"><div class="n green">{exp_stats["gold"]}</div><div class="l">GOLD</div></div>
    <div class="stat"><div class="n red">{exp_stats["negative"]}</div><div class="l">NEGATIVE</div></div>
    <div class="stat"><div class="n" style="color:#faad14">{exp_stats["unknown"]}</div><div class="l">UNKNOWN</div></div>
  </div>
  <div class="card">
    <h2>Top Failure Types</h2>
    {''.join(f'<div class="bar"><span class="n">{k}</span><div class="b"><div class="f" style="width:{v/(exp_top_failures[0][1] if exp_top_failures else 1)*100}%"></div></div><span class="c">{v}</span></div>' for k, v in exp_top_failures) if exp_top_failures else '<p style="color:#6b7280">No cases yet.</p>'}
  </div>
  <div class="card">
    <h2>Experience Cases (latest 200)</h2>
    <div class="search-box"><input type="text" id="searchExp" placeholder="Filter cases..." onkeyup="filterExpTable()"></div>
    <table id="expTable">
      <tr><th>Pool</th><th>Type</th><th>Failure</th><th>Verification</th><th>Outcome</th><th>Run ID</th></tr>
      {''.join(f'<tr><td><span style="color:{"#52c41a" if c["pool"]=="GOLD" else "#ef4444" if c["pool"]=="NEGATIVE" else "#faad14"};font-weight:600">{c["pool"]}</span></td><td>{c["case_type"]}</td><td style="font-size:11px">{c.get("failure_type") or "—"}</td><td style="font-size:11px">{c.get("verification_status") or "—"}</td><td style="font-size:11px">{c["outcome"]}</td><td style="font-size:10px;color:#6b7280">{c["run_id"]}</td></tr>' for c in exp_cases)}
    </table>
  </div>
</div>

<div id="tab-candidates" class="tab-panel">
  <div class="stats">
    <div class="stat"><div class="n">{candidates_data["total_cands"]}</div><div class="l">Total Candidates</div></div>
    <div class="stat"><div class="n green">{len(candidates_data.get("adopted", []))}</div><div class="l">Adopted (this snapshot)</div></div>
    <div class="stat"><div class="n yellow">{sum(1 for c in candidates_data.get("candidates", []) if c.get("decision_status") == "PENDING")}</div><div class="l">Pending</div></div>
    <div class="stat"><div class="n red">{sum(1 for c in candidates_data.get("candidates", []) if c.get("decision_status") == "REJECTED")}</div><div class="l">Rejected</div></div>
  </div>
  <div class="card">
    <h2>Snapshot: <code>{_esc(candidates_data.get("snapshot_hash", ""))}</code></h2>
    {f'<p style="color:#ef4444">Error: {_esc(str(candidates_data.get("error", "")))}</p>' if candidates_data.get("error") else ""}
    {''.join(f'''<details class="cand-card">
      <summary class="cand-header">
        <span class="cand-id">{_esc(c.get("candidate_id", "unknown"))}</span>
        <span class="cand-status {_esc(c.get("decision_status", "PENDING").lower())}">{_esc(c.get("decision_status", "PENDING"))}</span>
      </summary>
      <div class="cand-stats">
        <div>Attempts: <span>{c.get("stats", {}).get("attempts", 0)}</span></div>
        <div>Support: <span>{c.get("stats", {}).get("verified_count", 0)}</span></div>
        <div>Failed: <span>{c.get("stats", {}).get("failed_count", 0)}</span></div>
        <div>Conflict: <span>{c.get("stats", {}).get("conflict_count", 0)}</span></div>
        <div>Strength: <span>{_esc(c.get("strength", "unknown"))}</span></div>
      </div>
      <div class="cand-refs">
        Evidence refs: vrefs={len(c.get("vrefs", []))} frefs={len(c.get("frefs", []))} cref={len(c.get("cref", []))}
      </div>
      {f'<div class="cand-refs"><b>Decision reason:</b> {_esc(c.get("decision_reason", ""))}</div>' if c.get("decision_reason") else ""}
      <details>
        <summary style="cursor:pointer;color:#8b5cf6;font-size:11px;margin-top:8px">Show {len(c.get("vrefs", []))} verified / {len(c.get("frefs", []))} failed / {len(c.get("cref", []))} conflict refs</summary>
        <div style="font-size:10px;color:#6b7280;margin-top:6px">
          <div><b>VERIFIED ({len(c.get("vrefs", []))}):</b></div>
          {''.join(f'<div style="margin-left:10px">run={_esc(r.get("run",""))} fid={_esc(r.get("fid",""))} id={_esc(r.get("id",""))}</div>' for r in c.get("vrefs", []))}
          <div style="margin-top:6px"><b>FAILED ({len(c.get("frefs", []))}):</b></div>
          {''.join(f'<div style="margin-left:10px">run={_esc(r.get("run",""))} fid={_esc(r.get("fid",""))} id={_esc(r.get("id",""))}</div>' for r in c.get("frefs", []))}
          <div style="margin-top:6px"><b>CONFLICT ({len(c.get("cref", []))}):</b></div>
          {''.join(f'<div style="margin-left:10px">run={_esc(r.get("run",""))} fid={_esc(r.get("fid",""))} id={_esc(r.get("id",""))}</div>' for r in c.get("cref", []))}
        </div>
      </details>
    </details>''' for c in candidates_data.get("candidates", [])) if candidates_data.get("candidates") else '<p style="color:#6b7280">No candidates yet. Run "python mine_candidates.py mine" to generate candidates.</p>'}
  </div>
  <div class="card">
    <h2>CLI Commands for Review</h2>
    <pre style="background:#0f1117;padding:12px;border-radius:8px;font-size:11px;overflow-x:auto"># Review candidates (current snapshot: {candidates_data.get("snapshot_hash", "N/A")})
python scripts/evidence/record_intervention.py review --min-verified 1

# Adopt a candidate (use IDs from above)
python scripts/evidence/record_intervention.py adopt <candidate_id> {candidates_data.get("snapshot_hash", "<hash>")} --reason "..."

# Reject a candidate
python scripts/evidence/record_intervention.py reject <candidate_id> {candidates_data.get("snapshot_hash", "<hash>")} --reason "..."

# List decisions
python scripts/evidence/record_intervention.py list-decisions</pre>
  </div>
</div>

</div>
<script>
function showTab(name) {{
  document.querySelectorAll('.tab-panel').forEach(p => p.classList.remove('active'));
  document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
  document.getElementById('tab-' + name).classList.add('active');
  event.target.classList.add('active');
}}
function filterTable() {{
  var q = document.getElementById('searchVerified').value.toLowerCase();
  document.querySelectorAll('#verifiedTable tr').forEach(function(tr, i) {{
    if (i === 0) return;
    tr.style.display = tr.textContent.toLowerCase().includes(q) ? '' : 'none';
  }});
}}
function filterExpTable() {{
  var q = document.getElementById('searchExp').value.toLowerCase();
  document.querySelectorAll('#expTable tr').forEach(function(tr, i) {{
    if (i === 0) return;
    tr.style.display = tr.textContent.toLowerCase().includes(q) ? '' : 'none';
  }});
}}
</script>
</body></html>"""

html = html.replace("__FUNCS_JSON__", json.dumps(search_funcs, ensure_ascii=False))
html = html.replace("__SNIPS_JSON__", json.dumps(search_snippets, ensure_ascii=False))
OUT.write_text(html, encoding="utf-8")
print(f"Report: {OUT} ({OUT.stat().st_size} bytes)")
