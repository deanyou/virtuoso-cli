#!/usr/bin/env python3
"""
Unit tests for mine_candidates.py

Tests cover the 5 key constraints:
1. Deterministic snapshot (includes source for hash stability)
2. Deduplicated support by followup_run_id
3. Correct strength (pass rate, manual separate)
4. Specific grouping (context VALUES, not just keys)
5. Traceable evidence (refs included)

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
_SCRIPT_DIR = Path(__file__).parent.resolve()
_SKILL_DIR = _SCRIPT_DIR.parent
sys.path.insert(0, str(_SKILL_DIR / "scripts" / "evidence"))

from mine_candidates import (
    mine_candidates,
    get_interventions,
    generate_report,
    _canonicalize_intervention,
    _make_group_signature,
    _strip_verification_metadata,
    _normalize_step_id,
    _compute_group_stats,
    TRUSTED_SOURCES,
)
from record_intervention import (
    init_db,
    record_intervention,
    update_verification_status,
    verify_with_evidence,
)


def _insert_verifier_events(db_path: Path, outcomes: dict):
    """Helper to insert VERIFIER_CONFIRMED events.
    
    outcomes is a dict mapping followup_id -> outcome (e.g., {"1": "PASSED"}).
    The followup_id is used directly as the run_id.
    """
    conn = init_db(db_path)
    for followup_id, outcome in outcomes.items():
        # followup_id is the run_id for the verifier events
        run_id = f"followup-{followup_id}" if not followup_id.startswith("followup-") else followup_id
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', ?)
        """, (f"evt-{followup_id}", run_id, outcome))
    conn.commit()
    conn.close()


class TestDeterminism(unittest.TestCase):
    """Test 1: Deterministic snapshot includes source for hash stability."""
    
    def setUp(self):
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        self.temp_db.unlink(missing_ok=True)
    
    def test_source_in_hash(self):
        """Changing source should change hash."""
        record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result1 = mine_candidates(db_path=self.temp_db)
        hash1 = result1["snapshot_hash"]
        
        # Change source from evidence to manual
        interventions = get_interventions(self.temp_db)
        update_verification_status(
            intervention_id=interventions[0]["intervention_id"],
            verification_status="VERIFIED",
            verification_source="manual",
            db_path=self.temp_db,
        )
        
        result2 = mine_candidates(db_path=self.temp_db)
        
        # Hash should change when source changes
        self.assertNotEqual(hash1, result2["snapshot_hash"])
    
    def test_canonicalize_includes_source(self):
        """Canonicalized intervention includes source."""
        iv = {
            "intervention_id": "test-id",
            "run_id": "run-1",
            "step_id": "step-1",
            "reason": "Test",
            "action": "Test",
            "source": "manual",
            "verification_status": "VERIFIED",
            "followup_run_id": "followup-1",
            "details": '{"_verification": {"source": "manual"}}',
            "timestamp": "2024-01-01T00:00:00Z",
        }
        
        canonical = _canonicalize_intervention(iv)
        
        self.assertEqual(canonical["source"], "manual")
        self.assertEqual(canonical["ver_source"], "manual")


class TestDeduplicatedSupport(unittest.TestCase):
    """Test 2: Support counted by unique followup_run_id."""
    
    def setUp(self):
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        _insert_verifier_events(self.temp_db, {"followup-1": "PASSED"})
    
    def tearDown(self):
        self.temp_db.unlink(missing_ok=True)
    
    def test_shared_followup_counts_once(self):
        """3 interventions sharing same followup = 1 support."""
        for i in range(3):
            record_intervention(
                run_id=f"run-{i}",
                step_id=f"step-{i}",
                reason="Test",
                action="Test",
                db_path=self.temp_db,
            )
        
        # All verify with same followup
        interventions = get_interventions(self.temp_db)
        for iv in interventions:
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id="followup-1",  # Same followup
                db_path=self.temp_db,
            )
        
        result = mine_candidates(db_path=self.temp_db)
        
        # Should have 1 candidate with 1 verified followup
        self.assertGreaterEqual(len(result["candidates"]), 1)
        # Count should be 1 (not 3)
        self.assertEqual(result["candidates"][0]["stats"]["verified_evidence_count"], 1)
        # Followups list should have only 1
        self.assertEqual(len(result["candidates"][0]["stats"]["verified_followups"]), 1)


class TestCorrectStrength(unittest.TestCase):
    """Test 3: Correct strength with source filtering."""
    
    def setUp(self):
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        _insert_verifier_events(self.temp_db, {"pass": "PASSED", "fail": "FAILED"})
    
    def tearDown(self):
        self.temp_db.unlink(missing_ok=True)
    
    def test_manual_failed_not_in_denominator(self):
        """Manual/override FAILED should not affect pass rate."""
        # Create manual FAILED
        record_intervention(
            run_id="run-manual",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        interventions = get_interventions(self.temp_db)
        iv = interventions[-1]
        
        # Manual FAILED (not counted in trusted denominator)
        update_verification_status(
            intervention_id=iv["intervention_id"],
            verification_status="FAILED",
            verification_source="override",  # Non-trusted source
            db_path=self.temp_db,
        )
        
        result = mine_candidates(db_path=self.temp_db)
        
        # No candidates with verified evidence
        self.assertEqual(len(result["candidates"]), 0)
    
    def test_manual_verified_manual_only_strength(self):
        """Manual VERIFIED should be 'manual_only' strength."""
        record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        interventions = get_interventions(self.temp_db)
        update_verification_status(
            intervention_id=interventions[0]["intervention_id"],
            verification_status="VERIFIED",
            verification_source="manual",
            db_path=self.temp_db,
        )
        
        result = mine_candidates(min_verified=0, db_path=self.temp_db)
        
        self.assertEqual(result["candidates"][0]["strength"], "manual_only")


class TestSpecificGrouping(unittest.TestCase):
    """Test 4: Context VALUES, not just keys."""
    
    def setUp(self):
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        _insert_verifier_events(self.temp_db, {"pass": "PASSED"})
    
    def tearDown(self):
        self.temp_db.unlink(missing_ok=True)
    
    def test_context_values_not_just_keys(self):
        """Different context VALUES should be separate groups."""
        record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Test",
            action="Test",
            details={"view_type": "schematic"},
            db_path=self.temp_db,
        )
        record_intervention(
            run_id="run-2",
            step_id="step-1",
            reason="Test",
            action="Test",
            details={"view_type": "layout"},  # Different VALUE
            db_path=self.temp_db,
        )
        
        interventions = get_interventions(self.temp_db)
        for iv in interventions:
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id="followup-pass",
                db_path=self.temp_db,
            )
        
        result = mine_candidates(db_path=self.temp_db)
        
        # Should be 2 groups (different context values)
        self.assertEqual(len(result["candidates"]), 2)
    
    def test_verification_metadata_excluded_from_grouping(self):
        """_verification metadata should not affect grouping."""
        record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Test",
            action="Test",
            details={"_verification": {"source": "evidence"}, "view": "schematic"},
            db_path=self.temp_db,
        )
        record_intervention(
            run_id="run-2",
            step_id="step-1",
            reason="Test",
            action="Test",
            details={"_verification": {"source": "manual"}, "view": "schematic"},
            db_path=self.temp_db,
        )
        
        interventions = get_interventions(self.temp_db)
        for iv in interventions:
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id="followup-pass",
                db_path=self.temp_db,
            )
        
        result = mine_candidates(db_path=self.temp_db)
        
        # Should be 1 group (same context except _verification)
        self.assertEqual(len(result["candidates"]), 1)


class TestNormalization(unittest.TestCase):
    """Test 5: Normalization fixes for None step_id and sorting."""
    
    def test_normalize_step_id_handles_none(self):
        """_normalize_step_id handles None."""
        self.assertEqual(_normalize_step_id(None), "")
        self.assertEqual(_normalize_step_id("step-1"), "step-1")
    
    def test_strip_verification_metadata(self):
        """_strip_verification_metadata removes _verification."""
        details = {"_verification": {"source": "manual"}, "view": "schematic"}
        result = _strip_verification_metadata(details)
        
        self.assertNotIn("_verification", result)
        self.assertEqual(result["view"], "schematic")
    
    def test_group_signature_excludes_verification(self):
        """Group signature excludes _verification."""
        sig = _make_group_signature(
            reason="Test",
            action="Test",
            context={"_verification": {"source": "evidence"}, "view": "schematic"},
        )
        
        parsed = json.loads(sig)
        self.assertNotIn("_verification", parsed["context"])
        self.assertEqual(parsed["context"]["view"], "schematic")


class TestTraceableEvidence(unittest.TestCase):
    """Test 6: Evidence references included."""
    
    def setUp(self):
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        _insert_verifier_events(self.temp_db, {"pass": "PASSED"})
    
    def tearDown(self):
        self.temp_db.unlink(missing_ok=True)
    
    def test_verified_refs_included(self):
        """Verified evidence includes intervention references."""
        record = record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        verify_with_evidence(
            intervention_id=record["intervention_id"],
            followup_run_id="followup-pass",
            db_path=self.temp_db,
        )
        
        result = mine_candidates(db_path=self.temp_db)
        
        cand = result["candidates"][0]
        self.assertIn("verified_evidence_refs", cand)
        self.assertEqual(len(cand["verified_evidence_refs"]), 1)


class TestCLI(unittest.TestCase):
    """Test CLI interface."""
    
    def setUp(self):
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        self.temp_db.unlink(missing_ok=True)
    
    def test_mine_command_deterministic(self):
        """mine command produces deterministic output."""
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
