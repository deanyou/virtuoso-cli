#!/usr/bin/env python3
"""Tests for P3: Candidate Decision Workflow."""

import tempfile
import unittest
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts" / "evidence"))
from record_intervention import (
    init_db,
    record_candidate_decision,
    list_candidate_decisions,
    get_adopted_candidates,
    record_intervention,
)
import mine_candidates


class TestCandidateDecisions(unittest.TestCase):
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)
    
    def tearDown(self):
        try:
            self.db.unlink()
        except (FileNotFoundError, PermissionError):
            pass
    
    def test_adopt_candidate(self):
        """ADOPTED decision is recorded and retrievable."""
        result = record_candidate_decision(
            "test-candidate", "snap1", "ADOPTED",
            reason="Good evidence", db_path=self.db
        )
        self.assertEqual(result["decision"], "ADOPTED")
        
        adopted = get_adopted_candidates(db_path=self.db)
        self.assertEqual(len(adopted), 1)
        self.assertEqual(adopted[0]["candidate_id"], "test-candidate")
    
    def test_reject_candidate(self):
        """REJECTED decision is recorded but not in adopted list."""
        result = record_candidate_decision(
            "test-candidate", "snap1", "REJECTED",
            reason="Insufficient evidence", db_path=self.db
        )
        self.assertEqual(result["decision"], "REJECTED")
        
        adopted = get_adopted_candidates(db_path=self.db)
        self.assertEqual(len(adopted), 0)
    
    def test_auto_revocation(self):
        """Adopting new version revokes previous adoption."""
        # Adopt v1
        record_candidate_decision("cand", "v1", "ADOPTED", db_path=self.db)
        # Adopt v2 (should revoke v1)
        record_candidate_decision("cand", "v2", "ADOPTED", db_path=self.db)
        
        adopted = get_adopted_candidates(db_path=self.db)
        self.assertEqual(len(adopted), 1)
        self.assertEqual(adopted[0]["snapshot_hash"], "v2")
    
    def test_list_decisions_with_filter(self):
        """List decisions can be filtered by candidate_id and decision."""
        record_candidate_decision("cand-a", "hash1", "ADOPTED", db_path=self.db)
        record_candidate_decision("cand-b", "hash2", "REJECTED", db_path=self.db)
        
        all_decisions = list_candidate_decisions(db_path=self.db)
        self.assertEqual(len(all_decisions), 2)
        
        adopted_only = list_candidate_decisions(decision="ADOPTED", db_path=self.db)
        self.assertEqual(len(adopted_only), 1)
        self.assertEqual(adopted_only[0]["candidate_id"], "cand-a")
        
        cand_a_only = list_candidate_decisions(candidate_id="cand-a", db_path=self.db)
        self.assertEqual(len(cand_a_only), 1)
    
    def test_defer_candidate(self):
        """DEFERRED decision is recorded but not adopted."""
        result = record_candidate_decision(
            "test-candidate", "snap1", "DEFERRED",
            reason="Need more evidence", db_path=self.db
        )
        self.assertEqual(result["decision"], "DEFERRED")
        
        adopted = get_adopted_candidates(db_path=self.db)
        self.assertEqual(len(adopted), 0)


class TestEndToEndPipeline(unittest.TestCase):
    """End-to-end test: mine -> adopt/reject -> review -> list-decisions."""
    
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)
    
    def tearDown(self):
        try:
            self.db.unlink()
        except (FileNotFoundError, PermissionError):
            pass
    
    def test_mine_adopt_reject_review_show_correct_status(self):
        """Real candidates -> decisions -> review shows ADOPTED/REJECTED correctly."""
        from record_intervention import verify_with_evidence, cmd_review, cmd_list_decisions
        from io import StringIO
        import contextlib
        
        # Create 2 intervention groups (2 candidates)
        for i in range(2):
            iv = record_intervention(
                run_id=f"run-{i}", step_id="step-1",
                reason=f"Test reason {i}", action=f"Test action {i}",
                db_path=self.db
            )
            conn = init_db(self.db)
            conn.execute("""
                INSERT INTO experience_events 
                (event_id, run_id, step_id, event_type, state, value_source, outcome)
                VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
            """, (f"evt-{i}", f"followup-{i}"))
            conn.commit()
            conn.close()
            verify_with_evidence(
                intervention_id=iv["intervention_id"],
                followup_run_id=f"followup-{i}",
                db_path=self.db
            )
        
        # Mine candidates
        result = mine_candidates.mine(min_verified=0, db_path=self.db)
        self.assertEqual(result["total_cands"], 2)
        
        cands = result["candidates"]
        cand_ids = [c["candidate_id"] for c in cands]
        snapshot_hash = result["h"]
        
        # Adopt first, reject second
        record_candidate_decision(cand_ids[0], snapshot_hash, "ADOPTED", db_path=self.db)
        record_candidate_decision(cand_ids[1], snapshot_hash, "REJECTED", db_path=self.db)
        
        # Test cmd_review output
        class MockArgs:
            def __init__(self):
                self.min_verified = 0
                self.db = str(self.db)
            db = None
        
        args = MockArgs()
        args.db = str(self.db)
        
        # Capture stdout
        f = StringIO()
        with contextlib.redirect_stdout(f):
            cmd_review(args)
        output = f.getvalue()
        
        # Verify ADOPTED and REJECTED appear in output
        self.assertIn("ADOPTED", output)
        self.assertIn("REJECTED", output)
        # Verify PENDING does NOT appear (both candidates have decisions)
        self.assertNotIn("PENDING", output)
        
        # Test cmd_list_decisions output
        f2 = StringIO()
        args2 = MockArgs()
        args2.db = str(self.db)
        args2.candidate_id = None
        args2.decision = None
        args2.include_revoked = False
        with contextlib.redirect_stdout(f2):
            cmd_list_decisions(args2)
        output2 = f2.getvalue()
        
        # Verify both statuses appear
        self.assertIn("ADOPTED", output2)
        self.assertIn("REJECTED", output2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
