#!/usr/bin/env python3
"""
mine_candidates.py — Mine candidate rules from intervention data.

This script analyzes interventions and their verification outcomes to identify
patterns that could inform future automation decisions.

P2 of the Evidence Loop Plan:
https://github.com/deanyou/virtuoso-cli/blob/main/docs/vcli-evidence-loop-plan.html

Key constraints:
- Only VERIFIED interventions count as trusted evidence
- Manual/override sources are excluded from support counts
- Same snapshot always produces same candidates (deterministic)
"""

import argparse
import hashlib
import json
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

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


def _hash_snapshot(evidence_data: Dict[str, Any]) -> str:
    """Generate deterministic hash of evidence snapshot."""
    # Canonicalize for determinism
    canonical = json.dumps(evidence_data, sort_keys=True, default=str)
    return hashlib.sha256(canonical.encode()).hexdigest()[:16]


def get_intervention_stats(db_path: Optional[Path] = None) -> Dict[str, Any]:
    """
    Get intervention statistics grouped by reason/action patterns.
    
    Returns stats broken down by:
    - verification_status (VERIFIED, FAILED, UNKNOWN)
    - verification_source (evidence, manual, override)
    
    Only TRUSTED sources contribute to VERIFIED counts.
    """
    conn = init_db(db_path)
    
    # Get all interventions with their details
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
        ORDER BY timestamp DESC
    """)
    
    columns = [desc[0] for desc in cursor.description]
    interventions = [dict(zip(columns, row)) for row in cursor]
    conn.close()
    
    # Group by reason pattern (first 50 chars as signature)
    reason_groups: Dict[str, List[Dict]] = {}
    for iv in interventions:
        reason = iv.get("reason", "") or ""
        # Create reason signature (first 50 chars + hash if longer)
        if len(reason) > 50:
            reason_sig = reason[:47] + "..."
        else:
            reason_sig = reason
        
        if reason_sig not in reason_groups:
            reason_groups[reason_sig] = []
        reason_groups[reason_sig].append(iv)
    
    # Compute statistics per group
    stats = {
        "total_interventions": len(interventions),
        "groups": []
    }
    
    for reason_sig, group_ivs in reason_groups.items():
        group_stats = _compute_group_stats(reason_sig, group_ivs)
        stats["groups"].append(group_stats)
    
    # Sort by support count (VERIFIED evidence count)
    stats["groups"].sort(key=lambda g: g["stats"]["verified_evidence_count"], reverse=True)
    
    return stats


def _compute_group_stats(reason_sig: str, interventions: List[Dict]) -> Dict[str, Any]:
    """Compute statistics for a group of interventions."""
    # Initialize counters
    counters = {
        "total": len(interventions),
        "verified_evidence": 0,  # Only evidence source + VERIFIED status
        "verified_manual": 0,    # Manual/override + VERIFIED status
        "failed": 0,
        "unknown": 0,
        "by_run": set(),         # Unique run IDs
        "by_followup": set(),    # Unique followup run IDs
    }
    
    failed_cases = []
    
    for iv in interventions:
        details = json.loads(iv.get("details") or "{}")
        ver_info = details.get("_verification", {})
        source = ver_info.get("source", "unknown")
        status = iv.get("verification_status") or ver_info.get("status")
        
        counters["by_run"].add(iv.get("run_id"))
        if iv.get("followup_run_id"):
            counters["by_followup"].add(iv["followup_run_id"])
        
        # Count based on source and status
        if status == "VERIFIED":
            if source in TRUSTED_SOURCES:
                counters["verified_evidence"] += 1
            else:
                counters["verified_manual"] += 1
        elif status == "FAILED":
            counters["failed"] += 1
            failed_cases.append({
                "intervention_id": iv["intervention_id"],
                "run_id": iv["run_id"],
                "reason": iv.get("reason"),
                "action": iv.get("action"),
            })
        else:
            counters["unknown"] += 1
    
    # Determine candidate strength
    total_verified = counters["verified_evidence"] + counters["verified_manual"]
    evidence_ratio = counters["verified_evidence"] / max(1, total_verified)
    
    if counters["verified_evidence"] >= 3 and evidence_ratio >= 0.8:
        strength = "strong"
    elif counters["verified_evidence"] >= 1 and evidence_ratio >= 0.5:
        strength = "moderate"
    elif total_verified >= 1:
        strength = "weak"
    else:
        strength = "insufficient"
    
    return {
        "reason_signature": reason_sig,
        "sample_count": len(interventions),
        "stats": {
            "verified_evidence_count": counters["verified_evidence"],
            "verified_manual_count": counters["verified_manual"],
            "failed_count": counters["failed"],
            "unknown_count": counters["unknown"],
            "unique_runs": len(counters["by_run"]),
            "unique_followups": len(counters["by_followup"]),
        },
        "strength": strength,
        "evidence_ratio": round(evidence_ratio, 2),
        "failed_cases": failed_cases[:5],  # First 5 failed cases for analysis
    }


def mine_candidates(
    min_verified: int = 1,
    db_path: Optional[Path] = None,
    output_format: str = "json",
) -> Dict[str, Any]:
    """
    Mine candidate rules from intervention data.
    
    Args:
        min_verified: Minimum VERIFIED evidence count to include
        db_path: Optional database path
        output_format: Output format (json or report)
    
    Returns:
        Candidate analysis with candidates and statistics
    """
    # Get snapshot of intervention data
    stats = get_intervention_stats(db_path)
    
    # Filter to candidates meeting threshold
    candidates = []
    for group in stats["groups"]:
        if group["stats"]["verified_evidence_count"] >= min_verified:
            candidates.append(group)
    
    # Build candidate records with provenance
    evidence_snapshot = {
        "schema_version": CANDIDATE_SCHEMA_VERSION,
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "stats": {
            "total_interventions": stats["total_interventions"],
            "total_groups": len(stats["groups"]),
            "candidates_meeting_threshold": len(candidates),
        },
        "groups": stats["groups"],
    }
    
    snapshot_hash = _hash_snapshot(evidence_snapshot)
    
    candidate_output = {
        "candidate_id": f"candidate-{snapshot_hash}",
        "schema_version": CANDIDATE_SCHEMA_VERSION,
        "miner_version": "1.0",
        "snapshot_hash": snapshot_hash,
        "min_verified_threshold": min_verified,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "total_interventions_analyzed": stats["total_interventions"],
        "total_candidates": len(candidates),
        "candidates": candidates,
        "evidence_note": (
            "Only interventions with verification_source='evidence' and "
            "verification_status='VERIFIED' count as trusted support. "
            "Manual/override sources are excluded from evidence counts."
        ),
    }
    
    return candidate_output


def generate_report(candidates: Dict[str, Any]) -> str:
    """Generate human-readable report from candidates."""
    lines = [
        "=" * 60,
        "Intervention Candidate Analysis Report",
        "=" * 60,
        f"Generated: {candidates['generated_at']}",
        f"Schema: v{candidates['schema_version']}",
        f"Snapshot: {candidates['snapshot_hash']}",
        "",
        f"Total interventions analyzed: {candidates['total_interventions_analyzed']}",
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
            f"{i}. Reason: {cand['reason_signature']}",
            f"   Sample count: {cand['sample_count']}",
            f"   Strength: {cand['strength']}",
            f"   Evidence ratio: {cand['evidence_ratio']:.0%}",
            f"   VERIFIED (evidence): {stats['verified_evidence_count']}",
            f"   VERIFIED (manual): {stats['verified_manual_count']}",
            f"   FAILED: {stats['failed_count']}",
            f"   UNKNOWN: {stats['unknown_count']}",
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
    stats = get_intervention_stats(
        db_path=Path(args.db) if getattr(args, 'db', None) else None,
    )
    print(json.dumps(stats, indent=2, default=str))
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

  # Get intervention statistics
  mine_candidates.py stats

Evidence Rules:
  - Only verification_source='evidence' + status='VERIFIED' counts as trusted
  - Manual/override sources excluded from evidence counts
  - Same data always produces same candidates (deterministic)
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
