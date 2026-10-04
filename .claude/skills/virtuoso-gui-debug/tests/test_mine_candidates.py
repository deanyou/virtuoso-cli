#!/usr/bin/env python3
"""Unit tests for mine_candidates.py."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

_SCRIPT_DIR = Path(__file__).parent.resolve()
_SKILL_DIR = _SCRIPT_DIR.parent
sys.path.insert(0, str(_SKILL_DIR / "scripts" / "evidence"))

from mine_candidates import (
    mine,
    get_interventions,
    norm_step,
    strip_verification,
    compute_stats,
)
from record_intervention import (
    init_db,
    record_intervention,
    update_verification_status,
    verify_with_evidence,
)


def insert_verifier_events(db_path, outcomes):
    conn = init_db(db_path)
    for fid, outcome in outcomes.items():
        run_id = fid if fid.startswith("followup-") else "followup-" + fid
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', ?)
        """, ("evt-" + fid, run_id, outcome))
    conn.commit()
    conn.close()


class TestAggregation(unittest.TestCase):
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)
    
    def tearDown(self):
        self.db.unlink(missing_ok=True)
    
    def test_shared_followup_counts_once(self):
        """3 different runs sharing same followup = 1 support."""
        insert_verifier_events(self.db, {"shared": "PASSED"})
        for i in range(3):
            record_intervention(
                run_id="run-" + str(i), step_id="step-1",
                reason="Test", action="Test", db_path=self.db)
        ivs = get_interventions(self.db)
        for iv in ivs:
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id="followup-shared", db_path=self.db)
        result = mine(db_path=self.db)
        self.assertEqual(result["total_cands"], 1)
        self.assertEqual(result["candidates"][0]["stats"]["verified_count"], 1)
    
    def test_same_run_multiple_followups_counts_once(self):
        """Same run with 3 different followups = 1 support."""
        for i in range(1, 4):
            conn = init_db(self.db)
            conn.execute("""
                INSERT INTO experience_events 
                (event_id, run_id, step_id, event_type, state, value_source, outcome)
                VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
            """, ("evt-" + str(i), "followup-" + str(i)))
            conn.commit()
            conn.close()
        record_intervention(run_id="same-run", step_id="step-1",
                         reason="Test", action="Test", db_path=self.db)
        ivs = get_interventions(self.db)
        for i, iv in enumerate(ivs):
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id="followup-" + str(i + 1), db_path=self.db)
        result = mine(db_path=self.db)
        self.assertEqual(result["candidates"][0]["stats"]["verified_count"], 1)
    
    def test_unknown_runs_count(self):
        """5 UNKNOWN interventions = 5 unknown_count."""
        for i in range(5):
            record_intervention(
                run_id="unknown-" + str(i), step_id="step-1",
                reason="Test", action="Test", db_path=self.db)
        result = mine(min_verified=0, db_path=self.db)
        self.assertEqual(result["candidates"][0]["stats"]["unknown_count"], 5)
    
    def test_manual_count(self):
        """5 manual VERIFIED = 5 manual_count."""
        for i in range(5):
            record_intervention(
                run_id="manual-" + str(i), step_id="step-1",
                reason="Test", action="Test", db_path=self.db)
        ivs = get_interventions(self.db)
        for iv in ivs:
            update_verification_status(
                intervention_id=iv["intervention_id"],
                verification_status="VERIFIED",
                verification_source="manual", db_path=self.db)
        result = mine(min_verified=0, db_path=self.db)
        self.assertEqual(result["candidates"][0]["stats"]["manual_count"], 5)
    
    def test_conflict_excluded_from_counts(self):
        """Conflicting followup excluded from verified/failed counts."""
        conn = init_db(self.db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES ('p', 'shared', 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES ('f', 'shared', 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'FAILED')
        """)
        conn.commit()
        conn.close()
        record_intervention(run_id="run-1", step_id="step-1",
                         reason="Test", action="Test", db_path=self.db)
        ivs = get_interventions(self.db)
        verify_with_evidence(
            intervention_id=ivs[0]["intervention_id"],
            followup_run_id="shared", db_path=self.db)
        result = mine(min_verified=0, db_path=self.db)
        c = result["candidates"][0]
        # Conflict should be tracked
        self.assertGreater(len(c["stats"]["conflicts"]), 0)
        # But not counted as verified or failed
        self.assertEqual(c["stats"]["verified_count"], 0)
        self.assertEqual(c["stats"]["failed_count"], 0)


class TestNormalization(unittest.TestCase):
    def test_norm_step(self):
        self.assertEqual(norm_step(None), "")
        self.assertEqual(norm_step("step-1"), "step-1")
    
    def test_strip_verification(self):
        d = {"_verification": {"src": "manual"}, "view": "schematic"}
        r = strip_verification(d)
        self.assertNotIn("_verification", r)
        self.assertEqual(r["view"], "schematic")


class TestCLI(unittest.TestCase):
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)
    
    def tearDown(self):
        self.db.unlink(missing_ok=True)
    
    def test_deterministic(self):
        r1 = subprocess.run([sys.executable, str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
                           "--db", str(self.db), "mine"],
                          capture_output=True, text=True)
        r2 = subprocess.run([sys.executable, str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
                           "--db", str(self.db), "mine"],
                          capture_output=True, text=True)
        h1 = json.loads(r1.stdout)["h"]
        h2 = json.loads(r2.stdout)["h"]
        self.assertEqual(h1, h2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
