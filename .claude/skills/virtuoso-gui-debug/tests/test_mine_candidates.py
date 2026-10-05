#!/usr/bin/env python3
"""Unit tests for mine_candidates.py - TRIPLE deduplication by run, followup, conflict."""

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
    _ref_sort_key,
)
from record_intervention import (
    init_db,
    record_intervention,
    update_verification_status,
    verify_with_evidence,
)


def insert_verifier_events(db_path, outcomes):
    """Insert VERIFIER_CONFIRMED events into experience_events."""
    conn = init_db(db_path)
    for fid, outcome in outcomes.items():
        run_id = "followup-" + fid if not fid.startswith("followup-") else fid
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', ?)
        """, ("evt-" + fid, run_id, outcome))
    conn.commit()
    conn.close()


class TestTripleDeduplication(unittest.TestCase):
    """Test TRIPLE deduplication: by original run, by shared followup, by conflict."""
    
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)
    
    def tearDown(self):
        self.db.unlink(missing_ok=True)
    
    def test_shared_followup_counts_once(self):
        """3 different runs sharing same followup = 1 support, 3 attempts.
        
        Union-Find connectivity: 3 runs connected via shared followup = 1 component.
        """
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
        c = result["candidates"][0]["stats"]
        # 3 runs share 1 followup → 1 connected component → 1 support
        self.assertEqual(c["verified_count"], 1)
        self.assertEqual(c["attempts"], 3)
    
    def test_same_run_multiple_followups_counts_once(self):
        """Same run with 3 followups = 1 support, 3 attempts.
        
        Union-Find connectivity: run connected to 3 followups = 1 component.
        """
        for i in range(1, 4):
            conn = init_db(self.db)
            run_id = "followup-" + str(i)
            conn.execute("""
                INSERT INTO experience_events 
                (event_id, run_id, step_id, event_type, state, value_source, outcome)
                VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
            """, ("evt-" + str(i), run_id))
            conn.commit()
            conn.close()
        
        # Create THREE interventions in SAME run
        for i in range(3):
            record_intervention(
                run_id="same-run", step_id="step-" + str(i),
                reason="Test", action="Test", db_path=self.db)
        
        ivs = get_interventions(self.db)
        self.assertEqual(len(ivs), 3)
        
        for i, iv in enumerate(ivs):
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id="followup-" + str(i + 1), db_path=self.db)
        
        result = mine(db_path=self.db)
        c = result["candidates"][0]["stats"]
        # Same run, 3 followups → 1 connected component → 1 support
        self.assertEqual(c["verified_count"], 1)
        self.assertEqual(c["attempts"], 3)
    
    def test_different_runs_different_support(self):
        """3 different runs with different followups = 3 support, 3 attempts."""
        for i in range(1, 4):
            conn = init_db(self.db)
            run_id = "followup-" + str(i)
            conn.execute("""
                INSERT INTO experience_events 
                (event_id, run_id, step_id, event_type, state, value_source, outcome)
                VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
            """, ("evt-" + str(i), run_id))
            conn.commit()
            conn.close()
        
        for i in range(3):
            record_intervention(
                run_id="run-" + str(i), step_id="step-1",
                reason="Test", action="Test", db_path=self.db)
        
        ivs = get_interventions(self.db)
        for i, iv in enumerate(ivs):
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id="followup-" + str(i + 1), db_path=self.db)
        
        result = mine(db_path=self.db)
        c = result["candidates"][0]["stats"]
        # 3 separate components → 3 support
        self.assertEqual(c["verified_count"], 3)
        self.assertEqual(c["attempts"], 3)
    
    def test_explicit_conflict_excludes_support(self):
        """Same run with VERIFIED + CONFLICT = 0 support, 1 attempt.
        
        Union-Find connectivity: CONFLICT status → component excluded.
        """
        # Insert PASSED event
        insert_verifier_events(self.db, {"cf": "PASSED"})
        # Add FAILED event to same followup
        conn = init_db(self.db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES ('evt-fail', 'followup-cf', 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'FAILED')
        """)
        conn.commit()
        conn.close()
        
        record_intervention(
            run_id="conflict-run", step_id="step-1",
            reason="Test", action="Test", db_path=self.db)
        ivs = get_interventions(self.db)
        # verify_with_evidence will detect PASSED + FAILED → CONFLICT
        verify_with_evidence(
            intervention_id=ivs[0]["intervention_id"],
            followup_run_id="followup-cf", db_path=self.db)
        
        result = mine(min_verified=0, db_path=self.db)
        c = result["candidates"][0]["stats"]
        # CONFLICT detected → excluded from both verified and failed
        self.assertEqual(c["verified_count"], 0)
        self.assertEqual(c["failed_count"], 0)
        self.assertEqual(c["conflict_count"], 1)
        self.assertEqual(c["attempts"], 1)
    
    def test_manual_conflict_does_not_pollute_trusted(self):
        """1 trusted VERIFIED + 1 manual CONFLICT = 1 trusted, 2 attempts."""
        insert_verifier_events(self.db, {"trusted": "PASSED"})
        
        iv_trusted = record_intervention(
            run_id="trusted-run", step_id="step-1",
            reason="Test", action="Test", db_path=self.db)
        verify_with_evidence(
            intervention_id=iv_trusted["intervention_id"],
            followup_run_id="followup-trusted", db_path=self.db)
        
        iv_manual = record_intervention(
            run_id="manual-conflict-run", step_id="step-1",
            reason="Test", action="Test", db_path=self.db)
        update_verification_status(
            intervention_id=iv_manual["intervention_id"],
            verification_status="CONFLICT",
            verification_source="manual", db_path=self.db)
        
        result = mine(min_verified=0, db_path=self.db)
        c = result["candidates"][0]["stats"]
        # Trusted support preserved (separate component), manual conflict isolated
        self.assertEqual(c["verified_count"], 1)
        self.assertEqual(c["conflict_count"], 0)
        self.assertEqual(c["manual_count"], 1)


class TestEvidenceRefs(unittest.TestCase):
    """Test that evidence refs are preserved for traceability."""
    
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)
    
    def tearDown(self):
        self.db.unlink(missing_ok=True)
    
    def test_vrefs_preserved(self):
        """Verified interventions have refs in vrefs."""
        insert_verifier_events(self.db, {"v1": "PASSED"})
        iv = record_intervention(
            run_id="run-1", step_id="step-1",
            reason="Test", action="Test", db_path=self.db)
        verify_with_evidence(
            intervention_id=iv["intervention_id"],
            followup_run_id="followup-v1", db_path=self.db)
        
        result = mine(db_path=self.db)
        c = result["candidates"][0]
        self.assertEqual(len(c["vrefs"]), 1)
        self.assertEqual(c["vrefs"][0]["run"], "run-1")
    
    def test_cref_preserved_for_conflict(self):
        """Conflicting interventions have refs in cref."""
        insert_verifier_events(self.db, {"cf": "PASSED"})
        conn = init_db(self.db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES ('evt-fail', 'followup-cf', 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'FAILED')
        """)
        conn.commit()
        conn.close()
        
        iv = record_intervention(
            run_id="conflict-run", step_id="step-1",
            reason="Test", action="Test", db_path=self.db)
        verify_with_evidence(
            intervention_id=iv["intervention_id"],
            followup_run_id="followup-cf", db_path=self.db)
        
        result = mine(min_verified=0, db_path=self.db)
        c = result["candidates"][0]
        self.assertGreater(len(c["cref"]), 0)


class TestCounts(unittest.TestCase):
    """Test various count scenarios."""
    
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)
    
    def tearDown(self):
        self.db.unlink(missing_ok=True)
    
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


class TestDeterminism(unittest.TestCase):
    """Test output determinism."""
    
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)
    
    def tearDown(self):
        self.db.unlink(missing_ok=True)
    
    def test_deterministic_hash(self):
        """Hash is deterministic across runs."""
        for i in range(3):
            record_intervention(
                run_id="run-" + str(i), step_id="step-1",
                reason="Test", action="Test", db_path=self.db)
        r1 = subprocess.run([sys.executable, str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
                           "--db", str(self.db), "mine"],
                          capture_output=True, text=True)
        r2 = subprocess.run([sys.executable, str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
                           "--db", str(self.db), "mine"],
                          capture_output=True, text=True)
        h1 = json.loads(r1.stdout)["h"]
        h2 = json.loads(r2.stdout)["h"]
        self.assertEqual(h1, h2)
    
    def test_ref_sort_key(self):
        """_ref_sort_key provides deterministic ordering."""
        refs = [
            {"run": "b", "fid": "x", "id": "2"},
            {"run": "a", "fid": "y", "id": "1"},
            {"run": "a", "fid": "x", "id": "2"},
            {"run": "a", "fid": "x", "id": "1"},
        ]
        sorted_refs = sorted(refs, key=_ref_sort_key)
        self.assertEqual(sorted_refs[0]["id"], "1")
        self.assertEqual(sorted_refs[1]["id"], "2")


class TestNormalization(unittest.TestCase):
    def test_norm_step(self):
        self.assertEqual(norm_step(None), "")
        self.assertEqual(norm_step("step-1"), "step-1")
    
    def test_strip_verification(self):
        d = {"_verification": {"src": "manual"}, "view": "schematic"}
        r = strip_verification(d)
        self.assertNotIn("_verification", r)
        self.assertEqual(r["view"], "schematic")


if __name__ == "__main__":
    unittest.main(verbosity=2)
