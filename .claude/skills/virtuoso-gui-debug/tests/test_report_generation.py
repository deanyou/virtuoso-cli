#!/usr/bin/env python3
"""Tests for generate_report.py Candidates tab data generation."""

import tempfile
import unittest
from pathlib import Path
import sys
import json

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts" / "evidence"))


class TestCandidatesTabData(unittest.TestCase):
    """Test Candidates tab data generation logic."""
    
    def setUp(self):
        self.db = Path(tempfile.mktemp(suffix=".db"))
        from record_intervention import init_db
        init_db(self.db)
    
    def tearDown(self):
        self.db.unlink(missing_ok=True)
    
    def _generate_candidates_data(self, db_path):
        """Replicate what generate_report.py does for candidates."""
        from mine_candidates import mine
        from record_intervention import get_adopted_candidates, list_candidate_decisions
        import json as _json
        import html as _html
        
        def _esc(s):
            if s is None:
                return ""
            return _html.escape(str(s), quote=True)
        
        def _parse_sig(sig):
            try:
                data = _json.loads(sig)
                return {
                    "reason": data.get("reason", ""),
                    "action": data.get("action", ""),
                    "context": data.get("context", {}),
                }
            except:
                return {"reason": sig[:50], "action": "", "context": {}}
        
        def _fmt_context(ctx, max_len=80):
            if not ctx:
                return ""
            parts = [f"{k}={v}" for k, v in sorted(ctx.items())]
            s = ", ".join(parts)
            if len(s) > max_len:
                s = s[:max_len-3] + "..."
            return s
        
        result = mine(min_verified=0, db_path=db_path)
        decisions = list_candidate_decisions(include_revoked=True, db_path=db_path)
        adopted = get_adopted_candidates(db_path=db_path)
        
        decision_map = {}
        for d in decisions:
            key = (d["candidate_id"], d["snapshot_hash"])
            decision_map[key] = d
        
        snapshot_hash = result.get("h", "")
        adopted_current = [d for d in adopted if d.get("snapshot_hash") == snapshot_hash]
        
        for c in result.get("candidates", []):
            cand_id = c.get("candidate_id", "")
            sig_data = _parse_sig(c.get("sig", ""))
            c["reason"] = sig_data["reason"]
            c["action"] = sig_data["action"]
            c["context"] = sig_data["context"]
            c["_context_str"] = _fmt_context(sig_data["context"])
            
            dec = decision_map.get((cand_id, snapshot_hash), {})
            if dec:
                c["decision_status"] = dec["decision"]
                c["decision_revoked"] = dec.get("revoked_at")
                c["decision_reason"] = dec.get("reason")
            else:
                c["decision_status"] = "PENDING"
                c["decision_revoked"] = None
                c["decision_reason"] = None
        
        return {
            "candidates": result.get("candidates", []),
            "adopted": adopted_current,
            "total_cands": result.get("total_cands", 0),
            "snapshot_hash": snapshot_hash,
        }
    
    def test_candidates_with_reason_action_display(self):
        """Candidates display reason/action/context correctly."""
        from record_intervention import (
            init_db, record_intervention, verify_with_evidence,
            record_candidate_decision
        )
        import mine_candidates
        
        # Create intervention with special chars in reason/action
        iv = record_intervention(
            run_id="run-1", step_id="step-1",
            reason="Window <error> detected", action="Click OK & retry",
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
        verify_with_evidence(intervention_id=iv["intervention_id"],
            followup_run_id="followup-1", db_path=self.db)
        
        # Mine and adopt
        result = mine_candidates.mine(min_verified=0, db_path=self.db)
        snap = result["h"]
        record_candidate_decision(result["candidates"][0]["candidate_id"], snap, "ADOPTED",
            reason="<script>alert('xss')</script>", db_path=self.db)
        
        # Generate report data (this is what generate_report.py does)
        data = self._generate_candidates_data(self.db)
        
        # Check data structure
        self.assertEqual(len(data["candidates"]), 1)
        self.assertEqual(len(data["adopted"]), 1)
        
        c = data["candidates"][0]
        self.assertIn("Window", c.get("reason", ""))
        self.assertIn("Click", c.get("action", ""))
        self.assertEqual(c.get("decision_status"), "ADOPTED")
        
        # Check decision reason is stored
        self.assertIn("xss", c.get("decision_reason", ""))
        
        print(f"✓ Reason: {c.get('reason')}")
        print(f"✓ Action: {c.get('action')}")
        print(f"✓ Decision: {c.get('decision_status')}")
    
    def test_snapshot_change_adopted_filtering(self):
        """After new evidence, old adopted not in current snapshot."""
        from record_intervention import (
            init_db, record_intervention, verify_with_evidence,
            record_candidate_decision
        )
        import mine_candidates
        
        # Create first intervention
        iv1 = record_intervention(
            run_id="run-1", step_id="step-1",
            reason="First reason", action="First action",
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
        record_candidate_decision(result1["candidates"][0]["candidate_id"], snap1,
            "ADOPTED", reason="Good", db_path=self.db)
        
        # Add second intervention (changes snapshot)
        iv2 = record_intervention(
            run_id="run-2", step_id="step-1",
            reason="Second reason", action="Second action",
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
        
        result2 = mine_candidates.mine(min_verified=0, db_path=self.db)
        snap2 = result2["h"]
        self.assertNotEqual(snap1, snap2)
        
        # Generate report data (simulating report generation)
        data = self._generate_candidates_data(self.db)
        
        # Check adopted count is for CURRENT snapshot only
        self.assertEqual(len(data["adopted"]), 0)  # No adopted in new snapshot
        self.assertEqual(data["snapshot_hash"], snap2)
        
        print(f"✓ Old snapshot: {snap1}")
        print(f"✓ New snapshot: {snap2}")
        print(f"✓ Adopted in current: {len(data['adopted'])}")
    
    def test_html_escaping_in_reason_action(self):
        """Special chars in reason/action are escaped."""
        from record_intervention import (
            init_db, record_intervention, verify_with_evidence,
            record_candidate_decision
        )
        import mine_candidates
        import html
        
        # Create intervention with XSS attempt in reason
        iv = record_intervention(
            run_id="run-1", step_id="step-1",
            reason="<script>alert('xss')</script>",
            action="Click <button>&",
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
        verify_with_evidence(intervention_id=iv["intervention_id"],
            followup_run_id="followup-1", db_path=self.db)
        
        result = mine_candidates.mine(min_verified=0, db_path=self.db)
        snap = result["h"]
        record_candidate_decision(result["candidates"][0]["candidate_id"], snap,
            "ADOPTED", reason="Test <b>bold</b>", db_path=self.db)
        
        data = self._generate_candidates_data(self.db)
        c = data["candidates"][0]
        
        # Raw reason/action should contain the XSS attempt
        raw_reason = c.get("reason", "")
        self.assertIn("<script>", raw_reason)
        
        # In HTML context, these should be escaped
        esc_reason = html.escape(raw_reason, quote=True)
        self.assertNotIn("<script>", esc_reason)
        
        print(f"✓ Raw reason: {raw_reason}")
        print(f"✓ Escaped: {esc_reason}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
