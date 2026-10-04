#!/usr/bin/env python3
"""
mine_candidates.py — Mine candidate rules from intervention data.

This script analyzes interventions and their verification outcomes to identify
patterns that could inform future automation decisions.

P2 of the Evidence Loop Plan:
https://github.com/deanyou/virtuoso-cli/blob/main/docs/vcli-evidence-loop-plan.html

Key constraints enforced:
1. Deterministic: Same data always produces same candidates (no timestamps in hash)
2. Deduplicated: Support counted by unique run, not by intervention
3. Correct strength: Pass rate uses verified/failed counts; manual/override separate
4. Specific grouping: Full reason + action + context signature
5. Traceable evidence: Include intervention/run/step references in output
"""

import argparse
import hashlib
import json
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional, Set

# Add script to path
_SCRIPT_DIR = Path(__file__).parent.resolve()  # scripts/evidence/
_SKILL_ROOT = _SCRIPT_DIR.parent.parent  # virtuoso-gui-debug/

sys.path.insert(0, str(_SKILL_ROOT / "vgui_runner"))
sys.path.insert(0, str(_SCRIPT_DIR))

from record_intervention import init_db, get_default_db_path

# Schema version for candidate schema
CANDIDATE_SCHEMA_VERSION = "1.0"

# Trusted verification sources (only these count as evidence)
TRUSTED_SOURCES = {"evidence"}
# All sources including manual
ALL_SOURCES = {"evidence", "manual", "override"}


def _canonicalize_intervention(iv: Dict[str, Any]) -> Dict[str, Any]:
    """
    Canonicalize intervention for deterministic hashing.
    Removes timestamps and non-deterministic fields.
    """
    return {
        "intervention_id": iv.get("intervention_id"),
        "run_id": iv.get("run_id"),
        "step_id": iv.get("step_id"),
        "reason": iv.get("reason"),
        "action": iv.get("action"),
        "source": iv.get("source"),
        "verification_status": iv.get("verification_status"),
        "followup_run_id": iv.get("followup_run_id"),
        # Keep details but remove _verification.timestamp if present
        "details_keys": sorted(json.loads(iv.get("details") or "{}").keys()),
    }


def _hash_snapshot(evidence_data: Dict[str, Any]) -> str:
    """
    Generate deterministic hash of evidence snapshot.
    
    Uses canonicalized interventions (no timestamps) for determinism.
    """
    canonical = json.dumps(evidence_data, sort_keys=True, default=str)
    return hashlib.sha256(canonical.encode()).hexdigest()[:16]


def _make_group_signature(reason: str, action: str, context: Optional[Dict]) -> str:
    """
    Create a specific group signature from reason + action + context.
    
    Uses full text (not truncated), includes action and context.
    Missing context is recorded as "MISSING_CONTEXT".
    """
    # Use full reason text (not truncated)
    reason_sig = reason or "EMPTY_REASON"
    action_sig = action or "EMPTY_ACTION"
    
    # Include context if available
    context_keys = sorted(context.keys()) if context else []
    
    return json.dumps({
        "reason": reason_sig,
        "action": action_sig,
        "context_keys": context_keys,
    }, sort_keys=True)


def get_interventions(db_path: Optional[Path] = None) -> List[Dict[str, Any]]:
    """Get all interventions from database."""
    conn = init_db(db_path)
    
    cursor = conn.execute("""
        SELECT 
            intervention_id,
            run_id,
            step_id,
            reason,
            action,
            source,
            verification_status,
            followup_run_id,
            details,
            timestamp
        FROM interventions
        ORDER BY run_id, step_id, intervention_id
    """)  # Stable order: run_id, step_id, intervention_id
    
    columns = [desc[0] for desc in cursor.description]
    interventions = [dict(zip(columns, row)) for row in cursor]
    conn.close()
    
    return interventions


def _compute_group_stats(
    signature: str,
    interventions: List[Dict[str, Any]],
) -> Dict[str, Any]:
    """
    Compute statistics for a group of interventions.
    
    Key changes:
    - Deduplicated by unique run_id (not intervention count)
    - Pass rate = verified_evidence / (verified_evidence + failed)
    - Manual-only records marked as "manual_only" (insufficient)
    """
    # Track unique runs for support
    verified_runs: Set[str] = set()
    failed_runs: Set[str] = set()
    unknown_runs: Set[str] = set()
    manual_verified_runs: Set[str] = set()
    
    # Full intervention references for traceability
    verified_interventions: List[Dict] = []
    failed_interventions: List[Dict] = []
    manual_interventions: List[Dict] = []
    unknown_interventions: List[Dict] = []
    
    for iv in interventions:
        details = json.loads(iv.get("details") or "{}")
        ver_info = details.get("_verification", {})
        source = ver_info.get("source", iv.get("source", "unknown"))
        status = iv.get("verification_status") or ver_info.get("status")
        run_id = iv.get("run_id")
        
        iv_ref = {
            "intervention_id": iv.get("intervention_id"),
            "run_id": run_id,
            "step_id": iv.get("step_id"),
            "source": source,
            "status": status,
            "followup_run_id": iv.get("followup_run_id"),
        }
        
        if status == "VERIFIED":
            if source in TRUSTED_SOURCES:
                verified_runs.add(run_id)
                verified_interventions.append(iv_ref)
            else:
                manual_verified_runs.add(run_id)
                manual_interventions.append(iv_ref)
        elif status == "FAILED":
            failed_runs.add(run_id)
            failed_interventions.append(iv_ref)
        else:  # UNKNOWN or None
            unknown_runs.add(run_id)
            unknown_interventions.append(iv_ref)
    
    # Deduplicated counts
    verified_count = len(verified_runs)
    manual_count = len(manual_verified_runs)
    failed_count = len(failed_runs)
    unknown_count = len(unknown_runs)
    
    # Pass rate: verified / (verified + failed)
    # Manual-only records should not contribute to pass rate
    total_verifiable = verified_count + failed_count
    if total_verifiable > 0:
        pass_rate = verified_count / total_verifiable
    else:
        pass_rate = 0.0  # No verifiable evidence
    
    # Strength classification
    # Only evidence-source VERIFIED counts as strong
    if verified_count >= 3 and pass_rate >= 0.8:
        strength = "strong"
    elif verified_count >= 1 and pass_rate >= 0.5:
        strength = "moderate"
    elif verified_count >= 1:
        strength = "weak"
    elif manual_count >= 1 and verified_count == 0:
        strength = "manual_only"  # Only manual records, no evidence
    else:
        strength = "insufficient"
    
    return {
        "signature": signature,
        "total_interventions": len(interventions),
        "unique_runs": verified_count + manual_count + failed_count + unknown_count,
        "stats": {
            "verified_evidence_count": verified_count,  # Deduplicated by run
            "verified_evidence_runs": sorted(verified_runs),
            "verified_manual_count": manual_count,
            "verified_manual_runs": sorted(manual_verified_runs),
            "failed_count": failed_count,
            "failed_runs": sorted(failed_runs),
            "unknown_count": unknown_count,
            "unknown_runs": sorted(unknown_runs),
            "pass_rate": round(pass_rate, 3),
        },
        "strength": strength,
        # Evidence for traceability
        "verified_evidence_refs": verified_interventions,
        "failed_refs": failed_interventions[:5],  # Limit for output size
        "manual_refs": manual_interventions,
        "unknown_refs": unknown_interventions[:5],  # Limit for output size
    }


def mine_candidates(
    min_verified: int = 1,
    db_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """
    Mine candidate rules from intervention data.
    
    Key properties:
    - Deterministic: Same data produces same hash (no timestamps)
    - Deduplicated: Support counted by unique run_id
    - Correct strength: Pass rate = verified / (verified + failed)
    - Traceable: Full intervention references included
    """
    interventions = get_interventions(db_path)
    
    # Group by full signature (reason + action + context)
    groups: Dict[str, List[Dict]] = {}
    for iv in interventions:
        details = json.loads(iv.get("details") or "{}")
        signature = _make_group_signature(
            reason=iv.get("reason", ""),
            action=iv.get("action", ""),
            context=details,
        )
        
        if signature not in groups:
            groups[signature] = []
        groups[signature].append(iv)
    
    # Compute stats for each group
    group_stats = []
    for sig, group_ivs in groups.items():
        stats = _compute_group_stats(sig, group_ivs)
        group_stats.append(stats)
    
    # Sort by verified evidence count, then pass rate
    group_stats.sort(
        key=lambda g: (g["stats"]["verified_evidence_count"], g["stats"]["pass_rate"]),
        reverse=True
    )
    
    # Filter by threshold
    candidates = [
        g for g in group_stats
        if g["stats"]["verified_evidence_count"] >= min_verified
    ]
    
    # Canonicalize interventions for deterministic snapshot
    canonical_ivs = [_canonicalize_intervention(iv) for iv in interventions]
    canonical_ivs.sort(key=lambda x: (x["run_id"], x["step_id"], x["intervention_id"]))
    
    # Build snapshot WITHOUT timestamp for determinism
    evidence_snapshot = {
        "schema_version": CANDIDATE_SCHEMA_VERSION,
        "interventions": canonical_ivs,
        "group_count": len(group_stats),
        "candidate_count": len(candidates),
    }
    
    snapshot_hash = _hash_snapshot(evidence_snapshot)
    
    return {
        "candidate_id": f"candidate-{snapshot_hash}",
        "schema_version": CANDIDATE_SCHEMA_VERSION,
        "miner_version": "1.0",
        "snapshot_hash": snapshot_hash,
        "min_verified_threshold": min_verified,
        "total_interventions_analyzed": len(interventions),
        "total_candidates": len(candidates),
        "total_groups": len(group_stats),
        "candidates": candidates,
        "evidence_note": (
            "Support deduplicated by run_id. "
            "Strength based on verified_evidence_count and pass_rate. "
            "Manual-only records are 'manual_only' strength (no evidence). "
            "Pass rate = verified / (verified + failed)."
        ),
    }


def generate_report(candidates: Dict[str, Any]) -> str:
    """Generate human-readable report from candidates."""
    lines = [
        "=" * 60,
        "Intervention Candidate Analysis Report",
        "=" * 60,
        f"Candidate ID: {candidates['candidate_id']}",
        f"Schema: v{candidates['schema_version']}",
        f"Snapshot: {candidates['snapshot_hash']}",
        "",
        f"Total interventions analyzed: {candidates['total_interventions_analyzed']}",
        f"Total groups: {candidates['total_groups']}",
        f"Candidates meeting threshold: {candidates['total_candidates']}",
        f"Min verified threshold: {candidates['min_verified_threshold']}",
        "",
        "-" * 60,
        "CANDIDATES (sorted by evidence strength)",
        "-" * 60,
        "",
    ]
    
    for i, cand in enumerate(candidates["candidates"], 1):
        stats = cand["stats"]
        lines.extend([
            f"{i}. {cand['signature'][:60]}..." if len(cand['signature']) > 60 else f"{i}. {cand['signature']}",
            f"   Strength: {cand['strength']}",
            f"   Pass rate: {stats['pass_rate']:.1%}",
            f"   VERIFIED (evidence): {stats['verified_evidence_count']}",
            f"   FAILED: {stats['failed_count']}",
            f"   UNKNOWN: {stats['unknown_count']}",
            f"   Manual-only: {stats['verified_manual_count']}",
            "",
            f"   Evidence runs: {', '.join(stats['verified_evidence_runs'][:5])}",
            "",
        ])
    
    lines.extend([
        "-" * 60,
        "NOTE",
        "-" * 60,
        candidates["evidence_note"],
        "",
        "=" * 60,
    ])
    
    return "\n".join(lines)


# ── CLI ─────────────────────────────────────────────────────────────────────

def cmd_mine(args: argparse.Namespace) -> int:
    """Handle 'mine' command."""
    output = mine_candidates(
        min_verified=args.min_verified,
        db_path=Path(args.db) if getattr(args, 'db', None) else None,
    )
    
    if args.format == "report":
        report = generate_report(output)
        print(report)
    else:
        print(json.dumps(output, indent=2, default=str))
    
    return 0


def cmd_stats(args: argparse.Namespace) -> int:
    """Handle 'stats' command."""
    interventions = get_interventions(
        db_path=Path(args.db) if getattr(args, 'db', None) else None,
    )
    print(json.dumps({
        "total_interventions": len(interventions),
        "interventions": interventions,
    }, indent=2, default=str))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Mine candidate rules from intervention data.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Mine candidates with default threshold
  mine_candidates.py mine

  # Mine with minimum 3 verified evidence
  mine_candidates.py mine --min-verified 3

  # Generate report format
  mine_candidates.py mine --format report

  # Get intervention data
  mine_candidates.py stats

Evidence Rules:
  - Support deduplicated by unique run_id
  - Only verification_source='evidence' + status='VERIFIED' counts as evidence
  - Pass rate = verified / (verified + failed)
  - Manual-only records marked as 'manual_only' strength
  - Same data always produces same snapshot hash (deterministic)
        """
    )
    
    parser.add_argument(
        "--db",
        help="Path to experience database (default: skill internal or ~/.cache)",
    )
    
    subparsers = parser.add_subparsers(dest="command", required=True)
    
    # mine
    p_mine = subparsers.add_parser("mine", help="Mine candidate rules")
    p_mine.add_argument(
        "--min-verified",
        type=int,
        default=1,
        help="Minimum VERIFIED evidence count (default: 1)",
    )
    p_mine.add_argument(
        "--format",
        choices=["json", "report"],
        default="json",
        help="Output format (default: json)",
    )
    p_mine.set_defaults(func=cmd_mine)
    
    # stats
    p_stats = subparsers.add_parser("stats", help="Get intervention statistics")
    p_stats.set_defaults(func=cmd_stats)
    
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
