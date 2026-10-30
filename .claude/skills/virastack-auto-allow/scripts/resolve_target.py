#!/usr/bin/env python3
"""Resolve the live ViraStack target: session id, DISPLAY and window id.

Prints a JSON object with the arguments `auto_allow.py` needs:

    {"session": "...", "display": ":0", "window_id": "0x...", "pid": 5730,
     "title": "ViraStack : ...", "alternatives": ["0x...", ...]}

`vcli` is the only binary boundary used — the window list comes from
`vcli window list-windows-x11`, which resolves the X display and the
XAUTHORITY cookie on the Virtuoso host itself.

Usage:
    python3 resolve_target.py --vcli /path/to/vcli [--session ID] [--title-prefix ViraStack]
"""

import argparse
import json
import subprocess
import sys


def run(cmd):
    proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return proc.returncode, proc.stdout.decode("utf-8", "replace"), proc.stderr.decode("utf-8", "replace")


def vcli_json(vcli, session, args):
    cmd = [vcli]
    if session:
        cmd += ["--session", session]
    cmd += args
    rc, out, err = run(cmd)
    if rc != 0:
        raise RuntimeError("%s failed: %s %s" % (" ".join(args), out.strip(), err.strip()))
    return json.loads(out)


def resolve_session(vcli, session):
    if session:
        return session
    data = vcli_json(vcli, None, ["session", "list", "--format", "json"])
    sessions = data.get("sessions") or []
    if not sessions:
        raise RuntimeError("no live vcli session — start virtuoso-daemon first")
    if len(sessions) > 1:
        raise RuntimeError(
            "multiple sessions (%s) — pass --session explicitly"
            % ", ".join(s["id"] for s in sessions)
        )
    return sessions[0]["id"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--vcli", required=True)
    ap.add_argument("--session", default="")
    ap.add_argument("--title-prefix", default="ViraStack")
    ap.add_argument("--display", default="")
    args = ap.parse_args()

    try:
        session = resolve_session(args.vcli, args.session)
        data = vcli_json(args.vcli, session, ["window", "list-windows-x11", "--format", "json"])
    except Exception as exc:  # noqa: BLE001 — report and exit non-zero
        print(json.dumps({"error": str(exc)}))
        return 1

    candidates = [
        w for w in (data.get("windows") or [])
        if (w.get("title") or "").startswith(args.title_prefix)
    ]
    if not candidates:
        print(json.dumps({
            "error": "no window titled %r — is ViraStack running?" % args.title_prefix,
            "session": session,
        }))
        return 1

    chosen = candidates[0]
    print(json.dumps({
        "session": session,
        "display": args.display or data.get("display"),
        "window_id": chosen.get("window_id"),
        "pid": chosen.get("pid"),
        "title": chosen.get("title"),
        "alternatives": [w.get("window_id") for w in candidates[1:]],
    }, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
