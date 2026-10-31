#!/usr/bin/env bash
# probe.sh — run one vcli command and capture the full proof set.
#
# Usage:
#   probe.sh <EVIDENCE_DIR> <FEATURE_SLUG> -- <vcli args...>
#
# Example:
#   probe.sh "$EVIDENCE" config-bootstrap -- init
#   probe.sh "$EVIDENCE" cell-open-dry-run -- cell open --lib demo --cell top --dry-run
#
# Writes into <EVIDENCE_DIR>/<FEATURE_SLUG>/:
#   action.txt        the exact command that was issued
#   stdout.txt        raw stdout
#   stdout.json       stdout parsed as JSON (only when it is JSON)
#   stderr.txt        raw stderr
#   exit-code.txt     vcli's semantic exit code
#   side-effects.txt  files that appeared / changed in the data dir
#   vbhome-path.txt   the disposable VB_HOME this probe used
#
# The probe never touches the user's real home. If VB_HOME is already
# exported it is used as-is (that is the run's data dir); otherwise a
# throwaway one is created under /tmp and named in vbhome-path.txt.
#
# Exit code: vcli's own exit code, so it composes in a pipeline.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_REPO="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

if [[ $# -lt 3 ]]; then
  echo "usage: probe.sh <EVIDENCE_DIR> <FEATURE_SLUG> -- <vcli args...>" >&2
  exit 2
fi

EVIDENCE_DIR="$1"
FEATURE_SLUG="$2"
shift 2
if [[ "${1:-}" == "--" ]]; then
  shift
fi
if [[ $# -eq 0 ]]; then
  echo "probe.sh: no vcli arguments given after --" >&2
  exit 2
fi

# Allow an explicit repo via env, else derive it from the skill location.
REPO="${VCLI_REPO:-$DEFAULT_REPO}"
VCLI="$REPO/target/debug/vcli"
if [[ ! -x "$VCLI" ]]; then
  echo "probe.sh: vcli binary not found at $VCLI (run: cargo build)" >&2
  exit 2
fi

OUT_DIR="$EVIDENCE_DIR/$FEATURE_SLUG"
mkdir -p "$OUT_DIR" || exit 2

CREATED_VB_HOME=0
if [[ -z "${VB_HOME:-}" ]]; then
  VB_HOME="$(mktemp -d /tmp/vcli-probe-XXXXXX)"
  CREATED_VB_HOME=1
fi
export VB_HOME
printf '%s\n' "$VB_HOME" >"$OUT_DIR/vbhome-path.txt"

snapshot() {
  if [[ -d "$VB_HOME" ]]; then
    (cd "$VB_HOME" && find . -type f 2>/dev/null | sort)
  fi
}

snapshot >"$OUT_DIR/.before"

printf '%s\n' "$VCLI --format json $*" >"$OUT_DIR/action.txt"

"$VCLI" --format json "$@" >"$OUT_DIR/stdout.txt" 2>"$OUT_DIR/stderr.txt"
RC=$?

printf '%s\n' "$RC" >"$OUT_DIR/exit-code.txt"

# stdout.json only when stdout really is JSON. Remove any stale copy
# first: OUT_DIR is reused per feature slug, so a previous run's success
# JSON must never survive as "evidence" for a run that produced no JSON.
rm -f "$OUT_DIR/stdout.json"
python3 - "$OUT_DIR/stdout.txt" "$OUT_DIR/stdout.json" <<'PY' 2>/dev/null
import json, sys
raw = open(sys.argv[1], encoding="utf-8", errors="replace").read()
try:
    parsed = json.loads(raw)
except Exception:
    sys.exit(1)
with open(sys.argv[2], "w", encoding="utf-8") as f:
    json.dump(parsed, f, indent=2, sort_keys=True)
    f.write("\n")
PY

# Side effects: what changed in the data dir across this one command.
snapshot >"$OUT_DIR/.after"
{
  echo "# side effects observed in $VB_HOME across this command"
  echo "# action: $VCLI --format json $*"
  echo "# NOTE: this diff covers file additions and removals only; a change to"
  echo "#       the contents of an existing file is invisible here and must be"
  echo "#       checked manually."
  echo
  echo "## files added"
  comm -13 "$OUT_DIR/.before" "$OUT_DIR/.after" | sed 's/^/  + /'
  echo
  echo "## files removed"
  comm -23 "$OUT_DIR/.before" "$OUT_DIR/.after" | sed 's/^/  - /'
  echo
  echo "## command history tail (append-only log of every vcli invocation)"
  if [[ -f "$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl" ]]; then
    tail -n 3 "$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl" | sed 's/^/  /'
  else
    echo "  (no history file at \$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl)"
  fi
} >"$OUT_DIR/side-effects.txt"
rm -f "$OUT_DIR/.before" "$OUT_DIR/.after"

# A throwaway VB_HOME is scratch state; the caller still owns cleanup of
# the run's data dir. Remove only the one this probe invented.
if [[ "$CREATED_VB_HOME" == "1" ]]; then
  rm -rf "$VB_HOME"
  printf '%s\n' "(removed throwaway VB_HOME)" >"$OUT_DIR/vbhome-path.txt"
fi

echo "probe: $FEATURE_SLUG -> $OUT_DIR (exit $RC)"
exit "$RC"
