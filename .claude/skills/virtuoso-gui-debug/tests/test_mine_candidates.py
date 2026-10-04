#!/usr/bin/env python3
"""
Unit tests for mine_candidates.py

Tests cover:
1. Deterministic output (same data -> same hash)
2. Evidence vs manual source handling
3. Candidate strength classification
4. Min verified threshold filtering
5. Report generation

Run with: python3 test_mine_candidates.py
Compatible with Python 3.9 and 3.13
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

# Add script to path
_SCRIPT_DIR = Path(__file__).parent.resolve()  # virtuoso-gui-debug/tests
_SKILL_DIR = _SCRIPT_DIR.parent  # virtuoso-gui-debug
sys.path.insert(0, str(_SKILL_DIR / "scripts" / "evidence"))

from mine_candidates import (
    mine_candidates,
    get_intervention_stats,
    generate_report,
    _hash_snapshot,
    TRUSTED_SOURCES,
    CANDIDATE_SCHEMA_VERSION,
)
from record_intervention import (
    init_db,
    record_intervention,
    update_verification_status,
    verify_with_evidence,
)


class TestDeterminism(unittest.TestCase):
    """Test 1: Same data always produces same candidates."""
    
    def test_same_data_same_hash(self):
        """Identical data produces identical hash."""
        data = {"test": "value", "number": 42}
        hash1 = _hash_snapshot(data)
        hash2 = _hash_snapshot(data)
        self.assertEqual(hash1, hash2)
    
    def test_different_data_different_hash(self):
        """Different data produces different hash."""
        data1 = {"test": "value1"}
        data2 = {"test": "value2"}
        hash1 = _hash_snapshot(data1)
        hash2 = _hash_snapshot(data2)
        self.assertNotEqual(hash1, hash2)
    
    def test_key_order_independent(self):
        """Key order doesn't affect hash."""
        data1 = {"a": 1, "b": 2, "c": 3}
        data2 = {"c": 3, "a": 1, "b": 2}
        hash1 = _hash_snapshot(data1)
        hash2 = _hash_snapshot(data2)
        self.assertEqual(hash1, hash2)


class TestEvidenceVsManualSource(unittest.TestCase):
    """Test 2: Evidence vs manual source handling."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        
        # Insert mock VERIFIER_CONFIRMED event
        conn = init_db(self.temp_db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES ('evt-1', 'followup-evidence', 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES ('evt-2', 'followup-failed', 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'FAILED')
        """)
        conn.commit()
        conn.close()
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_evidence_source_verified_counts_as_trusted(self):
        """Evidence source + VERIFIED counts as trusted evidence."""
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Window identity ambiguous",
            action="Selected target window",
            db_path=self.temp_db,
        )
        
        # Use verify_with_evidence (source='evidence')
        result = verify_with_evidence(
            intervention_id=record["intervention_id"],
            followup_run_id="followup-evidence",
            db_path=self.temp_db,
        )
        
        self.assertEqual(result["verification_status"], "VERIFIED")
        
        # Mine candidates
        stats = get_intervention_stats(self.temp_db)
        self.assertEqual(len(stats["groups"]), 1)
        group = stats["groups"][0]
        self.assertEqual(group["stats"]["verified_evidence_count"], 1)
        self.assertEqual(group["stats"]["verified_manual_count"], 0)
    
    def test_manual_source_verified_excluded_from_trusted(self):
        """Manual source + VERIFIED excluded from trusted evidence."""
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Window identity ambiguous",
            action="Selected target window",
            db_path=self.temp_db,
        )
        
        # Use manual update (source='manual')
        update_verification_status(
            intervention_id=record["intervention_id"],
            verification_status="VERIFIED",
            verification_source="manual",
            db_path=self.temp_db,
        )
        
        # Mine candidates
        stats = get_intervention_stats(self.temp_db)
        group = stats["groups"][0]
        # Manual should NOT count as trusted evidence
        self.assertEqual(group["stats"]["verified_evidence_count"], 0)
        self.assertEqual(group["stats"]["verified_manual_count"], 1)
    
    def test_failed_outcome_excluded_from_verified(self):
        """FAILED outcome excluded from verified count."""
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Window identity ambiguous",
            action="Selected target window",
            db_path=self.temp_db,
        )
        
        # Use verify_with_evidence with FAILED outcome
        result = verify_with_evidence(
            intervention_id=record["intervention_id"],
            followup_run_id="followup-failed",
            db_path=self.temp_db,
        )
        
        self.assertEqual(result["verification_status"], "FAILED")
        
        # Mine candidates
        stats = get_intervention_stats(self.temp_db)
        group = stats["groups"][0]
        self.assertEqual(group["stats"]["failed_count"], 1)
        self.assertEqual(group["stats"]["verified_evidence_count"], 0)


class TestCandidateStrength(unittest.TestCase):
    """Test 3: Candidate strength classification."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        
        # Insert mock VERIFIER_CONFIRMED event
        conn = init_db(self.temp_db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES ('evt-ev', 'followup', 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """)
        conn.commit()
        conn.close()
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_insufficient_strength(self):
        """Less than 1 verified evidence = insufficient."""
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        # No verification
        
        candidates = mine_candidates(db_path=self.temp_db)
        self.assertEqual(len(candidates["candidates"]), 0)
    
    def test_moderate_strength(self):
        """1 verified evidence = moderate strength."""
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        verify_with_evidence(
            intervention_id=record["intervention_id"],
            followup_run_id="followup",
            db_path=self.temp_db,
        )
        
        candidates = mine_candidates(db_path=self.temp_db)
        self.assertEqual(len(candidates["candidates"]), 1)
        self.assertEqual(candidates["candidates"][0]["strength"], "moderate")


class TestMinVerifiedThreshold(unittest.TestCase):
    """Test 4: Min verified threshold filtering."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        
        # Insert mock VERIFIER_CONFIRMED event
        conn = init_db(self.temp_db)
        for i in range(5):
            conn.execute("""
                INSERT INTO experience_events 
                (event_id, run_id, step_id, event_type, state, value_source, outcome)
                VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
            """, (f"evt-{i}", f"followup-{i}"))
        conn.commit()
        conn.close()
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_min_verified_filters_candidates(self):
        """Higher threshold filters out candidates."""
        # Create 3 interventions with DIFFERENT reasons (different groups)
        for i in range(3):
            record = record_intervention(
                run_id=f"test-run-{i}",
                step_id="step-1",
                reason=f"Reason {i}",  # Different reasons = different groups
                action="Test",
                db_path=self.temp_db,
            )
            verify_with_evidence(
                intervention_id=record["intervention_id"],
                followup_run_id=f"followup-{i}",
                db_path=self.temp_db,
            )
        
        # Default threshold (1) should include all 3 candidates
        candidates = mine_candidates(min_verified=1, db_path=self.temp_db)
        self.assertEqual(len(candidates["candidates"]), 3)
        
        # Threshold of 2 should exclude all (each group has only 1)
        candidates = mine_candidates(min_verified=2, db_path=self.temp_db)
        self.assertEqual(len(candidates["candidates"]), 0)


class TestReportGeneration(unittest.TestCase):
    """Test 5: Report generation."""
    
    def test_report_contains_key_sections(self):
        """Report contains expected sections."""
        candidates = {
            "generated_at": "2024-01-01T00:00:00Z",
            "schema_version": "1.0",
            "snapshot_hash": "abc123",
            "total_interventions_analyzed": 0,
            "total_candidates": 0,
            "min_verified_threshold": 1,
            "candidates": [],
            "evidence_note": "Only evidence sources count.",
        }
        
        report = generate_report(candidates)
        
        self.assertIn("Candidate Analysis Report", report)
        self.assertIn("Schema: v1.0", report)
        self.assertIn("Snapshot:", report)
        self.assertIn("CANDIDATES", report)
        self.assertIn("NOTE", report)


class TestCLI(unittest.TestCase):
    """Test CLI interface."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_stats_command(self):
        """stats command returns JSON."""
        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
                "--db", str(self.temp_db),
                "stats",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0)
        data = json.loads(result.stdout)
        self.assertIn("total_interventions", data)
    
    def test_mine_command(self):
        """mine command returns JSON."""
        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
                "--db", str(self.temp_db),
                "mine",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0)
        data = json.loads(result.stdout)
        self.assertIn("candidates", data)
        self.assertIn("snapshot_hash", data)
    
    def test_mine_report_format(self):
        """mine --format report returns text."""
        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
                "--db", str(self.temp_db),
                "mine",
                "--format", "report",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("Report", result.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
