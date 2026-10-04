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


def compute_stats(sig, interventions):
    fv = {}
    ff = {}
    rm = {}
    ru = {}
    
    for iv in interventions:
        details = json.loads(iv.get("details") or "{}")
        ver = details.get("_verification", {})
        src = ver.get("source") or iv.get("source", "unknown")
        status = iv.get("verification_status") or ver.get("status")
        run_id = iv.get("run_id")
        fid = iv.get("followup_run_id")
        ref = {
            "id": iv.get("intervention_id"),
            "run": run_id,
            "step": iv.get("step_id"),
            "src": src,
            "status": status,
            "fid": fid,
        }
        
        if status == "VERIFIED":
            if src in TRUSTED_SOURCES:
                fid_key = fid or "local"
                fv.setdefault(fid_key, []).append(ref)
            else:
                rm.setdefault(run_id, []).append(ref)
        elif status == "CONFLICT":
            # Track conflicts by followup
            fid_key = fid or "local"
            fv.setdefault(fid_key, []).append(ref)
            ff.setdefault(fid_key, []).append(ref)
        elif status == "FAILED":
            if src in TRUSTED_SOURCES:
                fid_key = fid or "local"
                ff.setdefault(fid_key, []).append(ref)
        else:
            ru.setdefault(run_id, []).append(ref)
    
    conflicts = set(fv.keys()) & set(ff.keys())
    verified_followups = set(fv.keys()) - conflicts
    verified_count = len(verified_followups)
    failed_followups = set(ff.keys()) - conflicts
    failed_count = len(failed_followups)
    
    conflict_refs = []
    for fid in conflicts:
        conflict_refs.extend(fv.get(fid, []))
        conflict_refs.extend(ff.get(fid, []))
    
    manual_count = len(rm)
    unknown_count = len(ru)
    
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
    
    vrefs = []
    for fid in verified_followups:
        vrefs.extend(fv[fid])
    frefs = []
    for fid in failed_followups:
        frefs.extend(ff[fid])
    mrefs = [r for refs in rm.values() for r in refs]
    urefs = [r for refs in ru.values() for r in refs]
    
    return {
        "sig": sig,
        "total": len(interventions),
        "stats": {
            "verified_count": verified_count,
            "verified_fids": sorted(f for f in verified_followups if f != "local"),
            "manual_count": manual_count,
            "failed_count": failed_count,
            "failed_fids": sorted(f for f in failed_followups if f != "local"),
            "unknown_count": unknown_count,
            "conflict_count": len(conflict_refs),
            "conflicts": sorted(conflicts),
            "pass_rate": round(pass_rate, 3),
        },
        "strength": strength,
        "vrefs": vrefs,
        "frefs": frefs[:5],
        "cref": conflict_refs[:10],
        "mrefs": mrefs,
        "urefs": urefs[:5],
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
