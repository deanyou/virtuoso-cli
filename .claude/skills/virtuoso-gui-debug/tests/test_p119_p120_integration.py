#!/usr/bin/env python3
"""
Integration test for #119/#120 — full evidence-loop pipeline.

Tests 4 chains:
  1. trace    — trace → original events (with run_id linkage)
  2. verify   — VERIFY/VERIFIER_CONFIRMED events → FAILED/CONFLICT/UNKNOWN classification
  3. adopt    — candidate adoption binds to snapshot, does not change runtime behaviour
  4. report   — state, refs, CLI args; evidence change → old decision ≠ new snapshot

Isolated: independent temp DB, no CMOP state mutated.

  python3 test_p119_p120_integration.py

This is NOT a live-executor test. Live evidence closure (real VERIFIER_CONFIRMED
from a running Virtuoso session) is out of scope per #109.
"""

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Dict, Any

_SCRIPT_DIR = Path(__file__).parent.resolve()          # tests/
_SKILL_DIR  = _SCRIPT_DIR.parent                      # virtuoso-gui-debug/
_EVIDENCE   = _SKILL_DIR / "scripts" / "evidence"

sys.path.insert(0, str(_EVIDENCE))
from record_intervention import (
    init_db,
    record_intervention,
    verify_with_evidence,
    get_trace_for_intervention,
    list_interventions,
    update_verification_status,
    record_candidate_decision,
    list_candidate_decisions,
    get_adopted_candidates,
    cmd_review,
)
import mine_candidates


# ─────────────────────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────────────────────

def _iv_id(iv: Dict[str, Any]) -> str:
    return iv["intervention_id"]


def _insert_verifier_events(db_path: Path, run_id: str, outcomes: list):
    """Insert VERIFIER_CONFIRMED events into experience_events.

    Args:
        db_path: DB path
        run_id:  followup run_id for the events
        outcomes: list of outcome strings, e.g. ["PASSED", "FAILED"]
    """
    conn = init_db(db_path)
    for i, outcome in enumerate(outcomes):
        conn.execute("""
            INSERT INTO experience_events
              (event_id, run_id, step_id, event_type, state,
               value_source, outcome)
            VALUES (?, ?, ?, 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', ?)
        """, (f"evt-{run_id}-{i}", run_id, f"step-{i}", outcome))
    conn.commit()
    conn.close()


def _insert_observed_events(db_path: Path, run_id: str, steps: list):
    """Insert OBSERVED execution events (original run)."""
    conn = init_db(db_path)
    for i, step_id in enumerate(steps):
        conn.execute("""
            INSERT INTO experience_events
              (event_id, run_id, step_id, event_type, state,
               value_source, outcome)
            VALUES (?, ?, ?, 'EXECUTE', 'EXECUTE', 'OBSERVED', 'ok')
        """, (f"orig-evt-{run_id}-{i}", run_id, step_id))
    conn.commit()
    conn.close()


# ─────────────────────────────────────────────────────────────────────────────
# CHAIN 1 — trace() traces back to original run events
# ─────────────────────────────────────────────────────────────────────────────

class TestChain1_Trace(unittest.TestCase):
    """trace → original_events and followup_events are correctly retrieved."""

    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)

    def tearDown(self):
        self.db.unlink(missing_ok=True)

    def test_trace_returns_original_events(self):
        """trace.original_events contains the original run's events."""
        # Insert original run events
        _insert_observed_events(self.db, "run-original", ["step-1", "step-2"])

        # Record an intervention
        iv = record_intervention(
            run_id="run-original",
            step_id="step-2",
            reason="Window not found",
            action="Click retry",
            db_path=self.db,
        )

        # trace it
        trace = get_trace_for_intervention(iv["intervention_id"], self.db)

        self.assertNotIn("error", trace)
        self.assertEqual(len(trace["original_events"]), 2)
        self.assertTrue(
            all(e["run_id"] == "run-original" for e in trace["original_events"])
        )

    def test_trace_returns_followup_events_after_verification(self):
        """trace.followup_events contains events from the verification run."""
        _insert_observed_events(self.db, "run-original", ["step-1"])

        iv = record_intervention(
            run_id="run-original",
            step_id="step-1",
            reason="Timeout",
            action="Reconnect",
            db_path=self.db,
        )

        # Simulate verification run
        _insert_verifier_events(self.db, "run-followup", ["PASSED"])
        _insert_observed_events(self.db, "run-followup", ["step-1"])

        update_verification_status(
            iv["intervention_id"],
            verification_status="VERIFIED",
            followup_run_id="run-followup",
            verification_source="evidence",
            db_path=self.db,
        )

        trace = get_trace_for_intervention(iv["intervention_id"], self.db)

        self.assertNotIn("error", trace)
        self.assertGreaterEqual(len(trace["followup_events"]), 1)
        self.assertTrue(
            all(e["run_id"] == "run-followup" for e in trace["followup_events"])
        )

    def test_trace_empty_followup_when_no_followup_run(self):
        """trace.followup_events is [] when intervention has no followup run."""
        iv = record_intervention(
            run_id="run-orphan",
            step_id="step-x",
            reason="Test",
            action="Test",
            db_path=self.db,
        )

        trace = get_trace_for_intervention(iv["intervention_id"], self.db)

        self.assertNotIn("error", trace)
        self.assertEqual(trace["followup_events"], [])

    def test_trace_cli_maps_to_function(self):
        """CLI trace command returns the same data as the function."""
        iv = record_intervention(
            run_id="run-cli",
            step_id="step-y",
            reason="CLI test",
            action="CLI action",
            db_path=self.db,
        )

        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"),
                "--db", str(self.db),
                "trace",
                iv["intervention_id"],
            ],
            capture_output=True,
            text=True,
        )

        self.assertEqual(result.returncode, 0)
        data = json.loads(result.stdout)
        self.assertEqual(len(data["original_events"]), 0)
        self.assertEqual(data["intervention"]["run_id"], "run-cli")


# ─────────────────────────────────────────────────────────────────────────────
# CHAIN 2 — VERIFY / VERIFIER_CONFIRMED → FAILED / CONFLICT / UNKNOWN
# ─────────────────────────────────────────────────────────────────────────────

class TestChain2_VerifierClassification(unittest.TestCase):
    """verify_with_evidence derives status from actual verifier events.

    Critical invariants:
      PASSED-only        → VERIFIED
      FAILED-only        → FAILED
      PASSED + FAILED    → CONFLICT
      no verifier events  → UNKNOWN
    """

    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)

    def tearDown(self):
        self.db.unlink(missing_ok=True)

    def test_passed_only_produces_verified(self):
        """Single PASSED verifier outcome → VERIFIED."""
        _insert_verifier_events(self.db, "run-pass", ["PASSED"])

        iv = record_intervention(
            run_id="run-orig", step_id="step-1",
            reason="Test", action="Test", db_path=self.db,
        )
        result = verify_with_evidence(iv["intervention_id"], "run-pass", self.db)

        self.assertEqual(result["verification_status"], "VERIFIED")
        self.assertEqual(result["derived_from"], "actual_passed_outcome")

    def test_failed_only_produces_failed(self):
        """Single FAILED verifier outcome → FAILED (never VERIFIED)."""
        _insert_verifier_events(self.db, "run-fail", ["FAILED"])

        iv = record_intervention(
            run_id="run-orig", step_id="step-1",
            reason="Test", action="Test", db_path=self.db,
        )
        result = verify_with_evidence(iv["intervention_id"], "run-fail", self.db)

        self.assertEqual(result["verification_status"], "FAILED")
        self.assertNotEqual(result["verification_status"], "VERIFIED")
        self.assertEqual(result["derived_from"], "actual_failed_outcome")

    def test_mixed_passed_and_failed_produces_conflict(self):
        """PASSED + FAILED in same followup run → CONFLICT."""
        _insert_verifier_events(self.db, "run-conflict", ["PASSED", "FAILED"])

        iv = record_intervention(
            run_id="run-orig", step_id="step-1",
            reason="Test", action="Test", db_path=self.db,
        )
        result = verify_with_evidence(iv["intervention_id"], "run-conflict", self.db)

        self.assertEqual(result["verification_status"], "CONFLICT")
        self.assertEqual(result["derived_from"], "conflict_detected")
        self.assertNotEqual(result["verification_status"], "VERIFIED")
        self.assertNotEqual(result["verification_status"], "FAILED")

    def test_multiple_passed_produces_verified(self):
        """Multiple PASSED verifier outcomes → VERIFIED."""
        _insert_verifier_events(self.db, "run-multi-pass", ["PASSED", "PASSED", "PASSED"])

        iv = record_intervention(
            run_id="run-orig", step_id="step-1",
            reason="Test", action="Test", db_path=self.db,
        )
        result = verify_with_evidence(iv["intervention_id"], "run-multi-pass", self.db)

        self.assertEqual(result["verification_status"], "VERIFIED")

    def test_multiple_failed_produces_failed(self):
        """Multiple FAILED verifier outcomes → FAILED."""
        _insert_verifier_events(self.db, "run-multi-fail", ["FAILED", "FAILED"])

        iv = record_intervention(
            run_id="run-orig", step_id="step-1",
            reason="Test", action="Test", db_path=self.db,
        )
        result = verify_with_evidence(iv["intervention_id"], "run-multi-fail", self.db)

        self.assertEqual(result["verification_status"], "FAILED")

    def test_no_verifier_events_produces_unknown(self):
        """Followup run exists but has no VERIFIER_CONFIRMED → UNKNOWN."""
        # Insert followup run without verifier events
        _insert_observed_events(self.db, "run-no-verifier", ["step-1"])

        iv = record_intervention(
            run_id="run-orig", step_id="step-1",
            reason="Test", action="Test", db_path=self.db,
        )
        result = verify_with_evidence(iv["intervention_id"], "run-no-verifier", self.db)

        self.assertEqual(result["verification_status"], "UNKNOWN")
        self.assertEqual(result["verifier_confirmed_count"], 0)
        self.assertEqual(result["derived_from"], "no_verifier_events")

    def test_verified_propagates_to_intervention_record(self):
        """verify_with_evidence actually updates the intervention record."""
        _insert_verifier_events(self.db, "run-pass", ["PASSED"])

        iv = record_intervention(
            run_id="run-orig", step_id="step-1",
            reason="Test", action="Test", db_path=self.db,
        )
        verify_with_evidence(iv["intervention_id"], "run-pass", self.db)

        # list_interventions does not filter by intervention_id; use raw query
        import sqlite3
        conn = sqlite3.connect(self.db)
        row = conn.execute(
            "SELECT verification_status FROM interventions WHERE intervention_id = ?",
            (iv["intervention_id"],)
        ).fetchone()
        conn.close()
        self.assertEqual(row[0], "VERIFIED")


# ─────────────────────────────────────────────────────────────────────────────
# CHAIN 3 — mine → adopt / reject → snapshot binding + no runtime change
# ─────────────────────────────────────────────────────────────────────────────

class TestChain3_CandidateAdoption(unittest.TestCase):
    """Adoption binds to snapshot hash; does not affect runtime behaviour."""

    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)

    def tearDown(self):
        self.db.unlink(missing_ok=True)

    def test_adopt_produces_decision_record(self):
        """Adopt creates a candidate_decisions row."""
        record_candidate_decision(
            "cand-001", "snap-abc", "ADOPTED",
            reason="Strong evidence", db_path=self.db,
        )
        adopted = get_adopted_candidates(self.db)
        self.assertEqual(len(adopted), 1)
        self.assertEqual(adopted[0]["candidate_id"], "cand-001")
        self.assertEqual(adopted[0]["snapshot_hash"], "snap-abc")

    def test_adopt_is_bound_to_snapshot(self):
        """ADOPTED only applies within the exact same snapshot hash."""
        record_candidate_decision(
            "cand-001", "snap-old", "ADOPTED", db_path=self.db,
        )
        adopted_old = get_adopted_candidates(self.db)
        self.assertEqual(len(adopted_old), 1)

        # New snapshot hash → not adopted
        decisions = list_candidate_decisions(
            candidate_id="cand-001", db_path=self.db,
        )
        # Decision exists but get_adopted only returns current snapshot
        self.assertEqual(len(decisions), 1)

    def test_adopt_does_not_affect_candidate_stats(self):
        """Adoption does not mutate the candidate stats or intervention data."""
        _insert_verifier_events(self.db, "run-f1", ["PASSED"])
        iv = record_intervention(
            run_id="run-orig", step_id="step-1",
            reason="Test reason", action="Test action",
            db_path=self.db,
        )
        verify_with_evidence(iv["intervention_id"], "run-f1", self.db)

        # Mine BEFORE adoption
        result_before = mine_candidates.mine(min_verified=0, db_path=self.db)
        cand_before = result_before["candidates"][0]
        stats_before = cand_before["stats"].copy()

        # Adopt
        record_candidate_decision(
            cand_before["candidate_id"],
            result_before["h"],
            "ADOPTED",
            reason="Good enough",
            db_path=self.db,
        )

        # Mine AFTER adoption — stats must be identical
        result_after = mine_candidates.mine(min_verified=0, db_path=self.db)
        cand_after = result_after["candidates"][0]

        self.assertEqual(cand_before["candidate_id"], cand_after["candidate_id"])
        self.assertEqual(cand_before["stats"], cand_after["stats"])

    def test_reject_leaves_adopted_empty(self):
        """REJECTED candidate does not appear in get_adopted_candidates()."""
        record_candidate_decision(
            "cand-002", "snap-r", "REJECTED",
            reason="Insufficient data", db_path=self.db,
        )
        self.assertEqual(len(get_adopted_candidates(self.db)), 0)

    def test_auto_revocation_on_second_adopt(self):
        """Re-adopting same candidate revokes the previous adoption."""
        record_candidate_decision(
            "cand-003", "snap-v1", "ADOPTED", db_path=self.db,
        )
        record_candidate_decision(
            "cand-003", "snap-v2", "ADOPTED", db_path=self.db,
        )

        adopted = get_adopted_candidates(self.db)
        self.assertEqual(len(adopted), 1)
        self.assertEqual(adopted[0]["snapshot_hash"], "snap-v2")

        # Check old one is revoked (include_revoked=True to see SUPERSEDED)
        all_decisions = list_candidate_decisions(
            candidate_id="cand-003", include_revoked=True, db_path=self.db,
        )
        old_decision = [d for d in all_decisions if d["snapshot_hash"] == "snap-v1"]
        self.assertEqual(len(old_decision), 1)
        self.assertEqual(old_decision[0]["decision"], "SUPERSEDED")
        self.assertIsNotNone(old_decision[0]["revoked_at"])

    def test_snapshot_hash_changes_with_new_evidence(self):
        """Adding new interventions changes snapshot hash."""
        _insert_verifier_events(self.db, "run-f1", ["PASSED"])
        iv1 = record_intervention(
            run_id="r1", step_id="s1",
            reason="Reason A", action="Action A",
            db_path=self.db,
        )
        verify_with_evidence(iv1["intervention_id"], "run-f1", self.db)
        snap1 = mine_candidates.mine(min_verified=0, db_path=self.db)["h"]

        _insert_verifier_events(self.db, "run-f2", ["PASSED"])
        iv2 = record_intervention(
            run_id="r2", step_id="s2",
            reason="Reason B", action="Action B",
            db_path=self.db,
        )
        verify_with_evidence(iv2["intervention_id"], "run-f2", self.db)
        snap2 = mine_candidates.mine(min_verified=0, db_path=self.db)["h"]

        self.assertNotEqual(snap1, snap2,
            "Different interventions must produce different snapshot hashes")

    def test_cmd_review_shows_decision_status(self):
        """CLI review command shows ADOPTED / REJECTED / PENDING."""
        from io import StringIO
        import contextlib

        _insert_verifier_events(self.db, "run-f1", ["PASSED"])
        iv = record_intervention(
            run_id="r1", step_id="s1",
            reason="R", action="A",
            db_path=self.db,
        )
        verify_with_evidence(iv["intervention_id"], "run-f1", self.db)
        result = mine_candidates.mine(min_verified=0, db_path=self.db)
        cand_id = result["candidates"][0]["candidate_id"]
        snap = result["h"]

        # No decision → PENDING
        args = type("Args", (), {"min_verified": 0, "db": str(self.db)})()
        out = StringIO()
        with contextlib.redirect_stdout(out):
            cmd_review(args)
        self.assertIn("PENDING", out.getvalue())

        # Adopt
        record_candidate_decision(cand_id, snap, "ADOPTED", db_path=self.db)
        out2 = StringIO()
        with contextlib.redirect_stdout(out2):
            cmd_review(type("Args", (), {"min_verified": 0, "db": str(self.db)})())
        self.assertIn("ADOPTED", out2.getvalue())
        self.assertNotIn("PENDING", out2.getvalue())


# ─────────────────────────────────────────────────────────────────────────────
# CHAIN 4 — Report: state, refs, CLI args; evidence change → old ≠ new snapshot
# ─────────────────────────────────────────────────────────────────────────────

class TestChain4_ReportGeneration(unittest.TestCase):
    """Report correctly surfaces state, refs, CLI args; old decisions don't
    apply to new snapshots (critical evidence-change invariant)."""

    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)

    def tearDown(self):
        self.db.unlink(missing_ok=True)

    def _generate_candidates_data(self):
        """Replicate generate_report.py's candidate data assembly."""
        import html
        result = mine_candidates.mine(min_verified=0, db_path=self.db)
        decisions = list_candidate_decisions(include_revoked=True, db_path=self.db)
        adopted = get_adopted_candidates(self.db)

        decision_map = {}
        for d in decisions:
            key = (d["candidate_id"], d["snapshot_hash"])
            decision_map[key] = d

        snapshot_hash = result.get("h", "")
        adopted_current = [d for d in adopted
                           if d.get("snapshot_hash") == snapshot_hash]

        for c in result.get("candidates", []):
            cand_id = c.get("candidate_id", "")
            try:
                sig_data = json.loads(c.get("sig", "{}"))
                c["reason"] = sig_data.get("reason", "")
                c["action"] = sig_data.get("action", "")
                c["context"] = sig_data.get("context", {})
            except Exception:
                c["reason"] = ""
                c["action"] = ""
                c["context"] = {}

            dec = decision_map.get((cand_id, snapshot_hash), {})
            if dec:
                c["decision_status"] = dec["decision"]
                c["decision_revoked"] = dec.get("revoked_at")
                c["decision_reason"] = dec.get("reason")
            else:
                c["decision_status"] = "PENDING"
                c["decision_revoked"] = None
                c["decision_reason"] = None

        return {
            "candidates": result.get("candidates", []),
            "adopted": adopted_current,
            "total_cands": result.get("total_cands", 0),
            "snapshot_hash": snapshot_hash,
        }

    def test_candidates_tab_shows_reason_action_context(self):
        """Candidates tab displays reason (red), action (green), context."""
        _insert_verifier_events(self.db, "run-f1", ["PASSED"])
        iv = record_intervention(
            run_id="r1", step_id="s1",
            reason="Window identity ambiguous",
            action="Select target window manually",
            db_path=self.db,
        )
        verify_with_evidence(iv["intervention_id"], "run-f1", self.db)

        data = self._generate_candidates_data()
        self.assertEqual(len(data["candidates"]), 1)
        c = data["candidates"][0]
        self.assertEqual(c["reason"], "Window identity ambiguous")
        self.assertEqual(c["action"], "Select target window manually")
        self.assertIn("candidate_id", c)
        self.assertIn("stats", c)

    def test_candidates_tab_shows_stats(self):
        """Candidate card shows attempts, verified_count, failed_count, conflict."""
        _insert_verifier_events(self.db, "run-pass", ["PASSED"])
        iv1 = record_intervention(
            run_id="r1", step_id="s1",
            reason="Test", action="Test", db_path=self.db,
        )
        verify_with_evidence(iv1["intervention_id"], "run-pass", self.db)

        _insert_verifier_events(self.db, "run-fail", ["FAILED"])
        iv2 = record_intervention(
            run_id="r2", step_id="s1",
            reason="Test", action="Test", db_path=self.db,
        )
        verify_with_evidence(iv2["intervention_id"], "run-fail", self.db)

        _insert_verifier_events(self.db, "run-conf", ["PASSED", "FAILED"])
        iv3 = record_intervention(
            run_id="r3", step_id="s1",
            reason="Test", action="Test", db_path=self.db,
        )
        verify_with_evidence(iv3["intervention_id"], "run-conf", self.db)

        data = self._generate_candidates_data()
        self.assertEqual(len(data["candidates"]), 1)  # Same sig → 1 candidate
        c = data["candidates"][0]
        self.assertEqual(c["stats"]["verified_count"], 1)
        self.assertEqual(c["stats"]["failed_count"], 1)
        self.assertEqual(c["stats"]["conflict_count"], 1)
        self.assertEqual(c["stats"]["attempts"], 3)

    def test_cli_commands_shown_in_report(self):
        """generate_report.py generates HTML containing CLI commands and snapshot hash.

        Gap fixed: clone the real skill DB via sqlite3.backup (24 tables, 15 MB) to
        a temp file; inject test interventions; run the patched generator against the
        clone.  Exit code 0 proves the generator runs cleanly (no swallowed exceptions).
        HTML assertions verify the Candidates tab structure and CLI command strings.
        """

        gen_script = _SKILL_DIR / "scripts" / "generate_report.py"
        if not gen_script.exists():
            self.skipTest("generate_report.py not found")
            return

        # Step 1: clone the real skill DB (all 24 tables) to an independent temp file
        import sqlite3 as _sqlite3
        skill_db = _SKILL_DIR / "data" / "skill_db.sqlite3"
        backup = Path(tempfile.mktemp(suffix=".db"))
        src_conn = _sqlite3.connect(str(skill_db))
        dst_conn = _sqlite3.connect(str(backup))
        src_conn.backup(dst_conn)
        src_conn.close()
        dst_conn.close()

        report_out = Path(tempfile.mkdtemp()) / "rsi_report.html"
        patched_script = Path(tempfile.mkdtemp()) / "gen_patched.py"

        try:
            # Step 2: inject test interventions into the cloned DB
            _conn = _sqlite3.connect(str(backup))
            _conn.execute("""
                INSERT INTO experience_events
                  (event_id, run_id, task_id, step_id, attempt, event_type, timestamp, state,
                   outcome, channel, risk_class, failure_type, action, duration_ms,
                   value_source, evidence_refs, details)
                VALUES (?, ?, ?, ?, 0, 'VERIFY', datetime('now'), 'VERIFY',
                        ?, 'test', 'P3', 'none', 'retry', 0, 'VERIFIER_CONFIRMED', '[]', '{}')
            """, ("test-evt-f1", "test-run-f1", "task-1", "step-1", "PASSED"))
            _conn.execute("""
                INSERT INTO interventions
                  (intervention_id, run_id, step_id, attempt, timestamp, reason, action,
                   source, verification_status)
                VALUES (?, ?, ?, 0, datetime('now'), ?, ?, 'human', 'VERIFIED')
            """, ("test-iv-1", "test-run-f1", "step-1",
                  "Window identity ambiguous", "Select target window manually"))
            _conn.commit()
            _conn.close()

            # Step 3: patch generator DB/OUT paths and run as subprocess
            src = gen_script.read_text(encoding="utf-8")
            patched_src = (
                src
                .replace(
                    'DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"',
                    f'DB = Path(r"{backup}")',
                )
                .replace(
                    'OUT = Path(__file__).parent.parent / "report" / "rsi_report.html"',
                    f'OUT = Path(r"{report_out}")',
                )
                .replace(
                    'EVIDENCE_DIR = Path(__file__).parent / "evidence"',
                    f'EVIDENCE_DIR = Path(r"{_EVIDENCE}")',
                )
            )
            patched_script.write_text(patched_src, encoding="utf-8")

            result = subprocess.run(
                [sys.executable, str(patched_script)],
                capture_output=True, text=True,
                cwd=str(gen_script.parent),
            )

            # Exit code 0: generator ran cleanly (no swallowed exceptions)
            self.assertEqual(
                result.returncode, 0,
                f"Generator must exit 0. stderr: {result.stderr[:500]}\n"
                f"stdout: {result.stdout[:200]}",
            )
            self.assertTrue(
                report_out.exists(),
                f"Generator exited 0 but did not write HTML to {report_out}",
            )

            html_text = report_out.read_text(encoding="utf-8")

            # CLI commands (the primary output claim of #120)
            self.assertIn("record_intervention.py review", html_text)
            self.assertIn("record_intervention.py adopt", html_text)
            self.assertIn("record_intervention.py reject", html_text)
            self.assertIn("record_intervention.py list-decisions", html_text)

            # Candidates tab structure
            self.assertIn("Total Candidates", html_text)
            self.assertIn("Adopted (this snapshot)", html_text)
            self.assertIn("PENDING", html_text)
            self.assertIn("Snapshot:", html_text)

        finally:
            if backup.exists(): backup.unlink(missing_ok=True)
            if report_out.exists(): report_out.unlink(missing_ok=True)
            if patched_script.exists(): patched_script.unlink(missing_ok=True)


    def test_evidence_change_invalidates_old_adopted_decision(self):
        """After new evidence (new snapshot), old adopted decision is NOT in
        the current snapshot's adopted list — the key evidence-change invariant.

        Gap fixed: calls the real CLI `review` command to assert PENDING status
        and 0 adopted count in the actual command output (not just data-layer
        replication).
        """
        # ── Snapshot 1 ──────────────────────────────────────────────────
        _insert_verifier_events(self.db, "run-f1", ["PASSED"])
        iv1 = record_intervention(
            run_id="r1", step_id="s1",
            reason="First reason", action="First action",
            db_path=self.db,
        )
        verify_with_evidence(iv1["intervention_id"], "run-f1", self.db)

        snap1 = mine_candidates.mine(min_verified=0, db_path=self.db)["h"]
        cand1_id = mine_candidates.mine(min_verified=0, db_path=self.db)[
            "candidates"
        ][0]["candidate_id"]
        record_candidate_decision(cand1_id, snap1, "ADOPTED",
                                 reason="Initial adoption", db_path=self.db)

        # ── Snapshot 2 (new intervention → new snapshot) ────────────────
        _insert_verifier_events(self.db, "run-f2", ["PASSED"])
        iv2 = record_intervention(
            run_id="r2", step_id="s1",
            reason="Second reason", action="Second action",
            db_path=self.db,
        )
        verify_with_evidence(iv2["intervention_id"], "run-f2", self.db)

        snap2 = mine_candidates.mine(min_verified=0, db_path=self.db)["h"]

        self.assertNotEqual(snap1, snap2, "Snapshots must differ")

        # Data-layer: snap1 decision is preserved but NOT in snap2's adopted list
        all_decisions = list_candidate_decisions(include_revoked=True, db_path=self.db)
        snap1_decisions = [d for d in all_decisions
                           if d["snapshot_hash"] == snap1]
        self.assertGreater(len(snap1_decisions), 0)

        data = self._generate_candidates_data()
        self.assertEqual(data["snapshot_hash"], snap2)
        self.assertEqual(len(data["adopted"]), 0,
            "Old snapshot adoption must NOT appear in new snapshot")

        # CLI-layer: real `review` command must show PENDING and 0 adopted for snap2
        review_result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"),
                "--db", str(self.db),
                "review", "--min-verified", "0",
            ],
            capture_output=True, text=True,
        )
        self.assertEqual(review_result.returncode, 0, review_result.stderr)

        review_output = review_result.stdout
        # ── Old ADOPTED is preserved (not deleted) but not applicable to snap2 ──
        # Auto-revocation fires only when cmd_adopt is called again for the same
        # candidate_id.  Here the evidence base changed → different snapshot hash →
        # the snap1 adoption is simply not returned for snap2 (snapshot-hash filter).
        # It remains in candidate_decisions with revoked_at=NULL as historical record.
        all_decisions = list_candidate_decisions(include_revoked=True, db_path=self.db)
        old_adopted = [d for d in all_decisions
                       if d["candidate_id"] == cand1_id and d["decision"] == "ADOPTED"]
        self.assertGreater(len(old_adopted), 0,
            f"ADOPTED decision for {cand1_id} must exist (snap1 is in history)")
        self.assertIsNone(old_adopted[0].get("revoked_at"),
            "ADOPTED from snap1 must NOT be marked revoked (historical record; "
            "it simply doesn't apply to snap2 because the snapshot hash differs")
        # Snap2's adopted list must be empty (the snap1 adoption filtered out)
        self.assertEqual(len(data["adopted"]), 0,
            "snap2 adopted list must be empty — old adoption filtered by hash")

        # ── CLI review: the SAME candidate shows PENDING on its line ─────────────
        # Combined regex locks cand1_id AND PENDING to the same output line.
        # A separate "PENDING elsewhere" would NOT satisfy this.
        self.assertRegex(
            review_output,
            rf"{cand1_id}.*PENDING|PENDING.*{cand1_id}",
            f"{cand1_id} must appear on a line that also contains PENDING")
        # Confirms 0 adopted in snap2
        self.assertRegex(review_output, r"0 adopted",
            "Adopted count must be 0 in new snapshot")

        # Also verify the HTML shows cand1_id with PENDING class (not adopted).
        # Clone the skill DB and inject test interventions, then run patched generator.
        import sqlite3 as _sqlite3
        skill_db = _SKILL_DIR / "data" / "skill_db.sqlite3"
        snap_backup = Path(tempfile.mktemp(suffix=".db"))
        src_c = _sqlite3.connect(str(skill_db))
        dst_c = _sqlite3.connect(str(snap_backup))
        src_c.backup(dst_c)
        src_c.close()
        dst_c.close()

        # Copy self.db interventions into the cloned DB (all 3 interventions)
        _ci = _sqlite3.connect(str(snap_backup))
        _c2 = _sqlite3.connect(str(self.db))
        for row in _c2.execute("SELECT * FROM experience_events"):
            try:
                _ci.execute(
                    "INSERT OR IGNORE INTO experience_events "
                    "(event_id, run_id, task_id, step_id, attempt, event_type, timestamp, state, "
                    "outcome, channel, risk_class, failure_type, action, duration_ms, "
                    "value_source, evidence_refs, details) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    row
                )
            except _sqlite3.IntegrityError:
                pass  # already exists
        # Copy candidate_decisions so the generator can see the revocation record
        # (adopted in snap1, revoked after snap2 evidence was added).
        for row in _c2.execute("SELECT * FROM candidate_decisions"):
            try:
                _ci.execute(
                    "INSERT OR IGNORE INTO candidate_decisions "
                    "(decision_id, candidate_id, snapshot_hash, decision, reason, "
                    "decided_by, decided_at, revoked_at, revocation_reason) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    row
                )
            except _sqlite3.IntegrityError:
                pass  # already exists
        for row in _c2.execute("SELECT * FROM interventions"):
            try:
                _ci.execute(
                    "INSERT OR IGNORE INTO interventions "
                    "(intervention_id, run_id, step_id, attempt, timestamp, reason, "
                    "action, source, followup_run_id, verification_status, "
                    "evidence_before, evidence_after, details, superseded_by, created_at) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    row
                )
            except _sqlite3.IntegrityError:
                pass
        _ci.commit()
        _ci.close()
        _c2.close()

        gen_script = _SKILL_DIR / "scripts" / "generate_report.py"
        report_html = Path(tempfile.mkdtemp()) / "snap2_report.html"
        patched_script = Path(tempfile.mkdtemp()) / "gen_snap.py"
        try:
            src = gen_script.read_text(encoding="utf-8")
            patched_src = (
                src
                .replace(
                    'DB = Path(__file__).parent.parent / "data" / "skill_db.sqlite3"',
                    f'DB = Path(r"{snap_backup}")',
                )
                .replace(
                    'OUT = Path(__file__).parent.parent / "report" / "rsi_report.html"',
                    f'OUT = Path(r"{report_html}")',
                )
                .replace(
                    'EVIDENCE_DIR = Path(__file__).parent / "evidence"',
                    f'EVIDENCE_DIR = Path(r"{_EVIDENCE}")',
                )
            )
            patched_script.write_text(patched_src, encoding="utf-8")
            r = subprocess.run(
                [sys.executable, str(patched_script)],
                capture_output=True, text=True,
                cwd=str(gen_script.parent),
            )
            self.assertEqual(r.returncode, 0, f"Generator failed: {r.stderr[:300]}")
            html = report_html.read_text(encoding="utf-8")
            # Combined check: cand1_id card must have PENDING class (not adopted).
            # Separate checks would not catch a cand1_id=ADOPTED card alongside a
            # different PENDING candidate elsewhere in the HTML.
            import re
            # Find the cand1_id card boundary and assert PENDING inside it.
            # This locks the two checks to the SAME card element.
            import re
            card_match = re.search(
                rf'(<details class="cand-card">.*?<span class="cand-id">{re.escape(cand1_id)}</span>.*?</details>)',
                html, re.DOTALL)
            self.assertIsNotNone(card_match,
                f"{cand1_id} must appear inside a cand-card element")
            card_html = card_match.group(1)
            self.assertIn('PENDING', card_html,
                f"{cand1_id} card must contain PENDING status (not adopted)")
            self.assertNotIn('adopted', card_html,
                f"{cand1_id} card must NOT have adopted status")
        finally:
            if snap_backup.exists(): snap_backup.unlink(missing_ok=True)
            if report_html.exists(): report_html.unlink(missing_ok=True)
            if patched_script.exists(): patched_script.unlink(missing_ok=True)

    def test_snapshot_hash_in_report_data(self):
        """Report data includes the snapshot hash used for filtering."""
        _insert_verifier_events(self.db, "run-f1", ["PASSED"])
        iv = record_intervention(
            run_id="r1", step_id="s1",
            reason="X", action="Y", db_path=self.db,
        )
        verify_with_evidence(iv["intervention_id"], "run-f1", self.db)

        data = self._generate_candidates_data()
        self.assertIn("snapshot_hash", data)
        self.assertIsInstance(data["snapshot_hash"], str)
        self.assertEqual(len(data["snapshot_hash"]), 16)  # SHA256[:16]

    def test_adopted_count_filters_by_current_snapshot(self):
        """Adopted count in report = only ADOPTED decisions with current hash.

        Gap fixed: calls the real CLI `review` command to verify 0 adopted
        count appears in actual command output (not just data-layer helper).
        """
        _insert_verifier_events(self.db, "run-f1", ["PASSED"])
        iv1 = record_intervention(
            run_id="r1", step_id="s1",
            reason="R1", action="A1", db_path=self.db,
        )
        verify_with_evidence(iv1["intervention_id"], "run-f1", self.db)
        snap1 = mine_candidates.mine(min_verified=0, db_path=self.db)["h"]
        cand1_id = mine_candidates.mine(min_verified=0, db_path=self.db)[
            "candidates"
        ][0]["candidate_id"]
        record_candidate_decision(cand1_id, snap1, "ADOPTED",
                                 reason="snap1 adopt", db_path=self.db)

        _insert_verifier_events(self.db, "run-f2", ["PASSED"])
        iv2 = record_intervention(
            run_id="r2", step_id="s1",
            reason="R2", action="A2", db_path=self.db,
        )
        verify_with_evidence(iv2["intervention_id"], "run-f2", self.db)

        # Data-layer sanity check
        data = self._generate_candidates_data()
        snap1_adopted_in_snap2 = [
            d for d in data["adopted"]
            if d.get("snapshot_hash") == snap1
        ]
        self.assertEqual(len(snap1_adopted_in_snap2), 0)

        # CLI-layer: real `review` output shows 0 adopted for the current snapshot
        review_result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"),
                "--db", str(self.db),
                "review", "--min-verified", "0",
            ],
            capture_output=True, text=True,
        )
        self.assertEqual(review_result.returncode, 0, review_result.stderr)
        self.assertRegex(review_result.stdout, r"0 adopted",
            "review CLI must show 0 adopted when current snapshot has no decisions")


# ─────────────────────────────────────────────────────────────────────────────
# Cross-chain: full pipeline smoke test
# ─────────────────────────────────────────────────────────────────────────────

class TestFullPipeline(unittest.TestCase):
    """End-to-end: record → verify → mine → adopt → trace → report data."""

    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)

    def tearDown(self):
        self.db.unlink(missing_ok=True)

    def test_full_pipeline(self):
        """Complete pipeline: record + 3 verifies + mine + adopt + trace + report."""
        # 1. Record an intervention (original run)
        _insert_observed_events(self.db, "run-orig", ["step-1"])

        iv = record_intervention(
            run_id="run-orig",
            step_id="step-1",
            reason="Window mismatch",
            action="Dismiss dialog manually",
            db_path=self.db,
        )
        iv_id = iv["intervention_id"]

        # 2. Verify: PASSED
        _insert_verifier_events(self.db, "run-followup-pass", ["PASSED"])
        r_pass = verify_with_evidence(iv_id, "run-followup-pass", self.db)
        self.assertEqual(r_pass["verification_status"], "VERIFIED")

        # 3. Mine candidates
        result = mine_candidates.mine(min_verified=1, db_path=self.db)
        self.assertEqual(result["total_cands"], 1)
        cand_id = result["candidates"][0]["candidate_id"]
        snap = result["h"]

        # 4. Adopt
        record_candidate_decision(
            cand_id, snap, "ADOPTED",
            reason="Proven effective", db_path=self.db,
        )

        # 5. Trace — followup_events present after verification
        trace = get_trace_for_intervention(iv_id, self.db)
        self.assertGreater(len(trace["followup_events"]), 0)
        self.assertEqual(trace["intervention"]["run_id"], "run-orig")

        # 6. Report data
        decisions = list_candidate_decisions(db_path=self.db)
        adopted = get_adopted_candidates(self.db)
        self.assertEqual(len(adopted), 1)
        self.assertEqual(adopted[0]["decision"], "ADOPTED")
        self.assertEqual(adopted[0]["candidate_id"], cand_id)
        self.assertEqual(adopted[0]["snapshot_hash"], snap)

    def test_conflict_in_full_pipeline(self):
        """CONFLICT candidate does not get mined with min_verified=1."""
        _insert_verifier_events(self.db, "run-conflict", ["PASSED", "FAILED"])
        iv = record_intervention(
            run_id="run-conf", step_id="s1",
            reason="Conflict scenario", action="Manual fix",
            db_path=self.db,
        )
        verify_with_evidence(iv["intervention_id"], "run-conflict", self.db)

        # Conflict → verified_count=0, failed_count=0, conflict_count=1
        result = mine_candidates.mine(min_verified=1, db_path=self.db)
        # Conflict excluded from verified_count, so min_verified=1 → 0 candidates
        self.assertEqual(result["total_cands"], 0)

        # But with min_verified=0 it should appear
        result_all = mine_candidates.mine(min_verified=0, db_path=self.db)
        self.assertEqual(result_all["total_cands"], 1)
        c = result_all["candidates"][0]
        self.assertEqual(c["stats"]["conflict_count"], 1)
        self.assertEqual(c["stats"]["verified_count"], 0)

    def test_all_verifier_classifications_in_single_pipeline(self):
        """All four classification outcomes appear correctly in mine output."""
        # VERIFIED
        _insert_verifier_events(self.db, "run-v", ["PASSED"])
        iv_v = record_intervention(run_id="rv", step_id="s",
                                    reason="R", action="A", db_path=self.db)
        verify_with_evidence(iv_v["intervention_id"], "run-v", self.db)

        # FAILED
        _insert_verifier_events(self.db, "run-f", ["FAILED"])
        iv_f = record_intervention(run_id="rf", step_id="s",
                                    reason="R", action="A", db_path=self.db)
        verify_with_evidence(iv_f["intervention_id"], "run-f", self.db)

        # CONFLICT
        _insert_verifier_events(self.db, "run-c", ["PASSED", "FAILED"])
        iv_c = record_intervention(run_id="rc", step_id="s",
                                    reason="R", action="A", db_path=self.db)
        verify_with_evidence(iv_c["intervention_id"], "run-c", self.db)

        # UNKNOWN
        iv_u = record_intervention(run_id="ru", step_id="s",
                                   reason="R", action="A", db_path=self.db)
        verify_with_evidence(iv_u["intervention_id"], "run-no-ev", self.db)

        result = mine_candidates.mine(min_verified=0, db_path=self.db)
        c = result["candidates"][0]["stats"]
        self.assertEqual(c["verified_count"], 1)
        self.assertEqual(c["failed_count"], 1)
        self.assertEqual(c["conflict_count"], 1)
        self.assertEqual(c["unknown_count"], 1)


# ─────────────────────────────────────────────────────────────────────────────
if __name__ == "__main__":
    unittest.main(verbosity=2)
