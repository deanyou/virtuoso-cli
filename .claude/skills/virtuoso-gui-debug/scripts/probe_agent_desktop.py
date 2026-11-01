#!/usr/bin/env python3
"""Read-only AT-SPI capability probe for an explicitly named desktop app.

The snapshot is processed in memory. Raw names, values, refs and tree content
are never written to disk or included in the JSON report.
"""

import argparse
import json
import re
import subprocess
from collections import Counter
from typing import Callable, Dict, List


_ROLE_NAMES = frozenset(("window", "dialog", "group", "button", "textbox",
                         "text", "menu", "menuitem", "checkbox", "combobox",
                         "listitem", "treeitem", "radio", "tab", "unknown"))
_ERROR_CODE = re.compile(r"^[A-Z_]{1,40}$")


def _run(argv: List[str], timeout: int, runner: Callable) -> Dict:
    result = runner(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                    text=True, timeout=timeout, check=False)
    # agent-desktop uses JSON on stdout even for a structured error (exit 1).
    if len(result.stdout) > 4_000_000:
        raise ValueError("agent-desktop response too large")
    data = json.loads(result.stdout)
    if not isinstance(data, dict) or not isinstance(data.get("ok"), bool):
        raise ValueError("invalid agent-desktop envelope")
    if result.returncode not in (0, 1) or (result.returncode == 0) != data["ok"]:
        raise ValueError("inconsistent agent-desktop exit status")
    if not isinstance(data.get("data" if data["ok"] else "error"), dict):
        raise ValueError("missing agent-desktop result payload")
    version = data.get("version")
    if not isinstance(version, str) or version.split(".")[0] != "2":
        raise ValueError("unsupported agent-desktop envelope version")
    return data


def _count_tree(tree: Dict) -> Dict:
    roles = Counter()
    actionable = 0
    stack = [tree]
    while stack:
        node = stack.pop()
        if not isinstance(node, dict):
            continue
        if isinstance(node.get("ref_id"), str):
            actionable += 1
        role = node.get("role")
        if isinstance(role, str):
            roles[role if role in _ROLE_NAMES else "other"] += 1
        children = node.get("children", [])
        if isinstance(children, list):
            stack.extend(children)
    return {"node_count": sum(roles.values()), "actionable_count": actionable,
            "role_counts": dict(sorted(roles.items()))}


def probe(app: str, binary: str = "agent-desktop", timeout: int = 20,
          runner: Callable = subprocess.run) -> Dict:
    """Return an aggregate report. Never attempt an action or a fallback."""
    report = {"probe_version": 1, "mode": "read_only", "app": app,
              "availability": "unknown", "reason_code": "probe_failed"}
    if not app or not app.strip() or timeout < 1 or timeout > 120:
        report["reason_code"] = "invalid_input"
        return report
    try:
        status = _run([binary, "status"], timeout, runner)
        if not status["ok"]:
            report["availability"] = "unavailable"
            code = status.get("error", {}).get("code", "")
            report["reason_code"] = "status_" + (code if isinstance(code, str) and _ERROR_CODE.fullmatch(code) else "unknown")
            return report
        snapshot = _run([binary, "snapshot", "--app", app, "-i", "--compact"],
                        timeout, runner)
        if not snapshot["ok"]:
            report["availability"] = "unavailable"
            code = snapshot.get("error", {}).get("code", "")
            report["reason_code"] = "snapshot_" + (code if isinstance(code, str) and _ERROR_CODE.fullmatch(code) else "unknown")
            return report
        data = snapshot.get("data", {})
        if not isinstance(data, dict) or not isinstance(data.get("complete"), bool):
            raise ValueError("snapshot completeness is missing")
        counts = _count_tree(data.get("tree"))
        report.update(counts)
        report["snapshot_complete"] = data["complete"]
        report["availability"] = "observed" if data["complete"] else "partial"
        report["reason_code"] = "complete_snapshot" if data["complete"] else "partial_snapshot"
    except FileNotFoundError:
        report["availability"] = "unavailable"
        report["reason_code"] = "binary_missing"
    except subprocess.TimeoutExpired:
        report["reason_code"] = "timeout"
    except (UnicodeError, ValueError, TypeError, KeyError, json.JSONDecodeError):
        report["reason_code"] = "invalid_envelope"
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, help="exact app name from agent-desktop list-apps")
    parser.add_argument("--binary", default="agent-desktop")
    parser.add_argument("--timeout", type=int, default=20)
    args = parser.parse_args()
    result = probe(args.app, args.binary, args.timeout)
    print(json.dumps(result, sort_keys=True))
    return 0 if result["availability"] == "observed" else 2


if __name__ == "__main__":
    raise SystemExit(main())
