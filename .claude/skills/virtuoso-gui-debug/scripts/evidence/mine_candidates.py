#!/usr/bin/env python3
"""Mine candidate rules from intervention data."""

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Set

_SCRIPT_DIR = Path(__file__).parent.resolve()
_SKILL_ROOT = _SCRIPT_DIR.parent.parent
sys.path.insert(0, str(_SKILL_ROOT / "vgui_runner"))
sys.path.insert(0, str(_SCRIPT_DIR))

from record_intervention import init_db, get_default_db_path

SCHEMA_VERSION = "1.0"
TRUSTED_SOURCES = {"evidence"}


def norm_step(step_id):
    if step_id is None:
        return ""
    return str(step_id)


def strip_verification(details):
    if not details:
        return {}
    return {k: v for k, v in details.items() if k != "_verification"}


def canonical(iv):
    details = json.loads(iv.get("details") or "{}")
    ver = details.get("_verification", {})
    return {
        "id": iv.get("intervention_id"),
        "run": iv.get("run_id"),
        "step": iv.get("step_id"),
        "reason": iv.get("reason"),
        "action": iv.get("action"),
        "source": iv.get("source"),
        "status": iv.get("verification_status"),
        "followup": iv.get("followup_run_id"),
        "ver_source": ver.get("source"),
        "context": strip_verification(details),
    }


def hash_snapshot(data):
    return hashlib.sha256(json.dumps(data, sort_keys=True, default=str).encode()).hexdigest()[:16]


def _hash_sig(sig):
    """Create a stable short ID from signature."""
    return hashlib.sha256(sig.encode()).hexdigest()[:12]


def group_sig(reason, action, context):
    reason = reason or "EMPTY_REASON"
    action = action or "EMPTY_ACTION"
    ctx = {k: context[k] for k in sorted((context or {}).keys()) if k != "_verification"}
    return json.dumps({"reason": reason, "action": action, "context": ctx}, sort_keys=True)


def get_interventions(db_path):
    conn = init_db(db_path)
    cur = conn.execute("""
        SELECT intervention_id, run_id, step_id, reason, action, source,
               verification_status, followup_run_id, details, timestamp
        FROM interventions ORDER BY run_id, step_id, intervention_id
    """)
    cols = [d[0] for d in cur.description]
    result = [dict(zip(cols, row)) for row in cur]
    conn.close()
    return result


def _ref_sort_key(r):
    return (r["run"], r.get("fid") or "", r["id"])


class UnionFind:
    """Union-Find for computing connected components in run↔followup graph."""
    def __init__(self):
        self.parent = {}
    
    def find(self, x):
        if x not in self.parent:
            self.parent[x] = x
        elif self.parent[x] != x:
            self.parent[x] = self.find(self.parent[x])
        return self.parent[x]
    
    def union(self, x, y):
        px, py = self.find(x), self.find(y)
        if px != py:
            self.parent[px] = py


def compute_stats(sig, interventions):
    """
    Compute statistics for a group of interventions.
    
    Deduplication model:
    - attempts: total number of intervention records
    - support: number of connected components (Union-Find on run↔followup graph)
    
    Connectivity rules:
    - Two runs are connected if they share a followup_run_id
    - A run is connected to its followup_run_id
    - Each connected component contributes at most 1 support
    
    Status per component:
    - If any node in component has CONFLICT → entire component is conflict (0 support)
    - If any node has VERIFIED → component contributes to verified_count
    - If only FAILED → component contributes to failed_count
    """
    # Track runs and their followups
    run_followups = {}  # run_id -> set[followup_run_id]
    followup_runs = {}  # followup_run_id -> set[run_id]
    
    # Track refs and status per (run, fid) pair
    run_fid_refs = {}   # (run_id, fid) -> list of refs
    run_fid_status = {} # (run_id, fid) -> set of statuses
    
    # Manual/unknown tracking by run
    manual_verified = {}   # run_id -> [refs]
    manual_failed = {}     # run_id -> [refs]
    manual_conflict = {}   # run_id -> [refs]
    unknown = {}           # run_id -> [refs]
    
    for iv in interventions:
        details = json.loads(iv.get("details") or "{}")
        ver = details.get("_verification", {})
        src = ver.get("source") or iv.get("source", "unknown")
        status = iv.get("verification_status") or ver.get("status")
        run_id = iv.get("run_id")
        fid = iv.get("followup_run_id") or None
        ref = {
            "id": iv.get("intervention_id"),
            "run": run_id,
            "step": iv.get("step_id"),
            "src": src,
            "status": status,
            "fid": fid,
        }
        
        is_trusted = src in TRUSTED_SOURCES
        
        if is_trusted:
            # Build connectivity graph for VERIFIED/FAILED/CONFLICT
            if status != "UNKNOWN":
                run_followups.setdefault(run_id, set())
                if fid:
                    run_followups[run_id].add(fid)
                    followup_runs.setdefault(fid, set()).add(run_id)
                
                key = (run_id, fid)
                run_fid_refs.setdefault(key, []).append(ref)
                run_fid_status.setdefault(key, set()).add(status)
            
            # Track trusted UNKNOWNs in unknown dict (for count/refs, not connectivity)
            if status == "UNKNOWN":
                unknown.setdefault(run_id, []).append(ref)
        else:
            # Manual/other sources - track separately
            if status == "VERIFIED":
                manual_verified.setdefault(run_id, []).append(ref)
            elif status == "CONFLICT":
                manual_conflict.setdefault(run_id, []).append(ref)
            elif status == "FAILED":
                manual_failed.setdefault(run_id, []).append(ref)
            else:
                unknown.setdefault(run_id, []).append(ref)
    
    # Build connected components using Union-Find
    uf = UnionFind()
    
    # Union runs that share a followup
    for fid, runs in followup_runs.items():
        runs_list = sorted(runs)  # Deterministic
        for i in range(1, len(runs_list)):
            uf.union(runs_list[0], runs_list[i])
    
    # Union runs with their followups (run_id connected to fid)
    for run_id, fids in run_followups.items():
        for fid in fids:
            uf.union(run_id, fid)
    
    # Get connected components
    components = {}  # root -> {run_id -> [refs], statuses: set, fids: set}
    for (run_id, fid), refs in run_fid_refs.items():
        root = uf.find(run_id)
        if root not in components:
            components[root] = {"runs": {}, "fids": set(), "statuses": set(), "refs": []}
        if run_id not in components[root]["runs"]:
            components[root]["runs"][run_id] = []
        components[root]["runs"][run_id].extend(refs)
        if fid:
            components[root]["fids"].add(fid)
        components[root]["refs"].extend(refs)
        for status in run_fid_status.get((run_id, fid), set()):
            components[root]["statuses"].add(status)
    
    # Classify components
    verified_components = []
    failed_components = []
    conflict_components = []
    
    for root, comp in components.items():
        statuses = comp["statuses"]
        # CONFLICT if: explicit CONFLICT status OR mixed VERIFIED+FAILED
        if "CONFLICT" in statuses or ({"VERIFIED", "FAILED"} <= statuses):
            conflict_components.append(comp)
        elif "VERIFIED" in statuses:
            verified_components.append(comp)
        elif "FAILED" in statuses:
            failed_components.append(comp)
    
    # Counts
    verified_count = len(verified_components)
    failed_count = len(failed_components)
    conflict_count = len(conflict_components)
    # attempts = all intervention records (trusted + manual + unknown)
    attempts = sum(len(c["refs"]) for c in components.values())
    attempts += sum(len(refs) for refs in manual_verified.values())
    attempts += sum(len(refs) for refs in manual_failed.values())
    attempts += sum(len(refs) for refs in manual_conflict.values())
    attempts += sum(len(refs) for refs in unknown.values())
    
    manual_count = len(manual_verified) + len(manual_failed) + len(manual_conflict)
    unknown_count = len(unknown)
    
    # pass_rate: exclude unknown and conflict from denominator
    total = verified_count + failed_count
    pass_rate = verified_count / total if total > 0 else 0.0
    
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
    
    # Build refs with deterministic ordering
    vrefs = sorted([r for c in verified_components for r in c["refs"]], key=_ref_sort_key)[:5]
    frefs = sorted([r for c in failed_components for r in c["refs"]], key=_ref_sort_key)[:5]
    cref = sorted([r for c in conflict_components for r in c["refs"]], key=_ref_sort_key)[:10]
    mrefs = sorted([r for refs in manual_verified.values() for r in refs] +
                   [r for refs in manual_failed.values() for r in refs] +
                   [r for refs in manual_conflict.values() for r in refs],
                  key=_ref_sort_key)[:5]
    urefs = sorted([r for refs in unknown.values() for r in refs], key=_ref_sort_key)[:5]
    
    return {
        "sig": sig,
        "total": len(interventions),
        "stats": {
            "attempts": attempts,
            "verified_count": verified_count,
            "verified_components": len(verified_components),
            "manual_count": manual_count,
            "failed_count": failed_count,
            "failed_components": len(failed_components),
            "unknown_count": unknown_count,
            "conflict_count": conflict_count,
            "conflict_components": len(conflict_components),
            "pass_rate": round(pass_rate, 3),
        },
        "strength": strength,
        "vrefs": vrefs,
        "frefs": frefs,
        "cref": cref,
        "mrefs": mrefs,
        "urefs": urefs,
    }


def mine(min_verified=1, db_path=None):
    interventions = get_interventions(db_path)
    
    groups = {}
    for iv in interventions:
        details = json.loads(iv.get("details") or "{}")
        sig = group_sig(iv.get("reason", ""), iv.get("action", ""), details)
        groups.setdefault(sig, []).append(iv)
    
    all_stats = [compute_stats(sig, g) for sig, g in groups.items()]
    all_stats.sort(key=lambda x: x["stats"]["verified_count"], reverse=True)
    
    # Add stable candidate_id based on hash of sig
    for stats in all_stats:
        stats["candidate_id"] = _hash_sig(stats.get("sig", ""))
    
    candidates = [s for s in all_stats if s["stats"]["verified_count"] >= min_verified]
    
    canon = sorted([canonical(iv) for iv in interventions],
                   key=lambda x: (x["run"], norm_step(x["step"]), x["id"]))
    
    snap = {
        "v": SCHEMA_VERSION,
        "ivs": canon,
        "gc": len(all_stats),
        "cc": len(candidates),
    }
    h = hash_snapshot(snap)
    
    return {
        "id": "candidate-" + h,
        "v": SCHEMA_VERSION,
        "h": h,
        "min_verified": min_verified,
        "total_ivs": len(interventions),
        "total_cands": len(candidates),
        "total_groups": len(all_stats),
        "candidates": candidates,
        "note": "Support by unique followup. Conflicts separate.",
    }


def report(cands):
    lines = ["=" * 60, "Candidate Analysis", "=" * 60]
    for i, c in enumerate(cands, 1):
        s = c["stats"]
        lines.extend([
            str(i) + ". " + c["strength"] + " | pass=" + str(round(s["pass_rate"] * 100)) + "%",
            "   VER=" + str(s["verified_count"]) + " FAILED=" + str(s["failed_count"]) + " UNKNOWN=" + str(s["unknown_count"]),
            "",
        ])
    return "\n".join(lines)


def cmd_mine(args):
    out = mine(min_verified=args.min_verified, db_path=Path(args.db) if args.db else None)
    if args.format == "report":
        print(report(out["candidates"]))
    else:
        print(json.dumps(out, indent=2))
    return 0


def cmd_stats(args):
    ivs = get_interventions(Path(args.db) if args.db else None)
    print(json.dumps({"total": len(ivs)}, indent=2))
    return 0


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--db")
    subs = p.add_subparsers(required=True)
    m = subs.add_parser("mine")
    m.add_argument("--min-verified", type=int, default=1)
    m.add_argument("--format", choices=["json", "report"], default="json")
    m.set_defaults(func=cmd_mine)
    s = subs.add_parser("stats")
    s.set_defaults(func=cmd_stats)
    return p.parse_args().func(p.parse_args())


if __name__ == "__main__":
    sys.exit(main())
