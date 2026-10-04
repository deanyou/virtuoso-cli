#!/usr/bin/env python3
"""
record_intervention.py — Record manual interventions for experience learning.

Usage:
    record_intervention.py record <run_id> <step_id> --reason <reason> --action <action>
    record_intervention.py list [--run-id <run_id>]
    record_intervention.py verify <intervention_id>

This module records human corrections that happen during GUI automation.
Interventions are stored in the experience DB with provenance tracking.

P1 of the Evidence Loop Plan:
https://github.com/deanyou/virtuoso-cli/blob/main/docs/vcli-evidence-loop-plan.html
"""

import argparse
import json
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

# ── Schema Extensions for Interventions ─────────────────────────────────────

INTERVENTION_SCHEMA = """
CREATE TABLE IF NOT EXISTS interventions (
    intervention_id  TEXT PRIMARY KEY,
    run_id           TEXT NOT NULL,
    step_id          TEXT,
    attempt          INTEGER DEFAULT 0,
    timestamp        TEXT NOT NULL,
    reason           TEXT NOT NULL,
    action           TEXT NOT NULL,
    source           TEXT DEFAULT 'human',
    followup_run_id  TEXT,
    verification_status TEXT,
    evidence_before  TEXT,
    evidence_after   TEXT,
    details          TEXT,
    superseded_by    TEXT,
    created_at       TEXT DEFAULT (datetime('now')),
    
    FOREIGN KEY (run_id) REFERENCES experience_events(run_id),
    FOREIGN KEY (step_id) REFERENCES experience_events(step_id)
);

CREATE INDEX IF NOT EXISTS idx_interventions_run ON interventions(run_id);
CREATE INDEX IF NOT EXISTS idx_interventions_step ON interventions(step_id);
CREATE INDEX IF NOT EXISTS idx_interventions_status ON interventions(verification_status);
"""


def get_db_path() -> Path:
    """Get path to experience database."""
    cache_dir = Path.home() / ".cache" / "virtuoso_bridge"
    return cache_dir / "experience.db"


def init_db(db_path: Optional[Path] = None) -> sqlite3.Connection:
    """Initialize database with intervention schema."""
    db_path = db_path or get_db_path()
    db_path.parent.mkdir(parents=True, exist_ok=True)
    
    conn = sqlite3.connect(db_path)
    
    # Initialize base experience schema (from experience.py)
    from vgui_runner.experience import init_schema
    init_schema(conn)
    
    # Add intervention table
    conn.executescript(INTERVENTION_SCHEMA)
    conn.commit()
    
    return conn


def _generate_id(run_id: str, step_id: str, timestamp: str) -> str:
    """Generate unique intervention ID."""
    import hashlib
    h = hashlib.sha256()
    h.update(f"{run_id}:{step_id}:{timestamp}".encode())
    return h.hexdigest()[:32]


def record_intervention(
    run_id: str,
    step_id: Optional[str],
    reason: str,
    action: str,
    attempt: int = 0,
    source: str = "human",
    evidence_before: Optional[str] = None,
    evidence_after: Optional[str] = None,
    details: Optional[Dict[str, Any]] = None,
    db_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """
    Record a manual intervention.
    
    Args:
        run_id: Original run ID that was blocked
        step_id: Step that was blocked
        reason: Why human intervention was needed
        action: What human did to resolve
        attempt: Attempt number
        source: Source of intervention (human, system, script)
        evidence_before: Evidence before intervention (e.g., screenshot path)
        evidence_after: Evidence after intervention
        details: Additional context
    
    Returns:
        Intervention record with generated ID
    """
    conn = init_db(db_path)
    
    timestamp = datetime.now(timezone.utc).isoformat()
    intervention_id = _generate_id(run_id, step_id or "", timestamp)
    
    record = {
        "intervention_id": intervention_id,
        "run_id": run_id,
        "step_id": step_id,
        "attempt": attempt,
        "timestamp": timestamp,
        "reason": reason,
        "action": action,
        "source": source,
        "followup_run_id": None,
        "verification_status": None,
        "evidence_before": evidence_before,
        "evidence_after": evidence_after,
        "details": json.dumps(details) if details else None,
        "superseded_by": None,
    }
    
    conn.execute("""
        INSERT INTO interventions (
            intervention_id, run_id, step_id, attempt, timestamp,
            reason, action, source, followup_run_id, verification_status,
            evidence_before, evidence_after, details, superseded_by
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    """, (
        record["intervention_id"],
        record["run_id"],
        record["step_id"],
        record["attempt"],
        record["timestamp"],
        record["reason"],
        record["action"],
        record["source"],
        record["followup_run_id"],
        record["verification_status"],
        record["evidence_before"],
        record["evidence_after"],
        record["details"],
        record["superseded_by"],
    ))
    conn.commit()
    conn.close()
    
    return record


def list_interventions(
    run_id: Optional[str] = None,
    verification_status: Optional[str] = None,
    db_path: Optional[Path] = None,
) -> List[Dict[str, Any]]:
    """List interventions, optionally filtered."""
    conn = init_db(db_path)
    
    query = "SELECT * FROM interventions WHERE 1=1"
    params = []
    
    if run_id:
        query += " AND run_id = ?"
        params.append(run_id)
    
    if verification_status:
        query += " AND verification_status = ?"
        params.append(verification_status)
    
    query += " ORDER BY timestamp DESC"
    
    cursor = conn.execute(query, params)
    columns = [desc[0] for desc in cursor.description]
    results = [dict(zip(columns, row)) for row in cursor]
    conn.close()
    
    return results


def update_verification_status(
    intervention_id: str,
    verification_status: str,
    followup_run_id: Optional[str] = None,
    db_path: Optional[Path] = None,
) -> bool:
    """Update verification status of an intervention."""
    conn = init_db(db_path)
    
    conn.execute("""
        UPDATE interventions
        SET verification_status = ?, followup_run_id = ?
        WHERE intervention_id = ?
    """, (verification_status, followup_run_id, intervention_id))
    
    rows = conn.rowcount
    conn.commit()
    conn.close()
    
    return rows > 0


def link_to_case(
    intervention_id: str,
    case_id: str,
    db_path: Optional[Path] = None,
) -> bool:
    """Link intervention to an experience case."""
    conn = init_db(db_path)
    
    # Get current details
    cursor = conn.execute(
        "SELECT details FROM interventions WHERE intervention_id = ?",
        (intervention_id,)
    )
    row = cursor.fetchone()
    if not row:
        conn.close()
        return False
    
    details = json.loads(row[0] or "{}")
    details["linked_case_id"] = case_id
    
    conn.execute("""
        UPDATE interventions SET details = ? WHERE intervention_id = ?
    """, (json.dumps(details), intervention_id))
    
    conn.commit()
    conn.close()
    return True


def get_trace_for_intervention(
    intervention_id: str,
    db_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """
    Get full trace context for an intervention.
    
    Returns:
        Dict with intervention, original events, and related cases
    """
    conn = init_db(db_path)
    
    # Get intervention
    cursor = conn.execute(
        "SELECT * FROM interventions WHERE intervention_id = ?",
        (intervention_id,)
    )
    columns = [desc[0] for desc in cursor.description]
    intervention = dict(zip(columns, cursor.fetchone()))
    
    if not intervention:
        conn.close()
        return {}
    
    # Get original run events
    cursor = conn.execute(
        "SELECT * FROM experience_events WHERE run_id = ? ORDER BY timestamp",
        (intervention["run_id"],)
    )
    columns = [desc[0] for desc in cursor.description]
    events = [dict(zip(columns, row)) for row in cursor]
    
    # Get related cases
    cursor = conn.execute(
        "SELECT * FROM experience_cases WHERE run_id = ?",
        (intervention["run_id"],)
    )
    columns = [desc[0] for desc in cursor.description]
    cases = [dict(zip(columns, row)) for row in cursor]
    
    conn.close()
    
    return {
        "intervention": intervention,
        "original_events": events,
        "related_cases": cases,
    }


# ── CLI ─────────────────────────────────────────────────────────────────────

def cmd_record(args: argparse.Namespace) -> int:
    """Handle 'record' command."""
    record = record_intervention(
        run_id=args.run_id,
        step_id=args.step_id,
        reason=args.reason,
        action=args.action,
        attempt=args.attempt or 0,
        source=args.source or "human",
        evidence_before=args.evidence_before,
        evidence_after=args.evidence_after,
        details={"cli": True} if args.details else None,
    )
    
    print(json.dumps(record, indent=2))
    print(f"\n✓ Intervention recorded: {record['intervention_id']}", file=sys.stderr)
    return 0


def cmd_list(args: argparse.Namespace) -> int:
    """Handle 'list' command."""
    interventions = list_interventions(
        run_id=args.run_id,
        verification_status=args.status,
    )
    
    if not interventions:
        print("No interventions found.")
        return 0
    
    for iv in interventions:
        print(f"[{iv['intervention_id'][:8]}] {iv['timestamp']}")
        print(f"  Run: {iv['run_id']} | Step: {iv['step_id'] or 'N/A'}")
        print(f"  Reason: {iv['reason']}")
        print(f"  Action: {iv['action']}")
        print(f"  Status: {iv['verification_status'] or 'pending'}")
        if iv['followup_run_id']:
            print(f"  Followup: {iv['followup_run_id']}")
        print()
    
    return 0


def cmd_verify(args: argparse.Namespace) -> int:
    """Handle 'verify' command."""
    success = update_verification_status(
        intervention_id=args.intervention_id,
        verification_status=args.status,
        followup_run_id=args.followup_run,
    )
    
    if success:
        print(f"✓ Updated verification status to '{args.status}'")
        return 0
    else:
        print(f"✗ Intervention not found: {args.intervention_id}")
        return 1


def cmd_trace(args: argparse.Namespace) -> int:
    """Handle 'trace' command."""
    trace = get_trace_for_intervention(args.intervention_id)
    
    if not trace:
        print(f"✗ Intervention not found: {args.intervention_id}")
        return 1
    
    print(json.dumps(trace, indent=2, default=str))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Record and manage manual interventions for experience learning.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Record a manual intervention
  record_intervention.py record run-123 step-456 \\
      --reason "Window identity ambiguous" \\
      --action "Selected target window manually"

  # List all interventions for a run
  record_intervention.py list --run-id run-123

  # Verify an intervention with followup run
  record_intervention.py verify abc123def456 \\
      --status VERIFIED \\
      --followup-run run-789

  # Get full trace for an intervention
  record_intervention.py trace abc123def456
        """
    )
    
    subparsers = parser.add_subparsers(dest="command", required=True)
    
    # record
    p_record = subparsers.add_parser("record", help="Record a new intervention")
    p_record.add_argument("run_id", help="Original run ID")
    p_record.add_argument("step_id", nargs="?", help="Blocked step ID")
    p_record.add_argument("--reason", "-r", required=True, help="Why intervention was needed")
    p_record.add_argument("--action", "-a", required=True, help="What human did")
    p_record.add_argument("--attempt", type=int, default=0, help="Attempt number")
    p_record.add_argument("--source", default="human", help="Source (human/system/script)")
    p_record.add_argument("--evidence-before", help="Evidence before (screenshot path)")
    p_record.add_argument("--evidence-after", help="Evidence after (screenshot path)")
    p_record.add_argument("--details", action="store_true", help="Include additional details")
    p_record.set_defaults(func=cmd_record)
    
    # list
    p_list = subparsers.add_parser("list", help="List interventions")
    p_list.add_argument("--run-id", help="Filter by run ID")
    p_list.add_argument("--status", help="Filter by verification status")
    p_list.set_defaults(func=cmd_list)
    
    # verify
    p_verify = subparsers.add_parser("verify", help="Update verification status")
    p_verify.add_argument("intervention_id", help="Intervention ID")
    p_verify.add_argument("--status", "-s", required=True,
                         choices=["VERIFIED", "FAILED", "UNKNOWN"],
                         help="Verification status")
    p_verify.add_argument("--followup-run", help="Followup run ID")
    p_verify.set_defaults(func=cmd_verify)
    
    # trace
    p_trace = subparsers.add_parser("trace", help="Get full trace for intervention")
    p_trace.add_argument("intervention_id", help="Intervention ID")
    p_trace.set_defaults(func=cmd_trace)
    
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
