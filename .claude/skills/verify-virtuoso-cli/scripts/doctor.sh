#!/usr/bin/env bash
# doctor.sh — read-only health check for a vcli verification run.
#
# Usage:
#   doctor.sh [REPO_ROOT]
#
# Exit codes:
#   0  tier A drivable (client-side surface is usable)
#   1  not drivable (binary missing, or config does not resolve)
#
# The four probes are the ones named in SKILL.md §2:
#   identity  -> vcli --version
#   config    -> vcli config check
#   sessions  -> vcli session list
#   bridge    -> vcli tunnel diagnose
#
# This script never mutates the target. The only files it produces are
# vcli's own append-only command history, which is the product's design.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_REPO="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
REPO="${1:-$DEFAULT_REPO}"

VCLI="$REPO/target/debug/vcli"
if [[ ! -x "$VCLI" ]]; then
  printf '{"tier_a": "not drivable", "tier_b": "unreachable", "reason": "vcli binary not built at %s", "probes": {}}\n' \
    "$VCLI" >&2
  exit 1
fi

# Use the caller's VB_HOME when set (that is the run's data dir).
# Otherwise make a throwaway one and remove it on exit.
CREATED_VB_HOME=0
if [[ -z "${VB_HOME:-}" ]]; then
  VB_HOME="$(mktemp -d /tmp/vcli-doctor-XXXXXX)"
  CREATED_VB_HOME=1
fi
export VB_HOME

cleanup() {
  if [[ "$CREATED_VB_HOME" == "1" && -n "${VB_HOME:-}" && -d "$VB_HOME" ]]; then
    rm -rf "$VB_HOME"
  fi
}
trap cleanup EXIT

WORK="$(mktemp -d /tmp/vcli-doctor-work-XXXXXX)"

run_probe() {           # run_probe <name> <args...>
  local name="$1"; shift
  "$VCLI" --format json "$@" >"$WORK/$name.out" 2>"$WORK/$name.err"
  echo $? >"$WORK/$name.rc"
}

run_probe identity --version
run_probe config    config check
run_probe sessions  session list
run_probe bridge    tunnel diagnose

VERDICT="$WORK/verdict.json"

python3 - "$WORK" "$VERDICT" "$VCLI" <<'PY'
import json, os, sys

work, out_path, vcli = sys.argv[1], sys.argv[2], sys.argv[3]


def rc(name):
    p = os.path.join(work, name + ".rc")
    try:
        return int(open(p).read().strip())
    except Exception:
        return -1


def out(name):
    p = os.path.join(work, name + ".out")
    try:
        return open(p, encoding="utf-8", errors="replace").read()
    except Exception:
        return ""


def as_json(name):
    try:
        return json.loads(out(name))
    except Exception:
        return None


probes = {}
reasons = []

# --- identity -----------------------------------------------------------
ident = out("identity").strip()
if rc("identity") == 0 and ident:
    probes["identity"] = "ok"
else:
    probes["identity"] = "failed"
    reasons.append("vcli --version failed (exit %s)" % rc("identity"))

# --- config -------------------------------------------------------------
cfg = as_json("config")
if rc("config") == 0 and isinstance(cfg, dict):
    diags = cfg.get("diagnostics") or []
    if diags:
        probes["config"] = "diagnostics"
        reasons.append("config check reported %d diagnostic(s)" % len(diags))
    else:
        probes["config"] = "ok"
else:
    probes["config"] = "failed"
    reasons.append("vcli config check failed (exit %s)" % rc("config"))

# --- sessions -----------------------------------------------------------
sess = as_json("sessions")
if rc("sessions") == 0 and isinstance(sess, dict):
    n = sess.get("count", 0)
    probes["sessions"] = "ok (%d session(s))" % n
else:
    probes["sessions"] = "failed"
    reasons.append("vcli session list failed (exit %s)" % rc("sessions"))

# --- bridge -------------------------------------------------------------
br = as_json("bridge")
if rc("bridge") == 0 and isinstance(br, dict):
    responsive = bool(br.get("daemon_responsive"))
    skill_ok = bool(br.get("skill_eval_ok"))
    if responsive and skill_ok:
        probes["bridge"] = "ok (virtuoso %s)" % (br.get("virtuoso_version") or "unknown")
    else:
        probes["bridge"] = "not reachable"
        reasons.append(
            "bridge daemon not usable (daemon_responsive=%s, skill_eval_ok=%s, port=%s)"
            % (responsive, skill_ok, br.get("port"))
        )
else:
    probes["bridge"] = "failed"
    reasons.append("vcli tunnel diagnose failed (exit %s)" % rc("bridge"))

tier_a = "drivable" if probes.get("identity") == "ok" and probes.get("config") in ("ok",) else "not drivable"
if tier_a != "drivable" and not reasons:
    reasons.append("identity or config probe failed")

tier_b = "drivable" if probes.get("bridge", "").startswith("ok") else "unreachable"

verdict = {
    "tier_a": tier_a,
    "tier_b": tier_b,
    "reason": "; ".join(reasons) if reasons else "client-side surface is usable",
    "probes": probes,
    "vcli": vcli,
}

with open(out_path, "w", encoding="utf-8") as f:
    json.dump(verdict, f, indent=2, sort_keys=True)
    f.write("\n")

print(json.dumps(verdict, sort_keys=True))
PY

VERDICT_RC=$?

# Tier A drivable -> exit 0. The bridge tier is reported, never enforced:
# a host without Virtuoso is a normal result, not a tool failure.
if [[ "$VERDICT_RC" != "0" ]]; then
  echo "doctor.sh: could not assemble verdict" >&2
  exit 1
fi

if grep -q '"tier_a": "drivable"' "$VERDICT"; then
  exit 0
fi
exit 1
