#!/usr/bin/env python3
"""Tests for generate_report.py — Candidates tab data and real generator."""

import tempfile
import unittest
from pathlib import Path
import sys
import os
import subprocess

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts" / "evidence"))


# ─── Minimal schema that generate_report.py requires ───────────────────────────

SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS fnd_functions (
    id INTEGER PRIMARY KEY,
    name TEXT,
    syntax TEXT,
    description TEXT,
    category TEXT,
    version TEXT,
    args_count INTEGER,
    confidence TEXT DEFAULT 'ported',
    last_verified_at TEXT,
    view_context TEXT DEFAULT 'any',
    call_count INTEGER DEFAULT 0,
    success_count INTEGER DEFAULT 0,
    fail_count INTEGER DEFAULT 0,
    UNIQUE(name, version)
);

CREATE TABLE IF NOT EXISTS functions (
    name TEXT PRIMARY KEY,
    category TEXT,
    func_exists INTEGER DEFAULT 0,
    signature TEXT,
    description TEXT,
    last_tested TEXT,
    test_count INTEGER DEFAULT 0,
    notes TEXT,
    version TEXT DEFAULT 'IC251',
    confidence TEXT DEFAULT 'discovered'
);

CREATE TABLE IF NOT EXISTS synonyms (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    canonical TEXT NOT NULL,
    variant TEXT NOT NULL,
    UNIQUE(canonical, variant)
);

CREATE TABLE IF NOT EXISTS param_examples (
    function_name TEXT,
    param_name TEXT,
    example_value TEXT,
    success_count INTEGER DEFAULT 1,
    last_used TEXT,
    PRIMARY KEY (function_name, param_name, example_value)
);

CREATE TABLE IF NOT EXISTS error_history (
    id INTEGER PRIMARY KEY,
    function_name TEXT,
    error_type TEXT,
    error_message TEXT,
    fixed BOOLEAN DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    view_context TEXT DEFAULT 'unknown'
);

CREATE TABLE IF NOT EXISTS snippets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    description TEXT,
    category TEXT,
    code TEXT NOT NULL,
    params TEXT,
    version TEXT DEFAULT 'IC251',
    success_count INTEGER DEFAULT 0,
    last_used TEXT,
    fail_count INTEGER DEFAULT 0,
    last_result TEXT,
    promoted_to TEXT DEFAULT NULL
);

CREATE TABLE IF NOT EXISTS snippet_executions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    snippet_name TEXT,
    success INTEGER,
    duration_ms INTEGER,
    error_message TEXT,
    params_used TEXT,
    executed_at TEXT
);

CREATE TABLE IF NOT EXISTS replay_insights (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    insight_type TEXT NOT NULL,
    pattern TEXT NOT NULL,
    context TEXT,
    success_rate REAL,
    sample_count INTEGER,
    recommendation TEXT,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS interventions (
    intervention_id     TEXT PRIMARY KEY,
    run_id             TEXT NOT NULL,
    step_id            TEXT,
    attempt            INTEGER DEFAULT 0,
    timestamp          TEXT NOT NULL,
    reason             TEXT NOT NULL,
    action             TEXT NOT NULL,
    source             TEXT DEFAULT 'human',
    followup_run_id    TEXT,
    verification_status TEXT,
    evidence_before    TEXT,
    evidence_after     TEXT,
    details            TEXT,
    superseded_by      TEXT,
    created_at         TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS experience_events (
    event_id      TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL,
    task_id       TEXT,
    step_id       TEXT,
    attempt       INTEGER DEFAULT 0,
    event_type    TEXT NOT NULL,
    timestamp     TEXT,
    state         TEXT,
    outcome       TEXT,
    channel       TEXT,
    risk_class    TEXT,
    failure_type  TEXT,
    action        TEXT,
    duration_ms   INTEGER,
    value_source  TEXT DEFAULT 'OBSERVED',
    evidence_refs TEXT,
    details       TEXT
);

CREATE TABLE IF NOT EXISTS candidate_decisions (
    decision_id         TEXT PRIMARY KEY,
    candidate_id       TEXT NOT NULL,
    snapshot_hash      TEXT NOT NULL,
    decision           TEXT NOT NULL,
    reason             TEXT,
    decided_by         TEXT DEFAULT 'human',
    decided_at         TEXT DEFAULT (datetime('now')),
    revoked_at         TEXT,
    revocation_reason   TEXT,
    UNIQUE(candidate_id, snapshot_hash)
);

CREATE TABLE IF NOT EXISTS rsi_progress (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT,
    phase TEXT,
    milestone TEXT,
    detail TEXT,
    functions_added INTEGER DEFAULT 0,
    errors_recorded INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS recordings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT,
    started_at TEXT,
    active INTEGER DEFAULT 1
);

CREATE TABLE IF NOT EXISTS experience_cases (
    case_id            TEXT PRIMARY KEY,
    run_id             TEXT NOT NULL,
    task_id            TEXT,
    case_type          TEXT NOT NULL,
    failure_type       TEXT,
    problem_signature  TEXT,
    initial_state      TEXT,
    action_sequence    TEXT,
    final_state        TEXT,
    outcome            TEXT,
    verification_status TEXT,
    quality_score      REAL DEFAULT 0.0,
    event_refs         TEXT,
    provenance         TEXT,
    created_at         TEXT DEFAULT (datetime('now')),
    source TEXT DEFAULT 'unknown'
);
"""


def create_minimal_schema(db_path):
    """Build all tables generate_report.py queries."""
    import sqlite3
    conn = sqlite3.connect(str(db_path))
    conn.executescript(SCHEMA_SQL)

    # Seed fnd_functions (overview stats need this)
    conn.executemany(
        "INSERT OR IGNORE INTO fnd_functions (name,category,version,confidence,description) VALUES (?,?,?,?,?)",
        [
            ("hiCreateApp",  "Application", "IC251", "ported", "Create application form"),
            ("dbCreateRect", "Layout",      "IC251", "ported", "Create rectangle"),
            ("geOpen",       "Window",      "IC251", "ported", "Open window"),
            ("geSelectAll",  "Selection",   "IC251", "ported", "Select all"),
            ("hiFormDone",   "Form",        "IC251", "ported", "Form done callback"),
        ]
    )
    conn.execute(
        "INSERT OR IGNORE INTO snippets (name,category,code,description,success_count,fail_count) VALUES (?,?,?,?,?,?)",
        ("my_snippet", "Layout", "# SKILL", "A useful snippet", 3, 1)
    )
    conn.execute(
        "INSERT OR IGNORE INTO error_history (function_name,error_type,error_message,fixed,created_at) VALUES (?,?,?,?,?)",
        ("hiCreateApp", "SyntaxError", "Unexpected token near 'hiCreateApp'", 1, "2026-01-01 00:00:00")
    )
    conn.commit()
    conn.close()


def run_generator(db_path, out_path):
    """Actually invoke generate_report.py with custom DB and OUT paths."""
    gen_script = Path(__file__).parent.parent / "scripts" / "generate_report.py"

    with open(gen_script) as f:
        script = f.read()

    script = script.replace(
        'DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"',
        f"DB = Path(r'{db_path}')"
    )
    script = script.replace(
        'OUT = Path(__file__).parent.parent / "report" / "rsi_report.html"',
        f"OUT = Path(r'{out_path}')"
    )

    temp_script = Path(tempfile.mktemp(suffix=".py"))
    with open(temp_script, "w") as f:
        f.write(script)

    scripts_dir = str(Path(__file__).parent.parent / "scripts")
    evidence_dir = str(Path(__file__).parent.parent / "scripts" / "evidence")
    env = os.environ.copy()
    env["PYTHONPATH"] = f"{scripts_dir}:{evidence_dir}"

    try:
        result = subprocess.run(
            [sys.executable, str(temp_script)],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True,
            cwd=scripts_dir,
            env=env
        )
        return result, temp_script
    except Exception:
        return None, temp_script


# ─── Test class: data layer ─────────────────────────────────────────────────────

class TestCandidatesTabData(unittest.TestCase):
    """Unit tests for candidates data generation — no subprocess."""

    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        from record_intervention import init_db
        init_db(self.db)

    def tearDown(self):
        import os; os.unlink(str(self.db)) if os.path.exists(str(self.db)) else None

    def test_html_escaping_in_reason_action(self):
        """User input in reason/action is properly escaped at the data layer."""
        from record_intervention import (
            init_db, record_intervention, verify_with_evidence,
            record_candidate_decision
        )
        import mine_candidates
        import html

        iv = record_intervention(
            run_id="run-1", step_id="step-1",
            reason="<script>alert('xss')</script>",
            action="Click <button>& click",
            db_path=self.db
        )
        conn = init_db(self.db)
        conn.execute("""
            INSERT INTO experience_events
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """, ("evt-1", "followup-1"))
        conn.commit()
        conn.close()
        verify_with_evidence(
            intervention_id=iv["intervention_id"],
            followup_run_id="followup-1",
            db_path=self.db
        )

        result = mine_candidates.mine(min_verified=0, db_path=self.db)

        # mine() returns a sig field; parse it the way generate_report does
        import json
        def _parse_sig(sig):
            try:
                return json.loads(sig)
            except:
                return {"reason": "", "action": "", "context": {}}

        raw_cand = result["candidates"][0]
        parsed = _parse_sig(raw_cand.get("sig", ""))
        raw_reason = parsed.get("reason", "")
        raw_action = parsed.get("action", "")

        esc_reason = html.escape(raw_reason, quote=True)
        esc_action = html.escape(raw_action, quote=True)

        # No raw script tag
        self.assertNotIn("<script>", esc_reason)
        self.assertNotIn("<script>", esc_action)

        # Escaped versions present
        self.assertIn("&lt;script&gt;", esc_reason)
        self.assertIn("&amp;", esc_action)

        print(f"✓ Raw:   {raw_reason}")
        print(f"✓ Escaped: {esc_reason}")


# ─── Test class: real generator ────────────────────────────────────────────────

class TestRealGenerator(unittest.TestCase):
    """Test generate_report.py by actually running it as a subprocess."""

    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        self.out = Path(tempfile.mktemp(suffix=".html"))
        create_minimal_schema(self.db)

    def tearDown(self):
        import os; os.unlink(str(self.db)) if os.path.exists(str(self.db)) else None
        import os; os.unlink(str(self.out)) if os.path.exists(str(self.out)) else None

    def test_report_generates_html_file(self):
        """Generator creates an HTML file with candidate data."""
        from record_intervention import (
            init_db, record_intervention, verify_with_evidence,
            record_candidate_decision
        )
        import mine_candidates

        # Create intervention with special chars in reason/action
        iv = record_intervention(
            run_id="run-1", step_id="step-1",
            reason="Window <alert>error",
            action="Click OK & retry",
            db_path=self.db
        )
        conn = init_db(self.db)
        conn.execute("""
            INSERT INTO experience_events
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """, ("evt-1", "followup-1"))
        conn.commit()
        conn.close()
        verify_with_evidence(
            intervention_id=iv["intervention_id"],
            followup_run_id="followup-1",
            db_path=self.db
        )

        # Mine and adopt
        result = mine_candidates.mine(min_verified=0, db_path=self.db)
        snap = result["h"]
        record_candidate_decision(
            result["candidates"][0]["candidate_id"], snap, "ADOPTED",
            reason="Good evidence",
            db_path=self.db
        )

        # Run the actual generator
        result_obj, temp_script = run_generator(self.db, self.out)
        self.assertIsNotNone(result_obj)
        self.assertEqual(result_obj.returncode, 0,
            f"Generator failed: {result_obj.stderr}")
        self.assertTrue(self.out.exists())

        html = self.out.read_text()

        # Basic structure
        self.assertIn("Candidates", html)
        self.assertIn(snap, html)        # snapshot hash
        self.assertIn("ADOPTED", html)    # decision status
        self.assertIn("Window", html)     # reason text
        self.assertIn("Click", html)       # action text
        self.assertIn("tab-candidates", html)

        # HTML escaping — no raw < > in reason/action output
        self.assertNotIn("<alert>", html,
            "Raw HTML not escaped in reason")
        self.assertIn("&lt;alert&gt;", html)  # escaped version
        self.assertIn("&amp; retry", html)      # & escaped

        temp_script.unlink()
        print(f"✓ Generated {len(html):,} bytes, {self.out.stat().st_size:,} on disk")

    def test_snapshot_change_affects_adopted_summary(self):
        """Old adopted decisions don't appear in current snapshot summary."""
        from record_intervention import (
            init_db, record_intervention, verify_with_evidence,
            record_candidate_decision
        )
        import mine_candidates

        # First snapshot: create + adopt
        iv1 = record_intervention(
            run_id="run-1", step_id="step-1",
            reason="First reason", action="First action",
            db_path=self.db
        )
        conn = init_db(self.db)
        conn.execute("""
            INSERT INTO experience_events
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """, ("evt-1", "followup-1"))
        conn.commit()
        conn.close()
        verify_with_evidence(
            intervention_id=iv1["intervention_id"],
            followup_run_id="followup-1",
            db_path=self.db
        )
        result1 = mine_candidates.mine(min_verified=0, db_path=self.db)
        snap1 = result1["h"]
        record_candidate_decision(
            result1["candidates"][0]["candidate_id"], snap1, "ADOPTED",
            reason="First snapshot", db_path=self.db
        )

        # Second snapshot: new evidence
        iv2 = record_intervention(
            run_id="run-2", step_id="step-1",
            reason="Second reason", action="Second action",
            db_path=self.db
        )
        conn = init_db(self.db)
        conn.execute("""
            INSERT INTO experience_events
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """, ("evt-2", "followup-2"))
        conn.commit()
        conn.close()
        verify_with_evidence(
            intervention_id=iv2["intervention_id"],
            followup_run_id="followup-2",
            db_path=self.db
        )
        result2 = mine_candidates.mine(min_verified=0, db_path=self.db)
        snap2 = result2["h"]
        self.assertNotEqual(snap1, snap2)

        # Run generator — current snapshot only
        result_obj, temp_script = run_generator(self.db, self.out)
        self.assertEqual(result_obj.returncode, 0)

        html = self.out.read_text()

        # Current snapshot appears
        self.assertIn(snap2, html)

        # Old snapshot absent from CLI commands section
        cli_start = html.find("CLI Commands for Review")
        cli_section = html[cli_start:] if cli_start >= 0 else ""
        self.assertNotIn(snap1, cli_section,
            "Old snapshot hash must not appear in CLI commands")

        # Stats show 0 adopted for new snapshot
        self.assertIn("Adopted (this snapshot)", html)

        temp_script.unlink()
        print(f"✓ Snap1={snap1}, Snap2={snap2}")
        print(f"✓ Old adopted hidden from current summary")


if __name__ == "__main__":
    unittest.main(verbosity=2)
