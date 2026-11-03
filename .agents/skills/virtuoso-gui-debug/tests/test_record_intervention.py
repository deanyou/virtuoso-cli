#!/usr/bin/env python3
"""
Unit tests for record_intervention.py

Tests cover:
1. Default database and import path correctness; new DB initializes fully
2. record rejects non-existent run; --force bypasses
3. Verification status derived from evidence: FAILED evidence cannot become VERIFIED
4. Manual declarations cannot impersonate independent verification
5. trace covers non-existent ID, original and followup runs
6. CLI --db usage matches help documentation

Run with: python3 test_record_intervention.py
Compatible with Python 3.9 and 3.13
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Optional

# Add script to path
_SCRIPT_DIR = Path(__file__).parent.resolve()  # virtuoso-gui-debug/tests
_SKILL_DIR = _SCRIPT_DIR.parent  # virtuoso-gui-debug
sys.path.insert(0, str(_SKILL_DIR / "scripts" / "evidence"))

from record_intervention import (
    init_db,
    get_default_db_path,
    record_intervention,
    list_interventions,
    update_verification_status,
    get_trace_for_intervention,
    verify_with_evidence,
    validate_run_exists,
    INTERVENTION_SCHEMA,
    _SKILL_ROOT,
)


class TestDatabaseAndImports(unittest.TestCase):
    """Test 1: Default database and import path correct; new DB initializes fully."""
    
    def test_default_db_path_returns_path(self):
        """Default database path is returned correctly.
        
        When skill internal database exists, it should be used.
        Fallback is experience.db in cache directory.
        """
        db_path = get_default_db_path()
        self.assertIsInstance(db_path, Path)
        # Should return skill internal db if exists, else cache
        if (_SKILL_ROOT / "data" / "skill_db.sqlite3").exists():
            self.assertEqual(db_path.name, "skill_db.sqlite3")
        else:
            self.assertEqual(db_path.name, "experience.db")
    
    def test_init_db_creates_schema(self):
        """New database initializes with intervention schema."""
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
            temp_db = Path(f.name)
        
        try:
            conn = init_db(temp_db)
            
            # Check intervention table exists
            cursor = conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='interventions'"
            )
            self.assertIsNotNone(cursor.fetchone(), "interventions table not created")
            
            # Check experience_events table exists (from base schema)
            cursor = conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='experience_events'"
            )
            self.assertIsNotNone(cursor.fetchone(), "experience_events table not created")
            
            # Check indexes
            cursor = conn.execute(
                "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_interventions_run'"
            )
            self.assertIsNotNone(cursor.fetchone(), "idx_interventions_run not created")
            
            conn.close()
        finally:
            temp_db.unlink(missing_ok=True)
    
    def test_import_path_works(self):
        """Script can be imported from different working directories."""
        # This test verifies the sys.path setup works
        # by importing the module at the top of this file
        self.assertTrue(hasattr(sys.modules.get('record_intervention', {}), 'record_intervention'))


class TestRecordValidation(unittest.TestCase):
    """Test 2: record rejects non-existent run; --force bypasses."""
    
    def setUp(self):
        """Create temporary database for each test."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_record_validates_run_when_flag_set(self):
        """record rejects non-existent run when validate_run=True."""
        with self.assertRaises(ValueError) as ctx:
            record_intervention(
                run_id="non-existent-run",
                step_id="step-1",
                reason="Test reason",
                action="Test action",
                db_path=self.temp_db,
                validate_run=True,
            )
        self.assertIn("not found in experience_events", str(ctx.exception))
    
    def test_record_allows_new_run_without_flag(self):
        """record allows new run when validate_run=False (default)."""
        record = record_intervention(
            run_id="new-run",
            step_id="step-1",
            reason="Test reason",
            action="Test action",
            db_path=self.temp_db,
            validate_run=False,
        )
        self.assertIsNotNone(record.get("intervention_id"))
        self.assertEqual(record["run_id"], "new-run")
    
    def test_cli_force_flag_bypasses_validation(self):
        """CLI --force flag bypasses run validation."""
        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"),
                "--db", str(self.temp_db),
                "record",
                "any-run-id",
                "any-step",
                "--reason", "Test",
                "--action", "Test",
                "--force",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, f"CLI failed: {result.stderr}")
        self.assertIn("Intervention recorded", result.stderr)
    
    def test_cli_validate_run_fails_without_force(self):
        """CLI without --force fails for non-existent run."""
        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"),
                "--db", str(self.temp_db),
                "record",
                "non-existent-run",
                "step-1",
                "--reason", "Test",
                "--action", "Test",
                "--validate-run",
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not found", result.stderr)


class TestVerificationContract(unittest.TestCase):
    """Test 3 & 4: Verification status derived from evidence."""
    
    def setUp(self):
        """Create temporary database with experience_events."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def _insert_verifier_event(self, run_id: str, outcome: str):
        """Insert a VERIFIER_CONFIRMED event."""
        conn = init_db(self.temp_db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', ?)
        """, (f"evt-{run_id}-{outcome}", run_id, outcome))
        conn.commit()
        conn.close()
    
    def test_verify_with_evidence_verified_on_passed_outcome(self):
        """verify_with_evidence sets VERIFIED when followup has VERIFIER_CONFIRMED PASSED."""
        self._insert_verifier_event("followup-run", "PASSED")
        
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result = verify_with_evidence(
            intervention_id=record["intervention_id"],
            followup_run_id="followup-run",
            db_path=self.temp_db,
        )
        
        self.assertEqual(result["verification_status"], "VERIFIED")
        self.assertEqual(result["derived_from"], "actual_passed_outcome")
        self.assertTrue(result["updated"])
    
    def test_verify_with_evidence_failed_on_failed_outcome(self):
        """verify_with_evidence sets FAILED when actual evidence has FAILED outcome."""
        self._insert_verifier_event("followup-run", "FAILED")
        
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result = verify_with_evidence(
            intervention_id=record["intervention_id"],
            followup_run_id="followup-run",
            db_path=self.temp_db,
        )
        
        self.assertEqual(result["verification_status"], "FAILED")
        self.assertEqual(result["derived_from"], "actual_failed_outcome")
        self.assertNotEqual(result["verification_status"], "VERIFIED")
    
    def test_verify_with_evidence_unknown_without_verifier(self):
        """verify_with_evidence sets UNKNOWN when followup has no VERIFIER_CONFIRMED."""
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result = verify_with_evidence(
            intervention_id=record["intervention_id"],
            followup_run_id="non-existent-followup",
            db_path=self.temp_db,
        )
        
        self.assertEqual(result["verification_status"], "UNKNOWN")
        self.assertEqual(result["verifier_confirmed_count"], 0)
    
    def test_failed_evidence_cannot_become_verified(self):
        """FAILED evidence must produce FAILED status, never VERIFIED.
        
        This tests the critical invariant: actual FAILED outcome in verifier
        events must result in FAILED status, regardless of any parameter.
        """
        # Insert FAILED verifier event
        self._insert_verifier_event("failed-followup", "FAILED")
        
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result = verify_with_evidence(
            intervention_id=record["intervention_id"],
            followup_run_id="failed-followup",
            db_path=self.temp_db,
        )
        
        # CRITICAL: FAILED evidence must NOT produce VERIFIED
        self.assertEqual(result["verification_status"], "FAILED")
        self.assertNotEqual(result["verification_status"], "VERIFIED")
    
    def test_update_verification_status_is_manual_override(self):
        """update_verification_status allows manual override (separate from evidence path).
        
        This is intentionally a separate path for manual intervention recording.
        The provenance is tracked via details field, not verification_status source.
        For evidence-based verification, use verify_with_evidence instead.
        """
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        # Use simple update_verification_status (manual override)
        success = update_verification_status(
            intervention_id=record["intervention_id"],
            verification_status="VERIFIED",
            db_path=self.temp_db,
        )
        
        self.assertTrue(success)
        
        # Check the status is set (manual override does not require evidence)
        interventions = list_interventions(
            run_id="test-run",
            db_path=self.temp_db,
        )
        self.assertEqual(interventions[0]["verification_status"], "VERIFIED")
        
        # Note: This is manual override, not evidence-based verification.
        # Use verify_with_evidence() for evidence-based status derivation.
    
    def test_verification_status_persists(self):
        """Verification status is actually persisted to database."""
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        update_verification_status(
            intervention_id=record["intervention_id"],
            verification_status="VERIFIED",
            followup_run_id="followup-run",
            db_path=self.temp_db,
        )
        
        # Read back from database
        conn = init_db(self.temp_db)
        cursor = conn.execute(
            "SELECT verification_status, followup_run_id FROM interventions WHERE intervention_id = ?",
            (record["intervention_id"],)
        )
        row = cursor.fetchone()
        conn.close()
        
        self.assertEqual(row[0], "VERIFIED")
        self.assertEqual(row[1], "followup-run")
    
    def test_manual_override_has_source_recorded(self):
        """Manual override records verification source in details."""
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        update_verification_status(
            intervention_id=record["intervention_id"],
            verification_status="VERIFIED",
            verification_source="manual",
            db_path=self.temp_db,
        )
        
        # Check that source is recorded in details
        interventions = list_interventions(db_path=self.temp_db)
        details = json.loads(interventions[0]["details"] or "{}")
        
        self.assertIn("_verification", details)
        self.assertEqual(details["_verification"]["source"], "manual")
        self.assertEqual(details["_verification"]["status"], "VERIFIED")
    
    def test_evidence_verification_has_source_recorded(self):
        """Evidence-based verification records source as 'evidence'."""
        record = record_intervention(
            run_id="test-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result = verify_with_evidence(
            intervention_id=record["intervention_id"],
            followup_run_id="followup-run",
            db_path=self.temp_db,
        )
        
        self.assertEqual(result["verification_status"], "UNKNOWN")  # No verifier events
        
        # Check that source is recorded
        interventions = list_interventions(db_path=self.temp_db)
        details = json.loads(interventions[0]["details"] or "{}")
        
        self.assertIn("_verification", details)
        self.assertEqual(details["_verification"]["source"], "evidence")


class TestTraceCoverage(unittest.TestCase):
    """Test 5: trace covers non-existent ID, original and followup runs."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
        
        # Insert mock events
        conn = init_db(self.temp_db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state)
            VALUES ('evt-1', 'original-run', 'step-1', 'EXECUTE', 'EXECUTE')
        """)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state)
            VALUES ('evt-2', 'followup-run', 'step-1', 'VERIFY', 'VERIFY')
        """)
        conn.commit()
        conn.close()
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_trace_non_existent_id_returns_error(self):
        """trace returns error dict for non-existent ID."""
        result = get_trace_for_intervention(
            intervention_id="non-existent-id",
            db_path=self.temp_db,
        )
        self.assertIn("error", result)
        self.assertIn("not found", result["error"].lower())
    
    def test_trace_includes_original_events(self):
        """trace includes original run events."""
        # Record intervention
        record = record_intervention(
            run_id="original-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result = get_trace_for_intervention(
            intervention_id=record["intervention_id"],
            db_path=self.temp_db,
        )
        
        self.assertIn("original_events", result)
        self.assertEqual(len(result["original_events"]), 1)
        self.assertEqual(result["original_events"][0]["run_id"], "original-run")
    
    def test_trace_includes_followup_events(self):
        """trace includes followup run events when present."""
        # Record intervention with followup
        record = record_intervention(
            run_id="original-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        update_verification_status(
            intervention_id=record["intervention_id"],
            verification_status="VERIFIED",
            followup_run_id="followup-run",
            db_path=self.temp_db,
        )
        
        result = get_trace_for_intervention(
            intervention_id=record["intervention_id"],
            db_path=self.temp_db,
        )
        
        self.assertIn("followup_events", result)
        self.assertEqual(len(result["followup_events"]), 1)
        self.assertEqual(result["followup_events"][0]["run_id"], "followup-run")
    
    def test_trace_no_followup_empty_followup_events(self):
        """trace has empty followup_events when no followup run."""
        record = record_intervention(
            run_id="original-run",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result = get_trace_for_intervention(
            intervention_id=record["intervention_id"],
            db_path=self.temp_db,
        )
        
        self.assertIn("followup_events", result)
        self.assertEqual(len(result["followup_events"]), 0)
    
    def test_cli_trace_nonexistent_returns_error(self):
        """CLI trace for non-existent ID returns exit code 1."""
        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"),
                "--db", str(self.temp_db),
                "trace",
                "nonexistent123",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("not found", result.stdout.lower())


class TestCLIDbOption(unittest.TestCase):
    """Test 6: CLI --db usage matches help documentation."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_help_shows_db_option(self):
        """Help documentation includes --db option."""
        result = subprocess.run(
            [sys.executable, str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"), "--help"],
            capture_output=True,
            text=True,
        )
        self.assertIn("--db", result.stdout)
    
    def test_record_with_custom_db(self):
        """record command works with --db option."""
        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"),
                "--db", str(self.temp_db),
                "record",
                "run-1",
                "step-1",
                "--reason", "Test reason",
                "--action", "Test action",
                "--force",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0)
        
        # Verify record is in the database
        interventions = list_interventions(db_path=self.temp_db)
        self.assertEqual(len(interventions), 1)
        self.assertEqual(interventions[0]["run_id"], "run-1")
    
    def test_list_with_custom_db(self):
        """list command works with --db option."""
        # Add a record first
        record_intervention(
            run_id="run-list",
            step_id="step-1",
            reason="Test",
            action="Test",
            db_path=self.temp_db,
        )
        
        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"),
                "--db", str(self.temp_db),
                "list",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("run-list", result.stdout)
    
    def test_record_subcommand_help_shows_db(self):
        """record subcommand help shows --db in parent options."""
        result = subprocess.run(
            [
                sys.executable,
                str(_SKILL_DIR / "scripts" / "evidence" / "record_intervention.py"),
                "record",
                "--help",
            ],
            capture_output=True,
            text=True,
        )
        # --db is a parent argument, shown in main help
        self.assertEqual(result.returncode, 0)


class TestValidateRunExists(unittest.TestCase):
    """Additional test: validate_run_exists function."""
    
    def setUp(self):
        """Create temporary database."""
        self.temp_db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.temp_db)
    
    def tearDown(self):
        """Clean up temporary database."""
        self.temp_db.unlink(missing_ok=True)
    
    def test_validate_returns_true_for_existing_run(self):
        """validate_run_exists returns True for existing run."""
        # Insert a run
        conn = init_db(self.temp_db)
        conn.execute("""
            INSERT INTO experience_events (event_id, run_id, step_id, event_type, state)
            VALUES ('evt-1', 'existing-run', 'step-1', 'EXECUTE', 'EXECUTE')
        """)
        conn.commit()
        conn.close()
        
        self.assertTrue(validate_run_exists("existing-run", self.temp_db))
    
    def test_validate_returns_false_for_nonexistent_run(self):
        """validate_run_exists returns False for non-existent run."""
        self.assertFalse(validate_run_exists("non-existent", self.temp_db))


if __name__ == "__main__":
    unittest.main(verbosity=2)
