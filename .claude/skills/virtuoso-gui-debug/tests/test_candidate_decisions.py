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
)


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


if __name__ == "__main__":
    unittest.main(verbosity=2)
