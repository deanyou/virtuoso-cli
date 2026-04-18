#!/usr/bin/env python3
"""
EDA error capture: extract SFE/OSSHNL/bridge error codes from vcli/spectre output,
classify to the right skill's memory/, append to error_log.jsonl.

Usage (from hook):
    echo "$TOOL_OUTPUT" | python3 eda_memory_saver.py "$COMMAND" - vcli
    python3 eda_memory_saver.py "$COMMAND" "$OUTPUT" spectre
"""

import json
import os
import re
import sys
import threading
from datetime import datetime
from pathlib import Path

# ── Error pattern routing table ──────────────────────────────────────────────
# (regex, target_skill, error_type, description_zh)
EDA_ERROR_PATTERNS = [
    # Spectre-specific
    (r"SFE-30\b", "spectre-netlist-gotchas", "sfe_30",
     "SFE-30: 使用 SPICE 参数 ac=，Spectre 原生语法应使用 mag="),
    (r"SFE-868\b", "spectre-netlist-gotchas", "sfe_868",
     "SFE-868: 模型文件路径解析失败（ADE oa/lib/../ 路径仅适用于 ADE，standalone 需绝对路径）"),
    (r"SFE-1997\b", "spectre-netlist-gotchas", "sfe_1997",
     "SFE-1997: oprobe 必须是电路元件（resistor/port），不能是节点名"),
    (r"SFE-675\b", "spectre-netlist-gotchas", "sfe_675",
     "SFE-675: modelFile 包含空 section 名——.lib 文件不能有空 section"),
    (r"SFE-\d+", "spectre-netlist-gotchas", "sfe_generic",
     "Spectre 仿真引擎错误（SFE-xxx）"),
    (r"terminated prematurely due to fatal error", "spectre-netlist-gotchas", "spectre_terminated",
     "Spectre 提前终止——查看 spectre.out 末尾两行定位根因"),
    (r"completes with \d+ error", "spectre-netlist-gotchas", "spectre_with_errors",
     "Spectre 完成但有错误——检查 spectre.out 中的 ERROR 行"),
    # Ocean netlisting
    (r"OSSHNL-116\b", "ocean-netlist-regen", "osshnl_116",
     "OSSHNL-116: 子 cell 无 spectre/schematic view（通常是 notes/title cell）"),
    (r"OSSHNL-109\b", "ocean-netlist-regen", "osshnl_109",
     "OSSHNL-109: netlisting 失败——检查 cell 的 spectre view 是否存在"),
    (r"OSSHNL-\d+", "ocean-netlist-regen", "osshnl_generic",
     "Ocean netlisting 错误（OSSHNL-xxx）"),
    (r"library.*not.*registered|not.*registered.*library", "ocean-netlist-regen", "lib_not_registered",
     "库未注册——Virtuoso 未从含 cds.lib 的正确目录启动"),
    (r"createNetlist.*nil", "ocean-netlist-regen", "create_netlist_nil",
     "createNetlist 返回 nil——库未注册或 cell 无 spectre view"),
    (r"run\(\) returns nil|run.*in.*0\.\d{1}s.*no spectre", "ocean-netlist-regen", "run_nil_fast",
     "run() 在 <0.3s 内返回 nil——modelFile 未设置或路径错误"),
    # SKILL shell / bridge
    (r"ipcBeginProcess.*127|exit.*code.*127", "skill-shell-gotchas", "ipc_127",
     "ipcBeginProcess exit 127——命令未在 PATH 中，需要绝对路径"),
    (r"\bsh\b.*returns nil|sh\(\).*nil", "skill-shell-gotchas", "sh_nil",
     "sh() 返回 nil（t/nil）不是 stdout——用 ipcBeginProcess 捕获输出"),
    (r"fprintf.*0 byte|0.byte.*fprintf", "skill-shell-gotchas", "fprintf_zero",
     "fprintf 写入 0 字节——路径中包含 ~ 需展开为绝对路径"),
    # Bridge / vcli
    (r"\[NAK\]", "vcli-errors", "bridge_nak",
     "Bridge 返回 NAK——SKILL 执行失败，查看 error 字段"),
    (r"connection refused|connection timed out|bridge.*timeout", "vcli-errors", "bridge_conn",
     "Bridge 连接失败——检查 VB_PORT/VB_SESSION 和 Virtuoso 进程状态"),
    (r"session.*not found|no session", "vcli-errors", "session_not_found",
     "Session 未找到——VB_SESSION 名称错误或 Virtuoso 重启了"),
]

SKILLS_DIR = Path(__file__).parent.parent.parent  # .claude/skills/

DEDUP_WINDOW_MINUTES = 60


def skill_memory_dir(skill_name: str) -> Path:
    d = SKILLS_DIR / skill_name / "memory"
    d.mkdir(parents=True, exist_ok=True)
    return d


def extract_error_lines(text: str) -> list[str]:
    """Pull lines containing EDA error patterns."""
    lines = []
    for line in text.splitlines():
        if any(kw in line for kw in ["SFE-", "OSSHNL-", "terminated prematurely",
                                      "ipcBeginProcess", "[NAK]", "Error (SFE",
                                      "completes with", "not registered",
                                      "createNetlist", "connection refused"]):
            lines.append(line.strip())
        if len(lines) >= 10:
            break
    return lines


def classify_error(text: str) -> tuple[str, str, str]:
    """Returns (skill_name, error_type, description)."""
    for pattern, skill, etype, desc in EDA_ERROR_PATTERNS:
        if re.search(pattern, text, re.IGNORECASE):
            return skill, etype, desc
    return "", "", ""


def is_duplicate(skill_name: str, error_type: str, source: str) -> bool:
    log_path = skill_memory_dir(skill_name) / "error_log.jsonl"
    if not log_path.exists():
        return False
    cutoff = datetime.now().timestamp() - DEDUP_WINDOW_MINUTES * 60
    try:
        lines = log_path.read_text().splitlines()
        for line in reversed(lines):
            try:
                entry = json.loads(line)
                ts = datetime.fromisoformat(entry["timestamp"]).timestamp()
                if ts < cutoff:
                    break
                if entry["type"] == error_type and entry["source"] == source:
                    return True
            except (json.JSONDecodeError, KeyError):
                continue
    except OSError:
        pass
    return False


def save_error(command: str, output: str, source: str) -> tuple[bool, str, str]:
    """
    Returns (saved, skill_name, description).
    source: "vcli" | "spectre" | "bridge" | "ocean"
    """
    error_lines = extract_error_lines(output)
    if not error_lines:
        return False, "", ""

    skill_name, error_type, description = classify_error(output)
    if not skill_name:
        return False, "", ""

    if is_duplicate(skill_name, error_type, source):
        return False, skill_name, description

    entry = {
        "timestamp": datetime.now().isoformat(),
        "type": error_type,
        "source": source,
        "command": command[:200],
        "message": error_lines[0],
        "errors": error_lines,
        "pattern": description,
        "context": {"session_id": "auto_hook"},
    }

    log_path = skill_memory_dir(skill_name) / "error_log.jsonl"
    with open(log_path, "a") as f:
        f.write(json.dumps(entry, ensure_ascii=False) + "\n")

    # Trigger evolution async
    threading.Thread(target=_trigger_evolve, args=(skill_name,), daemon=True).start()

    return True, skill_name, description


def _trigger_evolve(skill_name: str) -> None:
    evolve_script = Path(__file__).parent / "eda_auto_evolve.py"
    if evolve_script.exists():
        import subprocess
        subprocess.run(
            ["python3", str(evolve_script), skill_name],
            capture_output=True, timeout=30
        )


def main() -> None:
    if len(sys.argv) < 3:
        print("Usage: eda_memory_saver.py <command> <output_or_-> <source>", file=sys.stderr)
        sys.exit(2)

    command = sys.argv[1]
    output_arg = sys.argv[2]
    source = sys.argv[3] if len(sys.argv) > 3 else "vcli"

    output = sys.stdin.read() if output_arg == "-" else output_arg

    saved, skill_name, description = save_error(command, output, source)
    if saved:
        print(json.dumps({"saved": True, "skill": skill_name, "pattern": description},
                         ensure_ascii=False))
        sys.exit(0)
    elif skill_name:
        print(json.dumps({"saved": False, "skill": skill_name, "reason": "duplicate"}),
              file=sys.stderr)
        sys.exit(1)
    else:
        sys.exit(1)


if __name__ == "__main__":
    main()
