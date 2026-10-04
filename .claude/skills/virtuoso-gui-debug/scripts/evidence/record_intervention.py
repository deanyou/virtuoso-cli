#!/usr/bin/env python3
"""
record_intervention.py — Record manual interventions for experience learning.

Usage:
    record_intervention.py record <run_id> <step_id> --reason <reason> --action <action>
    record_intervention.py list [--run-id <run_id>]
    record_intervention.py verify <intervention_id>
    record_intervention.py trace <intervention_id>

This module records human corrections that happen during GUI automation.
Interventions are stored in the experience DB with provenance tracking.

P1 of the Evidence Loop Plan:
https://github.com/deanyou/virtuoso-cli/blob/main/docs/vcli-evidence-loop-plan.html
"""

import argparse
import json
import os
import sys
import sqlite3
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

# ── Skill root detection ─────────────────────────────────────────────────────────────
# Detect skill root by finding the directory containing vgui_runner/
_SCRIPT_DIR = Path(__file__).parent.resolve()  # scripts/evidence/

# Navigate to skill root (virtuoso-gui-debug/)
# _SCRIPT_DIR is scripts/evidence/, so:
#   parent = scripts/
#   parent.parent = virtuoso-gui-debug/
_SKILL_ROOT = _SCRIPT_DIR.parent.parent

# vgui_runner directory for imports
_VGUIDIR = _SKILL_ROOT / "vgui_runner"

if str(_VGUIDIR) not in sys.path:
    sys.path.insert(0, str(_VGUIDIR))


# ── Base Schema for experience_events (fallback if import fails) ──────────────────────────

BASE_SCHEMA = """
CREATE TABLE IF NOT EXISTS experience_events (
    event_id      TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL,
    task_id       TEXT,
    step_id       TEXT,
    attempt       INTEGER DEFAULT 0,
    event_type    TEXT NOT NULL,
    timestamp     TEXT,
    state         TEXT,
    outcome       TEXT,
    channel       TEXT,
    risk_class    TEXT,
    failure_type  TEXT,
    action        TEXT,
    duration_ms   INTEGER,
    value_source  TEXT DEFAULT 'OBSERVED',
    evidence_refs TEXT,
    details       TEXT
);

CREATE TABLE IF NOT EXISTS experience_cases (
    case_id            TEXT PRIMARY KEY,
    run_id             TEXT NOT NULL,
    task_id            TEXT,
    case_type          TEXT NOT NULL,
    failure_type       TEXT,
    problem_signature  TEXT,
    initial_state      TEXT,
    action_sequence    TEXT,
    final_state        TEXT,
    outcome            TEXT,
    verification_status TEXT,
    quality_score      REAL DEFAULT 0.0,
    event_refs         TEXT,
    provenance         TEXT,
    created_at         TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_events_run ON experience_events(run_id);
CREATE INDEX IF NOT EXISTS idx_events_step ON experience_events(step_id);
CREATE INDEX IF NOT EXISTS idx_events_failure ON experience_events(failure_type);
CREATE INDEX IF NOT EXISTS idx_cases_type ON experience_cases(case_type);
CREATE INDEX IF NOT EXISTS idx_cases_failure ON experience_cases(failure_type);
"""

# ── Schema Extensions for Interventions ────────────────────────────────────────

INTERVENTION_SCHEMA = """
CREATE TABLE IF NOT EXISTS interventions (
    intervention_id     TEXT PRIMARY KEY,
    run_id             TEXT NOT NULL,
    step_id            TEXT,
    attempt            INTEGER DEFAULT 0,
    timestamp          TEXT NOT NULL,
    reason             TEXT NOT NULL,
    action             TEXT NOT NULL,
    source             TEXT DEFAULT 'human',
    followup_run_id    TEXT,
    verification_status TEXT,
    evidence_before    TEXT,
    evidence_after     TEXT,
    details            TEXT,
    superseded_by      TEXT,
    created_at         TEXT DEFAULT (datetime('now')),
    
    FOREIGN KEY (run_id) REFERENCES experience_events(run_id),
    FOREIGN KEY (step_id) REFERENCES experience_events(step_id)
);

CREATE INDEX IF NOT EXISTS idx_interventions_run ON interventions(run_id);
CREATE INDEX IF NOT EXISTS idx_interventions_step ON interventions(step_id);
CREATE INDEX IF NOT EXISTS idx_interventions_status ON interventions(verification_status);
"""


def get_default_db_path() -> Path:
    """
    Get default database path.
    
    Priority:
    1. VB_EXPERIENCE_DB env var
    2. Skill internal db: <skill>/data/skill_db.sqlite3
    3. Cache db: ~/.cache/virtuoso_bridge/experience.db
    """
    # Check environment variable
    env_path = os.environ.get("VB_EXPERIENCE_DB")
    if env_path:
        return Path(env_path)
    
    # Check skill internal path
    skill_db = _SKILL_ROOT / "data" / "skill_db.sqlite3"
    if skill_db.exists():
        return skill_db
    
    # Fallback to cache
    cache_dir = Path.home() / ".cache" / "virtuoso_bridge"
    return cache_dir / "experience.db"


def init_db(db_path: Optional[Path] = None) -> sqlite3.Connection:
    """Initialize database with intervention schema."""
    db_path = db_path or get_default_db_path()
    db_path.parent.mkdir(parents=True, exist_ok=True)
    
    conn = sqlite3.connect(db_path)
    
    # Initialize base experience schema
    try:
        from vgui_runner.experience import init_schema as exp_init_schema
        exp_init_schema(conn)
    except ImportError:
        # Fallback: use local base schema
        conn.executescript(BASE_SCHEMA)
    
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


def validate_run_exists(run_id: str, db_path: Optional[Path] = None) -> bool:
    """Check if run_id exists in experience_events."""
    try:
        conn = init_db(db_path)
        cursor = conn.execute(
            "SELECT 1 FROM experience_events WHERE run_id = ? LIMIT 1",
            (run_id,)
        )
        exists = cursor.fetchone() is not None
        conn.close()
        return exists
    except sqlite3.OperationalError:
        return False  # Table doesn't exist yet


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
    validate_run: bool = False,
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
        db_path: Optional explicit database path
        validate_run: If True, require run_id to exist in experience_events
    
    Returns:
        Intervention record with generated ID
    
    Raises:
        ValueError: If validate_run=True but run_id doesn't exist
    """
    # Validate run exists if required
    if validate_run and not validate_run_exists(run_id, db_path):
        raise ValueError(f"run_id '{run_id}' not found in experience_events. "
                        "Use --force to skip validation.")
    
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
    verification_source: str = "manual",
    db_path: Optional[Path] = None,
) -> bool:
    """
    Update verification status of an intervention.
    
    Args:
        intervention_id: Intervention to update
        verification_status: New status (VERIFIED, FAILED, UNKNOWN)
        followup_run_id: Optional followup run ID
        verification_source: How status was determined ("evidence", "manual", "override")
        db_path: Database path
    
    Returns:
        True if updated, False if intervention not found
    
    Note:
        Manual/override sources are marked and should be excluded from
        trusted experience pool. Only "evidence" source is trusted.
    """
    conn = init_db(db_path)
    
    # Get existing details to preserve provenance
    cursor = conn.execute(
        "SELECT details FROM interventions WHERE intervention_id = ?",
        (intervention_id,)
    )
    row = cursor.fetchone()
    if not row:
        conn.close()
        return False
    
    existing_details = json.loads(row[0] or "{}")
    
    # Add verification provenance
    existing_details["_verification"] = {
        "source": verification_source,
        "status": verification_status,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "followup_run_id": followup_run_id,
    }
    
    cursor = conn.execute("""
        UPDATE interventions
        SET verification_status = ?, followup_run_id = ?, details = ?
        WHERE intervention_id = ?
    """, (verification_status, followup_run_id, json.dumps(existing_details), intervention_id))
    
    conn.commit()
    rows_updated = cursor.rowcount
    conn.close()
    
    return rows_updated > 0


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
        Dict with intervention, original events, followup events, and related cases
        Empty dict if intervention not found
    """
    conn = init_db(db_path)
    
    # Get intervention
    cursor = conn.execute(
        "SELECT * FROM interventions WHERE intervention_id = ?",
        (intervention_id,)
    )
    columns = [desc[0] for desc in cursor.description]
    intervention = cursor.fetchone()
    
    if not intervention:
        conn.close()
        return {"error": f"Intervention not found: {intervention_id}"}
    
    intervention = dict(zip(columns, intervention))
    
    # Get original run events
    try:
        cursor = conn.execute(
            "SELECT * FROM experience_events WHERE run_id = ? ORDER BY timestamp",
            (intervention["run_id"],)
        )
        columns = [desc[0] for desc in cursor.description]
        events = [dict(zip(columns, row)) for row in cursor]
    except sqlite3.OperationalError:
        events = []
    
    # Get followup run events if present
    followup_events = []
    if intervention.get("followup_run_id"):
        try:
            cursor = conn.execute(
                "SELECT * FROM experience_events WHERE run_id = ? ORDER BY timestamp",
                (intervention["followup_run_id"],)
            )
            columns = [desc[0] for desc in cursor.description]
            followup_events = [dict(zip(columns, row)) for row in cursor]
        except sqlite3.OperationalError:
            pass
    
    # Get related cases
    try:
        cursor = conn.execute(
            "SELECT * FROM experience_cases WHERE run_id = ?",
            (intervention["run_id"],)
        )
        columns = [desc[0] for desc in cursor.description]
        cases = [dict(zip(columns, row)) for row in cursor]
    except sqlite3.OperationalError:
        cases = []
    
    conn.close()
    
    return {
        "intervention": intervention,
        "original_events": events,
        "followup_events": followup_events,
        "related_cases": cases,
    }


def verify_with_evidence(
    intervention_id: str,
    followup_run_id: str,
    db_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """
    Verify intervention with evidence from followup run.
    
    This closes the verification contract by:
    1. Checking followup_run_id exists in experience_events
    2. Checking followup run has VERIFIER_CONFIRMED outcome
    3. Deriving status from ACTUAL verifier outcomes, not caller parameter
    
    Args:
        intervention_id: Intervention to verify
        followup_run_id: Run that verifies the correction
        db_path: Database path
    
    Returns:
        Dict with verification_result, evidence_refs, and derived status
    """
    conn = init_db(db_path)
    
    # Check intervention exists
    cursor = conn.execute(
        "SELECT * FROM interventions WHERE intervention_id = ?",
        (intervention_id,)
    )
    intervention = cursor.fetchone()
    if not intervention:
        conn.close()
        return {"error": f"Intervention not found: {intervention_id}"}
    
    # Check followup run has VERIFIER_CONFIRMED events
    cursor = conn.execute("""
        SELECT COUNT(*) FROM experience_events
        WHERE run_id = ? AND value_source = 'VERIFIER_CONFIRMED'
    """, (followup_run_id,))
    verifier_confirmed_count = cursor.fetchone()[0]
    
    # Get ACTUAL verifier outcomes from experience_events
    cursor = conn.execute("""
        SELECT outcome FROM experience_events
        WHERE run_id = ? AND state = 'VERIFY' AND value_source = 'VERIFIER_CONFIRMED'
    """, (followup_run_id,))
    verifier_outcomes = [row[0] for row in cursor.fetchall()]
    
    conn.close()
    
    # Derive verification status from ACTUAL evidence
    if verifier_confirmed_count == 0:
        status = "UNKNOWN"  # No verifier evidence
        derived_from = "no_verifier_events"
    else:
        # Check actual outcomes - FAILED evidence means FAILED, not VERIFIED
        if any(o.upper() == "FAILED" for o in verifier_outcomes):
            status = "FAILED"
            derived_from = "actual_failed_outcome"
        elif any(o.upper() == "PASSED" for o in verifier_outcomes):
            status = "VERIFIED"
            derived_from = "actual_passed_outcome"
        elif any(o.upper() == "UNAVAILABLE" for o in verifier_outcomes):
            status = "UNKNOWN"
            derived_from = "actual_unavailable_outcome"
        else:
            status = "UNKNOWN"
            derived_from = "no_distinct_outcome"
    
    # Update intervention with evidence-derived status
    success = update_verification_status(
        intervention_id=intervention_id,
        verification_status=status,
        followup_run_id=followup_run_id,
        verification_source="evidence",
        db_path=db_path,
    )
    
    return {
        "intervention_id": intervention_id,
        "followup_run_id": followup_run_id,
        "verification_status": status,
        "derived_from": derived_from,
        "verifier_confirmed_count": verifier_confirmed_count,
        "verifier_outcomes": verifier_outcomes,
        "updated": success,
    }


# ── CLI ─────────────────────────────────────────────────────────────────────

def cmd_record(args: argparse.Namespace) -> int:
    """Handle 'record' command."""
    try:
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
            db_path=Path(args.db) if getattr(args, 'db', None) else None,
            validate_run=args.validate_run if hasattr(args, 'validate_run') else False,
        )
        
        print(json.dumps(record, indent=2))
        print(f"\n✓ Intervention recorded: {record['intervention_id']}", file=sys.stderr)
        return 0
    except ValueError as e:
        print(f"✗ Validation error: {e}", file=sys.stderr)
        return 1


def cmd_list(args: argparse.Namespace) -> int:
    """Handle 'list' command."""
    db_path = Path(args.db) if getattr(args, 'db', None) else None
    interventions = list_interventions(
        run_id=args.run_id,
        verification_status=args.status,
        db_path=db_path,
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
    db_path = Path(args.db) if getattr(args, 'db', None) else None
    
    if args.verify_with_evidence:
        # Use evidence-based verification (status derived from actual verifier outcomes)
        result = verify_with_evidence(
            intervention_id=args.intervention_id,
            followup_run_id=args.followup_run,
            db_path=db_path,
        )
        print(json.dumps(result, indent=2, default=str))
        if result.get("error"):
            print(f"\n✗ {result['error']}", file=sys.stderr)
            return 1
        print(f"\n✓ Verification status: {result['verification_status']} (derived from evidence)", file=sys.stderr)
        return 0
    
    # Simple status update (manual override - tracked as non-evidence source)
    success = update_verification_status(
        intervention_id=args.intervention_id,
        verification_status=args.status,
        followup_run_id=args.followup_run,
        verification_source="manual",
        db_path=db_path,
    )
    
    if success:
        print(f"✓ Updated verification status to '{args.status}'")
        return 0
    else:
        print(f"✗ Intervention not found: {args.intervention_id}")
        return 1


def cmd_trace(args: argparse.Namespace) -> int:
    """Handle 'trace' command."""
    db_path = Path(args.db) if getattr(args, 'db', None) else None
    trace = get_trace_for_intervention(args.intervention_id, db_path=db_path)
    
    if trace.get("error"):
        print(f"✗ {trace['error']}")
        return 1
    
    print(json.dumps(trace, indent=2, default=str))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Record and manage manual interventions for experience learning.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Record a manual intervention (requires existing run_id)
  record_intervention.py record run-123 step-456 \\
      --reason "Window identity ambiguous" \\
      --action "Selected target window manually"

  # Record with auto-validation (fail if run doesn't exist)
  record_intervention.py record run-123 step-456 \\
      --reason "Window identity ambiguous" \\
      --action "Selected target window manually" \\
      --validate-run

  # Force record even if run doesn't exist (testing)
  record_intervention.py record run-123 step-456 \\
      --reason "Window identity ambiguous" \\
      --action "Selected target window manually" \\
      --force

  # List all interventions for a run
  record_intervention.py list --run-id run-123

  # Verify with evidence (checks followup run has verifier evidence)
  record_intervention.py verify abc123def456 \\
      --verify-with-evidence \\
      --followup-run run-789 \\
      --outcome PASSED

  # Simple status update (no evidence validation)
  record_intervention.py verify abc123def456 \\
      --status VERIFIED \\
      --followup-run run-789

  # Get full trace for an intervention
  record_intervention.py trace abc123def456

  # Use specific database
  record_intervention.py list --db /path/to/experience.db
        """
    )
    
    parser.add_argument(
        "--db",
        help="Path to experience database (default: skill internal or ~/.cache)",
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
    p_record.add_argument(
        "--validate-run",
        action="store_true",
        help="Require run_id to exist in experience_events",
    )
    p_record.add_argument(
        "--force",
        action="store_true",
        help="Skip validation (for testing or known new runs)",
    )
    p_record.set_defaults(func=cmd_record)
    
    # list
    p_list = subparsers.add_parser("list", help="List interventions")
    p_list.add_argument("--run-id", help="Filter by run ID")
    p_list.add_argument("--status", help="Filter by verification status")
    p_list.set_defaults(func=cmd_list)
    
    # verify
    p_verify = subparsers.add_parser("verify", help="Update verification status")
    p_verify.add_argument("intervention_id", help="Intervention ID")
    p_verify.add_argument("--status", "-s", help="Verification status (VERIFIED/FAILED/UNKNOWN)")
    p_verify.add_argument("--followup-run", help="Followup run ID")
    p_verify.add_argument(
        "--verify-with-evidence",
        action="store_true",
        help="Verify with evidence check from followup run",
    )
    p_verify.add_argument(
        "--outcome",
        choices=["PASSED", "FAILED", "UNKNOWN"],
        help="Verification outcome from followup run",
    )
    p_verify.set_defaults(func=cmd_verify)
    
    # trace
    p_trace = subparsers.add_parser("trace", help="Get full trace for intervention")
    p_trace.add_argument("intervention_id", help="Intervention ID")
    p_trace.set_defaults(func=cmd_trace)
    
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
