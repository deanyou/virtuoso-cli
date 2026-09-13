#!/usr/bin/env python3
"""Generate RSI status report HTML."""
import sqlite3, json
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"
OUT = Path(__file__).parent / "rsi_report.html"

conn = sqlite3.connect(str(DB))
conn.row_factory = sqlite3.Row

total = conn.execute("SELECT COUNT(*) FROM functions").fetchone()[0]
verified = conn.execute("SELECT COUNT(*) FROM functions WHERE confidence='verified'").fetchone()[0]
discovered = conn.execute("SELECT COUNT(*) FROM functions WHERE confidence='discovered'").fetchone()[0]

# Categories
cats = {}
for r in conn.execute("SELECT category, COUNT(*) as cnt FROM functions GROUP BY category ORDER BY cnt DESC LIMIT 15"):
    cats[r["category"]] = r["cnt"]

# Verified functions
verified_list = [dict(r) for r in conn.execute(
    "SELECT name, signature FROM functions WHERE confidence='verified' ORDER BY name"
)]

# Recent pitfalls (from known issues)
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
    ("Wrong 'unavailable' list", "dbCreateLabel/Pin/Contact/Text ALL exist."),
]

conn.close()

# Build HTML
html = f"""<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>RSI Report</title>
<style>
body{{font-family:system-ui;background:#0f1117;color:#e4e4e7;padding:24px;max-width:900px;margin:auto}}
h1{{color:#8b5cf6}} .stat{{display:inline-block;margin:8px 16px 8px 0;padding:12px 20px;background:#1a1a2e;border-radius:10px}}
.stat .n{{font-size:28px;font-weight:700;color:#8b5cf6}} .stat .l{{font-size:12px;color:#9ca3af}}
.card{{background:#1a1a2e;border:1px solid #2a2a4a;border-radius:12px;padding:16px;margin:16px 0}}
.card h2{{font-size:14px;color:#9ca3af;text-transform:uppercase;letter-spacing:1px}}
.bar{{display:flex;align-items:center;gap:8px;margin:4px 0}}
.bar .n{{width:70px;font-size:12px}} .bar .b{{flex:1;height:14px;background:#2a2a4a;border-radius:3px}}
.bar .f{{height:100%;background:linear-gradient(90deg,#8b5cf6,#6366f1)}} .bar .c{{width:24px;font-size:11px}}
table{{width:100%;border-collapse:collapse;font-size:13px}}
td,th{{padding:6px 10px;border-bottom:1px solid #2a2a4a;text-align:left}}
th{{color:#9ca3af;font-weight:400}} code{{color:#a5b4fc;font-family:monospace}}
.pit{{padding:8px;background:#1a0a0a;border-left:3px solid #ef4444;border-radius:4px;margin:6px 0;font-size:12px}}
.pit b{{color:#f87171}}
</style></head><body>
<h1>Virtuoso SKILL RSI Report</h1>
<p style="color:#9ca3af">Recursive Self-Improvement Knowledge Base — Generated automatically</p>

<div class="card">
<h2>Overview</h2>
<div class="stat"><div class="n">{total}</div><div class="l">Total Functions</div></div>
<div class="stat"><div class="n" style="color:#52c41a">{verified}</div><div class="l">Verified</div></div>
<div class="stat"><div class="n" style="color:#faad14">{discovered}</div><div class="l">Discovered</div></div>
</div>

<div class="card">
<h2>Categories</h2>
{"".join(f'<div class="bar"><span class="n">{k}</span><div class="b"><div class="f" style="width:{v/max(cats.values())*100}%"></div></div><span class="c">{v}</span></div>' for k,v in cats.items())}
</div>

<div class="card">
<h2>Verified Functions (production-ready)</h2>
<table><tr><th>Function</th><th>Signature</th></tr>
{"".join(f'<tr><td><code>{f["name"]}</code></td><td>{f["signature"] or "—"}</td></tr>' for f in verified_list)}
</table></div>

<div class="card">
<h2>Pitfalls — Never Repeat</h2>
{"".join(f'<div class="pit"><b>{t}</b><br>{d}</div>' for t,d in pitfalls)}
</div>

<div class="card">
<h2>RSI Efficiency</h2>
<table>
<tr><th>Metric</th><th>Value</th></tr>
<tr><td>First run (enumeration)</td><td>~300s for {total} functions</td></tr>
<tr><td>Second run (query)</td><td>~0.02s</td></tr>
<tr><td>Speedup</td><td>~15,000x</td></tr>
<tr><td>Verification</td><td><code>python3 scripts/verify_db.py</code></td></tr>
</table></div>

</body></html>"""

OUT.write_text(html, encoding="utf-8")
print(f"Report: {OUT} ({OUT.stat().st_size} bytes)")
