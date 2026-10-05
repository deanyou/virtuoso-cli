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
        self.db.unlink(missing_ok=True)
    
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
    """End-to-end test: mine -> adopt/reject -> review."""
    
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        init_db(self.db)
    
    def tearDown(self):
        self.db.unlink(missing_ok=True)
    
    def test_mine_adopt_reject_review_pipeline(self):
        """Real candidates mined -> decisions recorded -> review shows correct status."""
        from record_intervention import verify_with_evidence
        
        # Create 2 intervention groups (2 candidates)
        for i in range(2):
            iv = record_intervention(
                run_id=f"run-{i}", step_id="step-1",
                reason=f"Test reason {i}", action=f"Test action {i}",
                db_path=self.db
            )
            # Verify both
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
        
        # Get candidate IDs from each candidate
        cands = result["candidates"]
        self.assertEqual(len(cands), 2)
        
        # Each candidate has its own sig/candidate_id
        cand_ids = [c.get("candidate_id") or c.get("sig", "")[:28] for c in cands]
        snapshot_hash = result.get("h", "")
        
        self.assertIsNotNone(snapshot_hash)
        for cid in cand_ids:
            self.assertIsNotNone(cid)
        
        # Adopt first candidate
        r1 = record_candidate_decision(
            cand_ids[0], snapshot_hash, "ADOPTED",
            reason="Good evidence", db_path=self.db
        )
        self.assertEqual(r1["decision"], "ADOPTED")
        
        # Reject second candidate
        r2 = record_candidate_decision(
            cand_ids[1], snapshot_hash, "REJECTED",
            reason="Insufficient", db_path=self.db
        )
        self.assertEqual(r2["decision"], "REJECTED")
        
        # Verify decisions
        decisions = list_candidate_decisions(include_revoked=True, db_path=self.db)
        self.assertEqual(len(decisions), 2)
        
        # Check adopted list has only one
        adopted = get_adopted_candidates(db_path=self.db)
        self.assertEqual(len(adopted), 1)
        self.assertEqual(adopted[0]["decision"], "ADOPTED")
        
        # Verify statuses
        for d in decisions:
            if d["decision"] == "ADOPTED":
                self.assertEqual(d["candidate_id"], cand_ids[0])
            else:
                self.assertEqual(d["candidate_id"], cand_ids[1])
                self.assertEqual(d["decision"], "REJECTED")


if __name__ == "__main__":
    unittest.main(verbosity=2)
