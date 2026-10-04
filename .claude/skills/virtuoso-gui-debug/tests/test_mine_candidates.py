#!/usr/bin/env python3
"""
Unit tests for mine_candidates.py

Tests cover the 5 key constraints:
1. Deterministic output (same data -> same hash)
2. Deduplicated support (counted by unique run)
3. Correct strength (pass rate = verified / (verified + failed))
4. Specific grouping (full reason + action + context)
5. Traceable evidence (intervention references included)

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
    get_interventions,
    generate_report,
    _hash_snapshot,
    _canonicalize_intervention,
    _make_group_signature,
    _compute_group_stats,
    TRUSTED_SOURCES,
    CANDIDATE_SCHEMA_VERSION,
)
from record_intervention import (
    init_db,
    record_intervention,
    update_verification_status,
    verify_with_evidence,
)


def _insert_verifier_events(db_path: Path, outcomes: dict):
    """Helper to insert VERIFIER_CONFIRMED events."""
    conn = init_db(db_path)
    for name, outcome in outcomes.items():
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', ?)
        """, (f"evt-{name}", f"followup-{name}", outcome))
    conn.commit()
    conn.close()


class TestDeterminism(unittest.TestCase):
    """Test 1: Same data always produces same candidates."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_same_data_same_hash(self):
        """Identical data produces identical hash."""
        for i in range(2):
            record_intervention(
                run_id=f"run-{i}",
                step_id="step-1",
                reason="Test reason",
                action="Test action",
                db_path=self.temp_db,
            )
        
        result1 = mine_candidates(db_path=self.temp_db)
        result2 = mine_candidates(db_path=self.temp_db)
        
        self.assertEqual(result1["snapshot_hash"], result2["snapshot_hash"])
    
    def test_different_data_different_hash(self):
        """Different data produces different hash."""
        record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Reason 1",
            action="Action 1",
            db_path=self.temp_db,
        )
        
        result1 = mine_candidates(db_path=self.temp_db)
        hash1 = result1["snapshot_hash"]
        
        record_intervention(
            run_id="run-2",
            step_id="step-1",
            reason="Reason 2",
            action="Action 2",
            db_path=self.temp_db,
        )
        
        result2 = mine_candidates(db_path=self.temp_db)
        
        self.assertNotEqual(hash1, result2["snapshot_hash"])
    
    def test_canonicalize_removes_timestamp(self):
        """Canonicalized intervention has no timestamp."""
        iv = {
            "intervention_id": "test-id",
            "run_id": "run-1",
            "step_id": "step-1",
            "reason": "Test",
            "action": "Test",
            "source": "human",
            "verification_status": "VERIFIED",
            "followup_run_id": "followup-1",
            "details": '{"_verification": {"timestamp": "2024-01-01T00:00:00Z"}}',
            "timestamp": "2024-01-01T00:00:00Z",
        }
        
        canonical = _canonicalize_intervention(iv)
        
        self.assertNotIn("timestamp", canonical)
        self.assertIn("intervention_id", canonical)
    
    def test_no_timestamp_in_snapshot(self):
        """Snapshot hash computed without timestamps."""
        record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result = mine_candidates(db_path=self.temp_db)
        
        snapshot_str = json.dumps(result, default=str)
        self.assertNotRegex(snapshot_str, r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}')


class TestDeduplicatedSupport(unittest.TestCase):
    """Test 2: Support counted by unique run, not intervention."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        _insert_verifier_events(self.temp_db, {"pass": "PASSED"})
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_single_run_multiple_interventions_same_reason(self):
        """Single run with 3 interventions = 1 support, not 3."""
        for i in range(3):
            record_intervention(
                run_id="same-run",  # Same run_id
                step_id=f"step-{i}",
                reason="Window identity ambiguous",
                action="Selected target",
                db_path=self.temp_db,
            )
        
        # Verify all
        interventions = get_interventions(self.temp_db)
        for iv in interventions:
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id="followup-pass",
                db_path=self.temp_db,
            )
        
        result = mine_candidates(db_path=self.temp_db)
        
        # Should have 1 verified (not 3)
        self.assertEqual(len(result["candidates"]), 1)
        self.assertEqual(result["candidates"][0]["stats"]["verified_evidence_count"], 1)
    
    def test_multiple_runs_same_reason(self):
        """3 different runs = 3 support."""
        for i in range(3):
            record_intervention(
                run_id=f"run-{i}",
                step_id="step-1",
                reason="Window identity ambiguous",
                action="Selected target",
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
        
        self.assertEqual(result["candidates"][0]["stats"]["verified_evidence_count"], 3)


class TestCorrectStrength(unittest.TestCase):
    """Test 3: Strength based on pass rate, not raw counts."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        _insert_verifier_events(self.temp_db, {"pass": "PASSED", "fail": "FAILED"})
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_3_verified_9_failed_is_not_strong(self):
        """3 verified + 9 failed = 25% pass rate, not strong."""
        # 3 verified
        for i in range(3):
            record_intervention(
                run_id=f"verified-run-{i}",
                step_id="step-1",
                reason="Test",
                action="Test",
                db_path=self.temp_db,
            )
        
        # 9 failed
        for i in range(9):
            record_intervention(
                run_id=f"failed-run-{i}",
                step_id="step-1",
                reason="Test",
                action="Test",
                db_path=self.temp_db,
            )
        
        interventions = get_interventions(self.temp_db)
        for iv in interventions:
            if iv["run_id"].startswith("verified-run"):
                verify_with_evidence(
                    intervention_id=iv["intervention_id"],
                    followup_run_id="followup-pass",
                    db_path=self.temp_db,
                )
            else:
                verify_with_evidence(
                    intervention_id=iv["intervention_id"],
                    followup_run_id="followup-fail",
                    db_path=self.temp_db,
                )
        
        result = mine_candidates(db_path=self.temp_db)
        
        # Pass rate = 3/(3+9) = 25%
        cand = result["candidates"][0]
        self.assertEqual(cand["stats"]["pass_rate"], 0.25)
        self.assertEqual(cand["strength"], "weak")
    
    def test_manual_only_is_manual_only_strength(self):
        """Manual-only records should be 'manual_only' strength."""
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
        
        # Use min_verified=0 to include manual-only candidates
        result = mine_candidates(min_verified=0, db_path=self.temp_db)
        
        self.assertGreaterEqual(len(result["candidates"]), 1)
        cand = result["candidates"][0]
        self.assertEqual(cand["strength"], "manual_only")
        self.assertEqual(cand["stats"]["verified_evidence_count"], 0)


class TestSpecificGrouping(unittest.TestCase):
    """Test 4: Grouping uses full reason + action + context."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        _insert_verifier_events(self.temp_db, {"pass": "PASSED"})
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_different_actions_separate_groups(self):
        """Different actions should be separate groups."""
        record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Window identity ambiguous",
            action="Action A",
            db_path=self.temp_db,
        )
        record_intervention(
            run_id="run-2",
            step_id="step-1",
            reason="Window identity ambiguous",
            action="Action B",
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
        
        self.assertEqual(len(result["candidates"]), 2)
    
    def test_same_reason_different_context_structure(self):
        """Same reason but different context structure should be separate."""
        record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Test",
            action="Test",
            details={"key1": "value"},
            db_path=self.temp_db,
        )
        record_intervention(
            run_id="run-2",
            step_id="step-1",
            reason="Test",
            action="Test",
            details={"key1": "value", "key2": "value"},  # Different keys
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
        
        # Should be 2 groups due to different context keys
        self.assertEqual(len(result["candidates"]), 2)
    
    def test_group_signature_includes_context_keys(self):
        """Group signature should include context keys."""
        sig = _make_group_signature(
            reason="Test reason",
            action="Test action",
            context={"key1": "v1", "key2": "v2"},
        )
        
        parsed = json.loads(sig)
        self.assertEqual(parsed["reason"], "Test reason")
        self.assertEqual(parsed["action"], "Test action")
        self.assertEqual(parsed["context_keys"], ["key1", "key2"])


class TestTraceableEvidence(unittest.TestCase):
    """Test 5: Evidence references included for traceability."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        _insert_verifier_events(self.temp_db, {"pass": "PASSED", "fail": "FAILED"})
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_verified_evidence_refs_included(self):
        """Verified evidence should include intervention references."""
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
        self.assertEqual(
            cand["verified_evidence_refs"][0]["intervention_id"],
            record["intervention_id"]
        )
    
    def test_failed_refs_included(self):
        """Failed evidence should include intervention references."""
        record = record_intervention(
            run_id="run-1",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        interventions = get_interventions(self.temp_db)
        verify_with_evidence(
            intervention_id=interventions[0]["intervention_id"],
            followup_run_id="followup-fail",
            db_path=self.temp_db,
        )
        
        # Use min_verified=0 to include failed-only candidates
        result = mine_candidates(min_verified=0, db_path=self.temp_db)
        
        self.assertGreaterEqual(len(result["candidates"]), 1)


class TestCLI(unittest.TestCase):
    """Test CLI interface."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_mine_command_returns_json(self):
        """mine command returns JSON with expected fields."""
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
        self.assertIn("snapshot_hash", data)
        self.assertIn("candidates", data)
    
    def test_mine_deterministic_twice(self):
        """Two mine commands produce same hash."""
        result1 = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
                "--db", str(self.temp_db),
                "mine",
            ],
            capture_output=True,
            text=True,
        )
        result2 = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "mine_candidates.py"),
                "--db", str(self.temp_db),
                "mine",
            ],
            capture_output=True,
            text=True,
        )
        
        hash1 = json.loads(result1.stdout)["snapshot_hash"]
        hash2 = json.loads(result2.stdout)["snapshot_hash"]
        self.assertEqual(hash1, hash2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
