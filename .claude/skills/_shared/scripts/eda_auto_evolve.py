#!/usr/bin/env python3
"""
EDA knowledge evolution: read error_log.jsonl, detect patterns appearing ≥ threshold,
auto-create memory/*.md files, sync to Tier2 MEMORY.md.

Usage: python3 eda_auto_evolve.py <skill_name>
"""

import json
import os
import re
import sys
from collections import Counter
from datetime import datetime
from pathlib import Path

SKILLS_DIR = Path(__file__).parent.parent.parent  # .claude/skills/
THRESHOLD = 3  # occurrences before creating a memory file
LOOKBACK_HOURS = 168  # 7 days


def skill_memory_dir(skill_name: str) -> Path:
    return SKILLS_DIR / skill_name / "memory"


def analyze_errors(skill_name: str) -> list[dict]:
    log_path = skill_memory_dir(skill_name) / "error_log.jsonl"
    if not log_path.exists():
        return []
    cutoff = datetime.now().timestamp() - LOOKBACK_HOURS * 3600
    entries = []
    for line in log_path.read_text().splitlines():
        try:
            e = json.loads(line)
            ts = datetime.fromisoformat(e["timestamp"]).timestamp()
            if ts >= cutoff:
                entries.append(e)
        except (json.JSONDecodeError, KeyError, ValueError):
            continue
    return entries


def detect_patterns(entries: list[dict]) -> list[dict]:
    counts = Counter(e["type"] for e in entries)
    patterns = []
    for etype, count in counts.items():
        if count >= THRESHOLD:
            matching = [e for e in entries if e["type"] == etype]
            patterns.append({
                "type": etype,
                "count": count,
                "description": matching[0]["pattern"],
                "sample_errors": matching[0]["errors"][:3],
                "first_seen": matching[0]["timestamp"],
            })
    return patterns


def memory_filename(error_type: str) -> str:
    return f"auto-{error_type.replace('_', '-')}.md"


def render_memory_md(skill_name: str, pattern: dict) -> str:
    now = datetime.now().strftime("%Y-%m-%d")
    sample = "\n".join(f"  {e}" for e in pattern["sample_errors"])
    return f"""# EDA 记忆: {pattern['description']}

> 自动生成 by eda_auto_evolve.py | 首次发现: {pattern['first_seen'][:10]} | 出现次数: {pattern['count']}

## 核心问题

错误类型: `{pattern['type']}`
来源: `{skill_name}`
出现频率: **{pattern['count']} 次**（过去 7 天）

## 典型错误输出

```
{sample}
```

## 解决方案

> TODO: 在此记录根因和修复方法
> 参考 {skill_name}/SKILL.md 中对应章节

## 记忆要点

- [ ] 理解错误触发条件
- [ ] 记住正确的替代方案
- [ ] 用 vcli/virtuoso 命令验证修复

*由 eda_auto_evolve.py 自动生成于 {now}*
"""


def sync_to_tier2(skill_name: str, new_files: list[str]) -> None:
    """Push new pattern summaries to ~/.claude/projects/.../memory/MEMORY.md"""
    if not new_files:
        return
    # Find Tier2 MEMORY.md for current project
    project_dir = Path.home() / ".claude" / "projects"
    if not project_dir.exists():
        return
    # Find the virtuoso-cli project memory
    for d in project_dir.iterdir():
        memory_md = d / "memory" / "MEMORY.md"
        if memory_md.exists() and "virtuoso-cli" in str(d):
            lines = memory_md.read_text().splitlines()
            # Append new entries (avoid duplicates)
            for fname in new_files:
                tag = f"[auto-evolution/{skill_name}/{fname}]"
                if not any(tag in l for l in lines):
                    skill_path = f".claude/skills/{skill_name}/memory/{fname}"
                    lines.append(f"- [{fname}]({skill_path}) — auto-captured {skill_name} error pattern")
            memory_md.write_text("\n".join(lines) + "\n")
            break


def update_index(skill_name: str) -> None:
    memory_dir = skill_memory_dir(skill_name)
    md_files = sorted(memory_dir.glob("*.md"))
    log_path = memory_dir / "error_log.jsonl"
    total = sum(1 for _ in log_path.read_text().splitlines()) if log_path.exists() else 0

    index_lines = [
        f"# {skill_name} 错误记忆索引",
        f"\n更新时间: {datetime.now().strftime('%Y-%m-%d %H:%M')}  ",
        f"总捕获错误: {total}  ",
        f"记忆文件: {len([f for f in md_files if f.name != 'ERROR_INDEX.md'])}  ",
        "\n## 记忆文件\n",
    ]
    for f in md_files:
        if f.name == "ERROR_INDEX.md":
            continue
        index_lines.append(f"- [{f.name}]({f.name})")

    (memory_dir / "ERROR_INDEX.md").write_text("\n".join(index_lines) + "\n")


def run_evolution(skill_name: str) -> dict:
    entries = analyze_errors(skill_name)
    if not entries:
        return {"skill": skill_name, "analyzed": 0, "new_memories": 0}

    patterns = detect_patterns(entries)
    memory_dir = skill_memory_dir(skill_name)
    new_files = []

    for pattern in patterns:
        fname = memory_filename(pattern["type"])
        fpath = memory_dir / fname
        if not fpath.exists():
            fpath.write_text(render_memory_md(skill_name, pattern))
            new_files.append(fname)

    if new_files:
        sync_to_tier2(skill_name, new_files)

    update_index(skill_name)

    _trigger_signal_match()

    return {
        "skill": skill_name,
        "analyzed": len(entries),
        "patterns_detected": len(patterns),
        "new_memories": len(new_files),
        "new_files": new_files,
    }


def _trigger_signal_match() -> None:
    """Refresh .pensieve/signals/latest.json after every evolution run."""
    import subprocess
    matcher = Path(__file__).parent / "signal_matcher.py"
    if matcher.exists():
        subprocess.run(["python3", str(matcher)], capture_output=True, timeout=15)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: eda_auto_evolve.py <skill_name>", file=sys.stderr)
        sys.exit(1)
    result = run_evolution(sys.argv[1])
    print(json.dumps(result, ensure_ascii=False, indent=2))
