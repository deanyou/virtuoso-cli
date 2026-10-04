#!/usr/bin/env python3
"""
Unit tests for mine_candidates.py - focusing on aggregation logic.
"""

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
    mine_candidates,
    get_interventions,
    _canonicalize_intervention,
    _strip_verification_metadata,
    _normalize_step_id,
)
from record_intervention import (
    init_db,
    record_intervention,
    update_verification_status,
    verify_with_evidence,
)


def _insert_verifier_events(db_path: Path, outcomes: dict):
    """Insert VERIFIER_CONFIRMED events. outcomes maps followup_id -> outcome."""
    conn = init_db(db_path)
    for followup_id, outcome in outcomes.items():
        run_id = followup_id if followup_id.startswith("followup-") else f"followup-{followup_id}"
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', ?)
        """, (f"evt-{followup_id}", run_id, outcome))
    conn.commit()
    conn.close()


class TestAggregation(unittest.TestCase):
    """Test aggregation logic: deduplication, conflict resolution, counts."""
    
    def setUp(self):
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        self.temp_db.unlink(missing_ok=True)
    
    def test_same_run_multiple_followups_counts_once(self):
        """Same original run with 3 followups = 1 support (not 3)."""
        # Insert 3 different followups
        for i in range(1, 4):
            conn = init_db(self.temp_db)
            conn.execute("""
                INSERT INTO experience_events 
                (event_id, run_id, step_id, event_type, state, value_source, outcome)
                VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
            """, (f"evt-{i}", f"followup-{i}"))
            conn.commit()
            conn.close()
        
        # Create 3 interventions for SAME original run
        for i in range(3):
            record_intervention(
                run_id="same-run",
                step_id=f"step-{i}",
                reason="Test",
                action="Test",
                db_path=self.temp_db,
            )
        
        # Verify with different followups
        interventions = get_interventions(self.temp_db)
        for i, iv in enumerate(interventions):
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id=f"followup-{i+1}",
                db_path=self.temp_db,
            )
        
        result = mine_candidates(db_path=self.temp_db)
        
        # Same original run should count as 1 support
        self.assertGreaterEqual(len(result["candidates"]), 1)
        verified_count = result["candidates"][0]["stats"]["verified_evidence_count"]
        self.assertEqual(verified_count, 1)
    
    def test_unknown_runs_use_original_run_id(self):
        """5 UNKNOWN interventions = 5 unknown_count."""
        for i in range(5):
            record_intervention(
                run_id=f"unknown-run-{i}",
                step_id="step-1",
                reason="Test",
                action="Test",
                db_path=self.temp_db,
            )
        # No verification = all UNKNOWN
        
        result = mine_candidates(min_verified=0, db_path=self.temp_db)
        
        unknown_count = result["candidates"][0]["stats"]["unknown_count"]
        self.assertEqual(unknown_count, 5)
    
    def test_manual_verified_uses_original_run_id(self):
        """5 manual VERIFIED = 5 manual_count."""
        for i in range(5):
            record_intervention(
                run_id=f"manual-run-{i}",
                step_id="step-1",
                reason="Test",
                action="Test",
                db_path=self.temp_db,
            )
        
        interventions = get_interventions(self.temp_db)
        for iv in interventions:
            update_verification_status(
                intervention_id=iv["intervention_id"],
                verification_status="VERIFIED",
                verification_source="manual",
                db_path=self.temp_db,
            )
        
        result = mine_candidates(min_verified=0, db_path=self.temp_db)
        
        manual_count = result["candidates"][0]["stats"]["verified_manual_count"]
        self.assertEqual(manual_count, 5)
    
    def test_conflicting_followup_produces_failed(self):
        """Same followup with PASSED+FAILED history = FAILED status."""
        # Insert both outcomes for same followup
        conn = init_db(self.temp_db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES ('evt-pass', 'followup-1', 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES ('evt-fail', 'followup-1', 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'FAILED')
        """)
        conn.commit()
        conn.close()
        
        record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        interventions = get_interventions(self.temp_db)
        verify_result = verify_with_evidence(
            intervention_id=interventions[0]["intervention_id"],
            followup_run_id="followup-1",
            db_path=self.temp_db,
        )
        
        # Conflict should produce FAILED status (verified_with_evidence handles this)
        self.assertEqual(verify_result["verification_status"], "FAILED")
        self.assertEqual(verify_result["derived_from"], "actual_failed_outcome")


class TestDeterminism(unittest.TestCase):
    """Test deterministic output."""
    
    def setUp(self):
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        self.temp_db.unlink(missing_ok=True)
    
    def test_source_in_hash(self):
        """Changing source changes hash."""
        record_intervention(
            run_id="run-1", step_id="step-1",
            reason="Test", action="Test",
            db_path=self.temp_db,
        )
        
        result1 = mine_candidates(db_path=self.temp_db)
        hash1 = result1["snapshot_hash"]
        
        interventions = get_interventions(self.temp_db)
        update_verification_status(
            intervention_id=interventions[0]["intervention_id"],
            verification_status="VERIFIED",
            verification_source="manual",
            db_path=self.temp_db,
        )
        
        result2 = mine_candidates(db_path=self.temp_db)
        self.assertNotEqual(hash1, result2["snapshot_hash"])


class TestNormalization(unittest.TestCase):
    """Test normalization helpers."""
    
    def test_strip_verification_metadata(self):
        details = {"_verification": {"source": "manual"}, "view": "schematic"}
        result = _strip_verification_metadata(details)
        self.assertNotIn("_verification", result)
        self.assertEqual(result["view"], "schematic")
    
    def test_normalize_step_id(self):
        self.assertEqual(_normalize_step_id(None), "")
        self.assertEqual(_normalize_step_id("step-1"), "step-1")


class TestCLI(unittest.TestCase):
    """Test CLI interface."""
    
    def setUp(self):
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        self.temp_db.unlink(missing_ok=True)
    
    def test_deterministic(self):
        """Two mine commands produce same hash."""
        result1 = subprocess.run(
            [sys.executable, str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
             "--db", str(self.temp_db), "mine"],
            capture_output=True, text=True,
        )
        result2 = subprocess.run(
            [sys.executable, str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
             "--db", str(self.temp_db), "mine"],
            capture_output=True, text=True,
        )
        hash1 = json.loads(result1.stdout)["snapshot_hash"]
        hash2 = json.loads(result2.stdout)["snapshot_hash"]
        self.assertEqual(hash1, hash2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
