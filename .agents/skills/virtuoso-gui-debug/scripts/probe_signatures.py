#!/usr/bin/env python3
"""
Probe function signatures by calling with nil args and capturing error messages.
"""

import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from skill_db import get_db, record_function, get_known_functions


def probe_signature(vcli, session, name):
    """
    Call function with various nil args to extract expected signature.
    Strategy: call with 0, 1, 2, 3 nils and see which error message appears.
    Returns (arg_count: int|None, signature: str|None).
    """
    results = {}

    for nargs in range(0, 6):
        args = ", ".join(["nil"] * nargs)
        skill_expr = f"{name}({args})"
        cmd = [vcli, "--session", session, "skill", "exec", skill_expr]

        try:
            r = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            out, err = r.communicate(timeout=10)
            output = out.decode('utf-8', errors='replace')
        except Exception:
            continue

        try:
            data = json.loads(output)
        except json.JSONDecodeError:
            continue

        status = data.get("status", "")
        msg = data.get("output", "")
        errors = data.get("errors", [])
        error_str = " ".join(errors) if errors else ""

        if status == "error" and (msg or error_str):
            # Extract argument count from error
            match = re.search(r'(\d+) expected,\s*(\d+) given', error_str)
            if match:
                n_expected = int(match.group(1))
                results[nargs] = f"needs {n_expected} args (got {nargs})"
            else:
                results[nargs] = error_str[:150]
        elif status == "success":
            results[nargs] = f"WORKS: {msg[:100]}"

    return results


def run_signature_probe(vcli, session, limit=None):
    conn = get_db()
    funcs = get_known_functions(conn)
    if limit:
        funcs = funcs[:limit]

    print(f"Probing signatures for {len(funcs)} functions...")

    for i, f in enumerate(funcs):
        name = f["name"]
        results = probe_signature(vcli, session, name)

        # Extract signature from error messages
        sig_parts = []
        for nargs in sorted(results.keys()):
            msg = results[nargs]
            sig_parts.append(f"n={nargs}: {msg[:80]}")

        signature = " | ".join(sig_parts) if sig_parts else None

        if signature:
            record_function(conn, name, True, signature=signature)

        if (i + 1) % 20 == 0:
            print(f"  [{i+1}/{len(funcs)}] probed")

    print("Done.")
    return conn


if __name__ == "__main__":
    vcli = str(Path.home() / ".local/bin/vcli")
    session = sys.argv[1] if len(sys.argv) > 1 else "dean-user1-37749"
    limit = int(sys.argv[2]) if len(sys.argv) > 2 else None
    run_signature_probe(vcli, session, limit)
