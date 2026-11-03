#!/usr/bin/env python3
"""Batch runner: execute multiple vcli ops over a single SSH connection.

Usage (from Windows):
  python3 batch_runner.py --host user1@192.168.1.111 --script ops.sh

This avoids repeated SSH handshakes that trigger MaxStartups.
The script runs remotely with PID+wait collection.
"""
import argparse
import subprocess
import sys
import time
from pathlib import Path


def build_remote_script(ops: list[dict]) -> str:
    """Build a bash script that runs all ops in one SSH session.

    Each op: {"cmd": "vcli ...", "name": "op1"}
    Returns: bash script with stdout/stderr captured per-op.
    """
    lines = [
        "#!/bin/bash",
        "set -u",
        "export HOME=/home/user1",
        "export PATH=/usr/bin:/bin:/home/user1/.local/bin:$PATH",
        'OUTDIR=$(mktemp -d /tmp/vcli_batch.XXXXXX)',
        'echo "BATCH_DIR=$OUTDIR"',
        "",
    ]
    for i, op in enumerate(ops):
        name = op.get("name", f"op{i}")
        cmd = op["cmd"]
        lines += [
            f'# --- {name} ---',
            f'echo "{name}_START=$(date +%s.%N)"',
            f'{cmd} > "$OUTDIR/{name}.out" 2> "$OUTDIR/{name}.err" &',
            f'PID_{name}=$!',
            f'echo "{name}_PID=$PID_{name}"',
        ]
    lines += [
        "",
        "# Wait for all with timeout",
        'DEADLINE=$(($(date +%s) + 60))',
    ]
    for op in ops:
        name = op.get("name", f"op{i}")
        lines += [
            f'while kill -0 $PID_{name} 2>/dev/null; do',
            f'  now=$(date +%s)',
            f'  [ "$now" -gt "$DEADLINE" ] && {{ echo "{name}_TIMEOUT=1"; kill $PID_{name} 2>/dev/null; break; }}',
            f'  sleep 0.1',
            f'done',
        ]
    lines += [
        "",
        "echo '=== RESULTS ==='",
    ]
    for op in ops:
        name = op.get("name", f"op{i}")
        lines += [
            f'echo "{name}_EXIT=$(wait $PID_{name} 2>/dev/null; echo $?)";',
            f'echo "{name}_OUT_START";',
            f'cat "$OUTDIR/{name}.out";',
            f'echo "{name}_OUT_END";',
            f'echo "{name}_ERR_START";',
            f'cat "$OUTDIR/{name}.err";',
            f'echo "{name}_ERR_END";',
        ]
    lines.append('rm -rf "$OUTDIR"')
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", required=True, help="user@host")
    ap.add_argument("--key", default=None, help="SSH key path")
    ap.add_argument("--ops", nargs="+", required=True,
                    help="ops as name:cmd pairs")
    args = ap.parse_args()

    ops = []
    for spec in args.ops:
        name, cmd = spec.split(":", 1)
        ops.append({"name": name, "cmd": cmd})

    script = build_remote_script(ops)

    ssh_args = ["ssh", "-o", "ConnectTimeout=10"]
    if args.key:
        ssh_args += ["-i", args.key]
    ssh_args += [args.host, "bash -s"]

    print(f"Running {len(ops)} ops in single SSH session...")
    t0 = time.time()
    result = subprocess.run(ssh_args, input=script.encode("utf-8"), capture_output=True,
                             timeout=90)
    elapsed = time.time() - t0

    print(result.stdout.decode("utf-8", errors="replace"))
    if result.stderr:
        print("STDERR:", result.stderr.decode("utf-8", errors="replace"), file=sys.stderr)
    print(f"\nTotal wall time: {elapsed:.2f}s (1 SSH connection)")
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
