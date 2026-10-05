#!/usr/bin/env python3
"""Tests for Candidates tab data generation."""

import tempfile
import unittest
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts" / "evidence"))


class TestCandidatesData(unittest.TestCase):
    """Test the data generation logic for Candidates tab."""
    
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        from record_intervention import init_db
        init_db(self.db)
    
    def tearDown(self):
        self.db.unlink(missing_ok=True)
    
    def test_candidates_with_decisions(self):
        """Non-empty candidates with decisions work correctly."""
        from record_intervention import (
            init_db, record_intervention, verify_with_evidence,
            record_candidate_decision, get_adopted_candidates, list_candidate_decisions
        )
        import mine_candidates
        
        # Create 2 interventions and verify
        for i in range(2):
            iv = record_intervention(
                run_id=f"run-{i}", step_id="step-1",
                reason=f"Reason {i}", action=f"Action {i}",
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
        snap = result["h"]
        
        # Adopt first, reject second
        record_candidate_decision(cands[0]["candidate_id"], snap, "ADOPTED",
            reason="Good evidence", db_path=self.db)
        record_candidate_decision(cands[1]["candidate_id"], snap, "REJECTED",
            reason="Not enough", db_path=self.db)
        
        # Simulate what generate_report does
        decisions = list_candidate_decisions(include_revoked=True, db_path=self.db)
        adopted = get_adopted_candidates(db_path=self.db)
        
        # Build decision map
        decision_map = {}
        for d in decisions:
            key = (d["candidate_id"], d["snapshot_hash"])
            decision_map[key] = d
        
        # Attach decision status to each candidate
        for c in cands:
            cand_id = c.get("candidate_id", "")
            dec = decision_map.get((cand_id, snap), {})
            if dec:
                c["decision_status"] = dec["decision"]
                c["decision_revoked"] = dec.get("revoked_at")
                c["decision_reason"] = dec.get("reason")
            else:
                c["decision_status"] = "PENDING"
                c["decision_revoked"] = None
                c["decision_reason"] = None
        
        # Filter adopted to current snapshot
        adopted_current = [d for d in adopted if d.get("snapshot_hash") == snap]
        
        # Assertions
        self.assertEqual(len(cands), 2)
        self.assertEqual(len(adopted_current), 1)
        
        # Check each candidate has decision_status
        statuses = [c.get("decision_status") for c in cands]
        self.assertIn("ADOPTED", statuses)
        self.assertIn("REJECTED", statuses)
        
        print(f"✓ {len(cands)} candidates with decisions")
        print(f"✓ Adopted (current snapshot): {len(adopted_current)}")
    
    def test_snapshot_change_invalidates_old_adopted(self):
        """After new evidence, old adopted decisions don't count for new snapshot."""
        from record_intervention import (
            init_db, record_intervention, verify_with_evidence,
            record_candidate_decision, get_adopted_candidates
        )
        import mine_candidates
        
        # Create first intervention
        iv1 = record_intervention(
            run_id="run-1", step_id="step-1",
            reason="Reason 1", action="Action 1",
            db_path=self.db
        )
        conn = init_db(self.db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """, ("evt-1", "followup-1"))
        conn.commit()
        conn.close()
        verify_with_evidence(intervention_id=iv1["intervention_id"],
            followup_run_id="followup-1", db_path=self.db)
        
        result1 = mine_candidates.mine(min_verified=0, db_path=self.db)
        snap1 = result1["h"]
        
        # Adopt in snapshot 1
        record_candidate_decision(result1["candidates"][0]["candidate_id"], snap1,
            "ADOPTED", reason="Good", db_path=self.db)
        
        # Add second intervention (changes snapshot)
        iv2 = record_intervention(
            run_id="run-2", step_id="step-1",
            reason="Reason 2", action="Action 2",
            db_path=self.db
        )
        conn = init_db(self.db)
        conn.execute("""
            INSERT INTO experience_events 
            (event_id, run_id, step_id, event_type, state, value_source, outcome)
            VALUES (?, ?, 'step-1', 'VERIFY', 'VERIFY', 'VERIFIER_CONFIRMED', 'PASSED')
        """, ("evt-2", "followup-2"))
        conn.commit()
        conn.close()
        verify_with_evidence(intervention_id=iv2["intervention_id"],
            followup_run_id="followup-2", db_path=self.db)
        
        # New snapshot
        result2 = mine_candidates.mine(min_verified=0, db_path=self.db)
        snap2 = result2["h"]
        self.assertNotEqual(snap1, snap2)
        
        # Get adopted (all)
        all_adopted = get_adopted_candidates(db_path=self.db)
        
        # Filter to current snapshot
        adopted_current = [d for d in all_adopted if d.get("snapshot_hash") == snap2]
        
        # In new snapshot, no adopted yet
        self.assertEqual(len(adopted_current), 0)
        
        print(f"✓ Old snapshot: {snap1}")
        print(f"✓ New snapshot: {snap2}")
        print(f"✓ Old adopted: {len(all_adopted)}, Current adopted: {len(adopted_current)}")
    
    def test_html_escape(self):
        """HTML special characters are escaped."""
        import html
        
        def _esc(s):
            if s is None:
                return ""
            return html.escape(str(s), quote=True)
        
        test_cases = [
            ("<script>", "&lt;script&gt;"),
            ("'test'", "&#x27;test&#x27;"),
            ('"test"', "&quot;test&quot;"),
            ("plain", "plain"),
            (None, ""),
        ]
        
        for input_val, expected in test_cases:
            result = _esc(input_val)
            self.assertEqual(result, expected, f"Failed for {input_val!r}")
        
        print("✓ HTML escaping works correctly")


if __name__ == "__main__":
    unittest.main(verbosity=2)
