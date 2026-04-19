#!/usr/bin/env python3
"""
EDA Signal Matcher: aggregate error signals across all skills → ranked skill recommendations.

Reads:  .claude/skills/*/memory/error_log.jsonl  (written by eda_memory_saver.py)
Writes: <project-root>/.pensieve/signals/latest.json

Produces two lists:
  recommended_skills  — skills Claude should load in the next session (hot errors)
  promotion_candidates — auto-*.md files ready for promotion to SKILL.md

Usage:
    python3 signal_matcher.py [--project-root PATH] [--hot-hours N] [--promote-threshold N]
"""

import argparse
import json
import os
import re
from collections import defaultdict
from datetime import datetime
from pathlib import Path

# ── Defaults ─────────────────────────────────────────────────────────────────
HOT_HOURS = 24          # "recently active" window for recommended_skills
WARM_HOURS = 168        # 7 days for promotion counting
PROMOTE_THRESHOLD = 5   # auto-*.md occurrences before flagging as ready


def find_project_root() -> Path:
    """Walk up from skills dir to find .pensieve/."""
    candidate = Path(__file__).resolve()
    for _ in range(6):
        candidate = candidate.parent
        if (candidate / ".pensieve").is_dir():
            return candidate
    raise RuntimeError("Could not find .pensieve/ in any parent of this script")


def skills_dir(script_path: Path) -> Path:
    return script_path.parent.parent.parent  # _shared/scripts/../../ → skills/


def read_log(log_path: Path, since_ts: float) -> list[dict]:
    entries = []
    try:
        for line in log_path.read_text(encoding="utf-8").splitlines():
            try:
                e = json.loads(line)
                ts = datetime.fromisoformat(e["timestamp"]).timestamp()
                if ts >= since_ts:
                    entries.append(e)
            except (json.JSONDecodeError, KeyError, ValueError):
                continue
    except OSError:
        pass
    return entries


def aggregate_signals(skills_root: Path, hot_since: float, warm_since: float) -> dict:
    """
    Return:
      hot_counts  : {skill_name: {error_type: [entries]}}  — last HOT_HOURS
      warm_counts : {skill_name: {error_type: count}}      — last WARM_HOURS
      last_seen   : {skill_name: str ISO timestamp}
    """
    hot: dict[str, dict] = defaultdict(lambda: defaultdict(list))
    warm_total: dict[str, int] = defaultdict(int)
    last_seen: dict[str, str] = {}

    for skill_dir in sorted(skills_root.iterdir()):
        if not skill_dir.is_dir() or skill_dir.name.startswith("_"):
            continue
        log_path = skill_dir / "memory" / "error_log.jsonl"
        if not log_path.exists():
            continue

        warm_entries = read_log(log_path, warm_since)
        hot_entries = [e for e in warm_entries
                       if datetime.fromisoformat(e["timestamp"]).timestamp() >= hot_since]

        name = skill_dir.name
        for e in hot_entries:
            hot[name][e["type"]].append(e)

        warm_total[name] = len(warm_entries)

        if warm_entries:
            latest = max(e["timestamp"] for e in warm_entries)
            last_seen[name] = latest

    return dict(hot), dict(warm_total), last_seen


def build_recommended(hot: dict, last_seen: dict) -> list[dict]:
    recs = []
    for skill_name, type_map in hot.items():
        total = sum(len(v) for v in type_map.values())
        top_types = sorted(type_map.keys(), key=lambda t: -len(type_map[t]))
        top_descriptions = []
        for t in top_types[:3]:
            sample = type_map[t][0]
            top_descriptions.append(f"{t} ×{len(type_map[t])}: {sample.get('pattern', t)}")
        recs.append({
            "skill": skill_name,
            "reason": "; ".join(top_descriptions),
            "error_types": top_types,
            "count": total,
            "last_seen": last_seen.get(skill_name, ""),
        })
    return sorted(recs, key=lambda r: -r["count"])


def build_promotion_candidates(skills_root: Path, warm_since: float,
                               threshold: int = PROMOTE_THRESHOLD) -> list[dict]:
    """
    Find auto-*.md files in skill memory dirs whose logged occurrence count
    meets or exceeds PROMOTE_THRESHOLD.
    """
    candidates = []
    for skill_dir in sorted(skills_root.iterdir()):
        if not skill_dir.is_dir() or skill_dir.name.startswith("_"):
            continue
        memory_dir = skill_dir / "memory"
        log_path = memory_dir / "error_log.jsonl"
        if not log_path.exists():
            continue

        warm_entries = read_log(log_path, warm_since)
        from collections import Counter
        type_counts = Counter(e["type"] for e in warm_entries)

        for auto_file in sorted(memory_dir.glob("auto-*.md")):
            # Derive error_type from filename: auto-sfe-30.md → sfe_30
            stem = auto_file.stem[len("auto-"):]          # "sfe-30"
            etype = stem.replace("-", "_")                  # "sfe_30"
            count = type_counts.get(etype, 0)
            if count == 0:
                # Try partial match (e.g. "sfe_generic")
                count = sum(v for k, v in type_counts.items() if etype in k or k in etype)
            ready = count >= threshold
            # Read first line of file to get description
            try:
                first_line = auto_file.read_text(encoding="utf-8").splitlines()[0]
                desc = first_line.lstrip("#").strip()
            except OSError:
                desc = auto_file.name
            candidates.append({
                "skill": skill_dir.name,
                "file": auto_file.name,
                "description": desc,
                "count": count,
                "ready": ready,
                "promote_to": f".claude/skills/{skill_dir.name}/SKILL.md",
            })
    return sorted(candidates, key=lambda c: (-c["count"], c["skill"]))


def write_signals(project_root: Path, recommended: list, candidates: list) -> Path:
    signals_dir = project_root / ".pensieve" / "signals"
    signals_dir.mkdir(parents=True, exist_ok=True)
    out = {
        "generated": datetime.now().isoformat(),
        "recommended_skills": recommended,
        "promotion_candidates": candidates,
        "summary": {
            "hot_skills": len(recommended),
            "ready_to_promote": sum(1 for c in candidates if c["ready"]),
        },
    }
    path = signals_dir / "latest.json"
    path.write_text(json.dumps(out, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return path


def main() -> None:
    parser = argparse.ArgumentParser(description="EDA signal matcher")
    parser.add_argument("--project-root", help="Path to project root (default: auto-detect)")
    parser.add_argument("--hot-hours", type=int, default=HOT_HOURS)
    parser.add_argument("--promote-threshold", type=int, default=PROMOTE_THRESHOLD)
    args = parser.parse_args()

    project_root = Path(args.project_root) if args.project_root else find_project_root()
    s_dir = skills_dir(Path(__file__).resolve())

    now = datetime.now().timestamp()
    hot_since = now - args.hot_hours * 3600
    warm_since = now - WARM_HOURS * 3600
    promote_threshold = args.promote_threshold

    hot, warm_total, last_seen = aggregate_signals(s_dir, hot_since, warm_since)
    recommended = build_recommended(hot, last_seen)
    candidates = build_promotion_candidates(s_dir, warm_since, promote_threshold)

    out_path = write_signals(project_root, recommended, candidates)

    result = {
        "signals_written": str(out_path),
        "hot_skills": len(recommended),
        "promotion_candidates": len(candidates),
        "ready_to_promote": sum(1 for c in candidates if c["ready"]),
    }
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
