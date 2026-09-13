#!/usr/bin/env python3
"""Generate RSI status report HTML — includes fnd_functions (official docs)."""
import sqlite3, json
from pathlib import Path

DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"
OUT = Path(__file__).parent.parent / "report" / "rsi_report.html"

conn = sqlite3.connect(str(DB))
conn.row_factory = sqlite3.Row

# Manual enumeration stats
manual_total = conn.execute("SELECT COUNT(*) FROM functions").fetchone()[0]
manual_verified = conn.execute("SELECT COUNT(*) FROM functions WHERE confidence='verified'").fetchone()[0]

# Official fnd_functions stats
fnd_total = conn.execute("SELECT COUNT(*) FROM fnd_functions").fetchone()[0]
fnd_by_version = {}
for r in conn.execute("SELECT version, COUNT(*) as cnt FROM fnd_functions GROUP BY version ORDER BY cnt DESC"):
    fnd_by_version[r["version"]] = r["cnt"]

# Category breakdown for current target (IC251)
cats = {}
for r in conn.execute("SELECT category, COUNT(*) as cnt FROM fnd_functions WHERE version='IC251' GROUP BY category ORDER BY cnt DESC LIMIT 15"):
    cats[r["category"]] = r["cnt"]

# Version comparison
only_231 = conn.execute("SELECT COUNT(*) FROM fnd_functions WHERE version='IC231' AND name NOT IN (SELECT name FROM fnd_functions WHERE version='IC618')").fetchone()[0]
only_618 = conn.execute("SELECT COUNT(*) FROM fnd_functions WHERE version='IC618' AND name NOT IN (SELECT name FROM fnd_functions WHERE version='IC231')").fetchone()[0]
common = conn.execute("SELECT COUNT(DISTINCT a.name) FROM fnd_functions a JOIN fnd_functions b ON a.name=b.name AND a.version!=b.version").fetchone()[0]

# Verified functions (from manual enumeration)
verified_list = [dict(r) for r in conn.execute(
    "SELECT name, signature FROM functions WHERE confidence='verified' ORDER BY name"
)]

# Recent pitfalls
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
    ("vcli errors[] not output[]", "Enumeration bug: check data['errors'] not data['output']."),
]

conn.close()

max_cat = max(cats.values()) if cats else 1
cat_bars = "".join(
    f'<div class="bar"><span class="n">{k}</span><div class="b"><div class="f" style="width:{v/max_cat*100}%"></div></div><span class="c">{v}</span></div>'
    for k, v in cats.items()
)

version_cards = "".join(
    f'<div class="stat"><div class="n">{cnt:,}</div><div class="l">{ver}</div></div>'
    for ver, cnt in fnd_by_version.items()
)

verified_rows = "".join(
    f'<tr><td><code>{f["name"]}</code></td><td>{f["signature"] or "—"}</td></tr>'
    for f in verified_list
)

pitfall_html = "".join(
    f'<div class="pit"><b>{t}</b><br>{d}</div>' for t, d in pitfalls
)

html = f"""<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>RSI Report</title>
<style>
body{{font-family:system-ui;background:#0f1117;color:#e4e4e7;padding:24px;max-width:960px;margin:auto}}
h1{{color:#8b5cf6}} h2{{font-size:14px;color:#9ca3af;text-transform:uppercase;letter-spacing:1px}}
.stat{{display:inline-block;margin:8px 16px 8px 0;padding:12px 20px;background:#1a1a2e;border-radius:10px}}
.stat .n{{font-size:28px;font-weight:700;color:#8b5cf6}} .stat .l{{font-size:12px;color:#9ca3af}}
.card{{background:#1a1a2e;border:1px solid #2a2a4a;border-radius:12px;padding:16px;margin:16px 0}}
.bar{{display:flex;align-items:center;gap:8px;margin:4px 0}}
.bar .n{{width:140px;font-size:12px;text-align:right}} .bar .b{{flex:1;height:14px;background:#2a2a4a;border-radius:3px}}
.bar .f{{height:100%;background:linear-gradient(90deg,#8b5cf6,#6366f1)}} .bar .c{{width:40px;font-size:11px}}
table{{width:100%;border-collapse:collapse;font-size:13px}}
td,th{{padding:6px 10px;border-bottom:1px solid #2a2a4a;text-align:left}}
th{{color:#9ca3af;font-weight:400}} code{{color:#a5b4fc;font-family:monospace}}
.pit{{padding:8px;background:#1a0a0a;border-left:3px solid #ef4444;border-radius:4px;margin:6px 0;font-size:12px}}
.pit b{{color:#f87171}}
.tag{{display:inline-block;padding:2px 8px;border-radius:4px;font-size:11px;margin:2px}}
.tag-blue{{background:#1e3a5f;color:#7dd3fc}} .tag-green{{background:#1a3a1a;color:#86efac}}
</style></head><body>
<h1>Virtuoso SKILL RSI Report</h1>
<p style="color:#9ca3af">Recursive Self-Improvement Knowledge Base — Official .fnd docs + live verification</p>

<div class="card">
<h2>API Knowledge Base (by Version)</h2>
{version_cards}
<p style="font-size:12px;color:#6b7280;margin-top:8px">
  IC251 = IC231 baseline (to be refined by RSI).
  Common: {common:,} | IC231-only: {only_231:,} | IC618-only: {only_618:,}
</p>
</div>

<div class="card">
<h2>Categories (IC251 target)</h2>
{cat_bars}
</div>

<div class="card">
<h2>Verified Functions (live-tested)</h2>
<table><tr><th>Function</th><th>Signature</th></tr>
{verified_rows}
</table>
<p style="font-size:12px;color:#6b7280">Total manually verified: {manual_verified} / {manual_total} candidates.
  {fnd_total:,} official signatures available for instant lookup.</p>
</div>

<div class="card">
<h2>Pitfalls — Never Repeat</h2>
{pitfall_html}
</div>

<div class="card">
<h2>RSI Efficiency</h2>
<table>
<tr><th>Metric</th><th>Value</th></tr>
<tr><td>Official docs lookup</td><td><code>SELECT syntax FROM fnd_functions WHERE name='...' AND version='IC251'</code></td></tr>
<tr><td>Query time</td><td>~0.02s</td></tr>
<tr><td>First enumeration (trial-and-error)</td><td>~300s for 439 candidates, only 21 real</td></tr>
<tr><td>Speedup (docs vs enumeration)</td><td>~15,000x</td></tr>
<tr><td>Verification</td><td><code>python3 scripts/verify_db.py</code></td></tr>
</table></div>

</body></html>"""

OUT.parent.mkdir(parents=True, exist_ok=True)
OUT.write_text(html, encoding="utf-8")
print(f"Report: {OUT} ({OUT.stat().st_size} bytes)")
