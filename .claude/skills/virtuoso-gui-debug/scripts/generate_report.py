#!/usr/bin/env python3
"""Generate RSI status report HTML with tabs."""
import sqlite3, json
from pathlib import Path

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
    SELECT name, category, description, success_count, fail_count, last_used
    FROM snippets ORDER BY category, name
"""):
    d = dict(r)
    total = d['success_count'] + d['fail_count']
    d['total'] = total
    d['rate'] = round(d['success_count'] / total * 100) if total > 0 else None
    snippets.append(d)

# Recent snippet executions
snippet_runs = [dict(r) for r in conn.execute("""
    SELECT snippet_name, success, duration_ms, error_message, executed_at
    FROM snippet_executions ORDER BY executed_at DESC LIMIT 10
""")]

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
</style></head><body>
<div class="container">
<h1>Virtuoso SKILL RSI</h1>
<p class="subtitle">Recursive Self-Improvement Knowledge Base — Auto-generated</p>

<div class="tab-nav">
  <button class="tab-btn active" onclick="showTab('overview')">Overview</button>
  <button class="tab-btn" onclick="showTab('progress')">Progress</button>
  <button class="tab-btn" onclick="showTab('snippets')">Snippets</button>
  <button class="tab-btn" onclick="showTab('categories')">Categories</button>
  <button class="tab-btn" onclick="showTab('params')">Param Templates</button>
  <button class="tab-btn" onclick="showTab('pitfalls')">Pitfalls</button>
  <button class="tab-btn" onclick="showTab('verified')">Verified</button>
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
      <tr><th>Snippet</th><th>Category</th><th>Desc</th><th>Used</th><th>Success</th><th>Fail</th><th>Rate</th></tr>
      {"".join(f'''
      <tr>
        <td><code>{s["name"]}</code></td>
        <td>{s["category"]}</td>
        <td style="font-size:11px;color:#9ca3af">{s["description"][:50]}</td>
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
</script>
</body></html>"""

OUT.write_text(html, encoding="utf-8")
print(f"Report: {OUT} ({OUT.stat().st_size} bytes)")
