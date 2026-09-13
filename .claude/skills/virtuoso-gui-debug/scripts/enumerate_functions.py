#!/usr/bin/env python3
"""
Enumerate Virtuoso SKILL functions by name pattern, test existence via vcli,
and record results in SQLite.

Strategy:
1. Generate candidate function names from prefixes x entities
2. For each, call `name()` with no args via vcli skill exec
3. If response contains "undefined" → function doesn't exist
4. If response contains "error" with argument info → exists, extract signature
5. Record everything to skill_db.sqlite3

Usage:
    python3 enumerate_functions.py [--session SESSION_ID] [--prefix dbCreate]
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from skill_db import get_db, record_function, record_layer, record_shape_property


# Function name patterns
PREFIXES = [
    "dbCreate", "dbDelete", "dbGet", "dbOpen", "dbSave", "dbClose",
    "dbFind", "dbCopy", "dbMove", "dbTransform",
    "hiSet", "hiSelect", "hiClear", "hiUpdate",
    "geGet", "geFind",
    "ddGet", "ddOpen",
]

ENTITIES = [
    "Rect", "Polygon", "Path", "Inst", "Net", "Pin", "Contact",
    "Label", "Text", "CellView", "Cell", "Lib", "Shape",
    "Wire", "Bus", "Group", "Reference",
    "Layer", "Purpose", "Region", "Blockage",
    "Via", "Fence", "Hole",
]

# Standalone functions to test directly
STANDALONE = [
    "startFinder", "geGetEditCellView", "hiGetCurrentWindow",
    "dbSave", "dbClose", "dbOpenCellView",
    "hiClearSelected", "hiSetDrawMode",
]


def build_candidates():
    """Generate all candidate function names."""
    candidates = set(STANDALONE)
    for prefix in PREFIXES:
        for entity in ENTITIES:
            candidates.add(prefix + entity)
    return sorted(candidates)


def test_function(vcli, session, name):
    """
    Test if a SKILL function exists.
    Returns (exists: bool, signature: str|None, raw: str)
    """
    skill_expr = f"{name}()"
    cmd = [
        vcli, "--session", session, "skill", "exec", skill_expr
    ]
    try:
        result = subprocess.run(
            cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15
        )
        output = result.stdout.decode('utf-8', errors='replace').strip()
        stderr = result.stderr.decode('utf-8', errors='replace').strip()
        if not output:
            return False, None, ""

        # Parse JSON response
        try:
            data = json.loads(output)
        except json.JSONDecodeError:
            return False, None, (output + " " + stderr)[:200]

        status = data.get("status", "")
        out = data.get("output", "")

        if "undefined" in out.lower() or "undefined" in status.lower():
            return False, None, out[:200]

        if status == "success":
            # Function exists and returned something
            return True, None, out[:200]

        if status == "error":
            # Function exists but wrong args — extract signature from error
            return True, out[:300], out[:300]

        return True, None, out[:200]

    except subprocess.TimeoutExpired:
        return None, None, "timeout"
    except Exception as e:
        return None, None, str(e)[:200]


def run_enumeration(vcli, session, prefix_filter=None):
    """Run enumeration and populate database."""
    conn = get_db()
    candidates = build_candidates()

    if prefix_filter:
        candidates = [c for c in candidates if c.startswith(prefix_filter)]

    print(f"Testing {len(candidates)} candidate functions...")
    found = 0
    missing = 0

    for i, name in enumerate(candidates):
        exists, sig, raw = test_function(vcli, session, name)

        if exists is True:
            record_function(conn, name, True, signature=sig, notes=raw)
            found += 1
            print(f"  [{i+1}/{len(candidates)}] ✓ {name}")
        elif exists is False:
            record_function(conn, name, False)
            missing += 1
        else:
            # timeout or error — skip
            pass

        # Progress every 20
        if (i + 1) % 20 == 0:
            print(f"  ... progress: {i+1}/{len(candidates)}, found={found}")

    print(f"\nDone: {found} found, {missing} missing, {len(candidates)-found-missing} skipped")
    return conn


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--session", required=True, help="vcli session ID")
    parser.add_argument("--vcli", default="~/.local/bin/vcli", help="vcli path")
    parser.add_argument("--prefix", default=None, help="Filter by prefix (e.g. dbCreate)")
    args = parser.parse_args()

    vcli = args.vcli.replace("~", str(Path.home()))
    conn = run_enumeration(vcli, args.session, args.prefix)

    from skill_db import stats
    print(f"\nDatabase stats: {json.dumps(stats(conn), indent=2)}")
