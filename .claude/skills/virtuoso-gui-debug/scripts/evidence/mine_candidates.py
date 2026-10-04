#!/usr/bin/env python3
"""
mine_candidates.py — Mine candidate rules from intervention data.

This script analyzes interventions and their verification outcomes to identify
patterns that could inform future automation decisions.

P2 of the Evidence Loop Plan:
https://github.com/deanyou/virtuoso-cli/blob/main/docs/vcli-evidence-loop-plan.html

Key constraints enforced:
1. Deterministic: Same data always produces same candidates (stable hash)
2. Deduplicated: Support counted by unique followup run (shared verification counts once)
3. Correct strength: Pass rate uses verified/failed counts; manual/override separate
4. Specific grouping: Full reason + action + context values (not just keys)
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


def _normalize_step_id(step_id: Any) -> str:
    """Normalize step_id for sorting, handling None and strings."""
    if step_id is None:
        return ""
    return str(step_id)


def _strip_verification_metadata(details: Dict[str, Any]) -> Dict[str, Any]:
    """
    Strip verification metadata from details for grouping.
    Only keeps non-verification context.
    """
    if not details:
        return {}
    result = {}
    for k, v in details.items():
        if k == "_verification":
            continue  # Skip verification metadata
        result[k] = v
    return result


def _canonicalize_intervention(iv: Dict[str, Any]) -> Dict[str, Any]:
    """
    Canonicalize intervention for deterministic hashing.
    Keeps all fields that affect conclusions, removes only timestamps.
    """
    details = json.loads(iv.get("details") or "{}")
    ver_info = details.get("_verification", {})
    
    return {
        "intervention_id": iv.get("intervention_id"),
        "run_id": iv.get("run_id"),
        "step_id": iv.get("step_id"),
        "reason": iv.get("reason"),
        "action": iv.get("action"),
        # Include source and status that affect conclusions
        "source": iv.get("source"),
        "verification_status": iv.get("verification_status"),
        "followup_run_id": iv.get("followup_run_id"),
        # Include source from verification if present
        "ver_source": ver_info.get("source"),
        # Include context values (not just keys)
        "context": _strip_verification_metadata(details),
    }


def _hash_snapshot(evidence_data: Dict[str, Any]) -> str:
    """
    Generate deterministic hash of evidence snapshot.
    """
    canonical = json.dumps(evidence_data, sort_keys=True, default=str)
    return hashlib.sha256(canonical.encode()).hexdigest()[:16]


def _make_group_signature(reason: str, action: str, context: Optional[Dict]) -> str:
    """
    Create a specific group signature from reason + action + context VALUES.
    
    Uses full text and actual context VALUES (not just keys).
    Strips _verification metadata from context.
    """
    reason_sig = reason or "EMPTY_REASON"
    action_sig = action or "EMPTY_ACTION"
    
    # Strip verification metadata and get sorted context
    clean_context = _strip_verification_metadata(context or {})
    # Sort by key for deterministic output
    sorted_context = {k: clean_context[k] for k in sorted(clean_context.keys())}
    
    return json.dumps({
        "reason": reason_sig,
        "action": action_sig,
        "context": sorted_context,
    }, sort_keys=True, default=str)


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
    """)
    
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
    
    Key principles:
    1. Support deduplication: Same original run counts once.
       When same run has multiple followups, count as 1 support.
    2. Conflict resolution: If a followup has both PASSED and FAILED,
       exclude from both.
    3. Source filtering: Only TRUSTED_SOURCES count for verified/failed.
    4. Unknown/manual use original run_id.
    """
    # Track by original run_id
    run_verified: Dict[str, List[Dict]] = {}   # run_id -> VERIFIED refs
    run_failed: Dict[str, List[Dict]] = {}    # run_id -> FAILED refs
    run_unknown: Dict[str, List[Dict]] = {}  # run_id -> UNKNOWN refs
    run_manual: Dict[str, List[Dict]] = {}     # run_id -> manual VERIFIED refs
    
    # All unique runs and followups
    all_runs: Set[str] = set()
    all_followups: Set[str] = set()
    
    # First pass: collect all data
    for iv in interventions:
        details = json.loads(iv.get("details") or "{}")
        ver_info = details.get("_verification", {})
        source = ver_info.get("source", iv.get("source", "unknown"))
        status = iv.get("verification_status") or ver_info.get("status")
        run_id = iv.get("run_id")
        followup_id = iv.get("followup_run_id")
        
        all_runs.add(run_id)
        if followup_id:
            all_followups.add(followup_id)
        
        iv_ref = {
            "intervention_id": iv.get("intervention_id"),
            "run_id": run_id,
            "step_id": iv.get("step_id"),
            "source": source,
            "status": status,
            "followup_run_id": followup_id,
        }
        
        if status == "VERIFIED":
            if source in TRUSTED_SOURCES:
                if run_id not in run_verified:
                    run_verified[run_id] = []
                run_verified[run_id].append(iv_ref)
            else:
                if run_id not in run_manual:
                    run_manual[run_id] = []
                run_manual[run_id].append(iv_ref)
        elif status == "FAILED":
            if source in TRUSTED_SOURCES:
                if run_id not in run_failed:
                    run_failed[run_id] = []
                run_failed[run_id].append(iv_ref)
        else:  # UNKNOWN or None
            if run_id not in run_unknown:
                run_unknown[run_id] = []
            run_unknown[run_id].append(iv_ref)
    
    # Detect conflicting followups: appear in both verified and failed
    conflicting_followups: Set[str] = set()
    verified_followups: Set[str] = set()
    failed_followups: Set[str] = set()
    
    # Collect followup_ids from verified runs
    for iv_refs in run_verified.values():
        for ref in iv_refs:
            fid = ref.get("followup_run_id")
            if fid:
                verified_followups.add(fid)
    
    # Collect from failed runs
    for iv_refs in run_failed.values():
        for ref in iv_refs:
            fid = ref.get("followup_run_id")
            if fid:
                failed_followups.add(fid)
    
    # Find conflicts: followups in both verified and failed
    conflicting_followups = verified_followups & failed_followups
    
    # Count supports: same original run = 1 support (regardless of followup count)
    # If run has conflicting followup, exclude it
    verified_runs_supporting: Set[str] = set()
    for run_id, iv_refs in run_verified.items():
        has_conflict = any(
            ref.get("followup_run_id") in conflicting_followups
            for ref in iv_refs
        )
        if not has_conflict:
            verified_runs_supporting.add(run_id)
    
    failed_runs: Set[str] = set()
    for run_id, iv_refs in run_failed.items():
        has_conflict = any(
            ref.get("followup_run_id") in conflicting_followups
            for ref in iv_refs
        )
        if not has_conflict:
            failed_runs.add(run_id)
    
    # Deduplicated counts
    verified_count = len(verified_runs_supporting)
    failed_count = len(failed_runs)
    manual_count = len(run_manual)
    unknown_count = len(run_unknown)
    
    # Pass rate: verified / (verified + failed)
    total_verifiable = verified_count + failed_count
    if total_verifiable > 0:
        pass_rate = verified_count / total_verifiable
    else:
        pass_rate = 0.0
    
    # Strength classification
    if verified_count >= 3 and pass_rate >= 0.8:
        strength = "strong"
    elif verified_count >= 1 and pass_rate >= 0.5:
        strength = "moderate"
    elif verified_count >= 1:
        strength = "weak"
    elif manual_count >= 1 and verified_count == 0:
        strength = "manual_only"
    else:
        strength = "insufficient"
    
    # Flatten refs
    verified_refs = [ref for refs in run_verified.values() for ref in refs]
    failed_refs = [ref for refs in run_failed.values() for ref in refs]
    unknown_refs = [ref for refs in run_unknown.values() for ref in refs]
    manual_refs = [ref for refs in run_manual.values() for ref in refs]
    
    return {
        "signature": signature,
        "total_interventions": len(interventions),
        "unique_runs": len(all_runs),
        "stats": {
            "verified_evidence_count": verified_count,
            "verified_followups": sorted(verified_followups - conflicting_followups),
            "verified_manual_count": manual_count,
            "failed_count": failed_count,
            "failed_followups": sorted(failed_followups - conflicting_followups),
            "unknown_count": unknown_count,
            "conflicting_followups": sorted(conflicting_followups),
            "pass_rate": round(pass_rate, 3),
        },
        "strength": strength,
        "verified_evidence_refs": verified_refs,
        "failed_refs": failed_refs[:5],
        "manual_refs": manual_refs,
        "unknown_refs": unknown_refs[:5],
    }


def mine_candidates(
    min_verified: int = 1,
    db_path: Optional[Path] = None,
) -> Dict[str, Any]:
    """
    Mine candidate rules from intervention data.
    """
    interventions = get_interventions(db_path)
    
    # Group by full signature (reason + action + context VALUES)
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
    # Stable sort by normalized step_id
    canonical_ivs.sort(key=lambda x: (
        x["run_id"],
        _normalize_step_id(x.get("step_id")),
        x["intervention_id"]
    ))
    
    # Build snapshot
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
            "Support deduplicated by followup_run_id (shared verification counts once). "
            "Pass rate = verified_followups / (verified_followups + failed_followups). "
            "Only TRUSTED_SOURCES count for verified/failed. "
            "Manual-only records marked as 'manual_only' strength."
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
        f"Total interventions: {candidates['total_interventions_analyzed']}",
        f"Total groups: {candidates['total_groups']}",
        f"Candidates: {candidates['total_candidates']}",
        "",
        "-" * 60,
        "CANDIDATES",
        "-" * 60,
        "",
    ]
    
    for i, cand in enumerate(candidates["candidates"], 1):
        stats = cand["stats"]
        lines.extend([
            f"{i}. Strength: {cand['strength']}",
            f"   Pass rate: {stats['pass_rate']:.1%}",
            f"   VERIFIED (evidence): {stats['verified_evidence_count']}",
            f"   FAILED: {stats['failed_count']}",
            f"   Manual-only: {stats['verified_manual_count']}",
            f"   UNKNOWN: {stats['unknown_count']}",
            "",
        ])
    
    lines.extend([
        "-" * 60,
        candidates["evidence_note"],
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
        print(generate_report(output))
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
    )
    
    parser.add_argument(
        "--db",
        help="Path to experience database (default: skill internal or ~/.cache)",
    )
    
    subparsers = parser.add_subparsers(dest="command", required=True)
    
    p_mine = subparsers.add_parser("mine", help="Mine candidate rules")
    p_mine.add_argument("--min-verified", type=int, default=1)
    p_mine.add_argument("--format", choices=["json", "report"], default="json")
    p_mine.set_defaults(func=cmd_mine)
    
    p_stats = subparsers.add_parser("stats", help="Get interventions")
    p_stats.set_defaults(func=cmd_stats)
    
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
