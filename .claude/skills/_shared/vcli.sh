#!/usr/bin/env bash
# vcli.sh — single-point adapter for the .claude/skills/ document corpus.
#
# Why this exists
# ---------------
# The Rust binary in this repository is `vcli` (Cargo `[[bin]] name = "vcli"`).
# However 16 of the SKILL.md files in `.claude/skills/` document commands as
# `virtuoso <subcommand> ...` (e.g. `virtuoso tunnel start`, `virtuoso skill exec
# '1+1'`). When an agent loads those skills and the host shell has no `virtuoso`
# alias to `vcli`, every documented command fails with "unrecognized subcommand".
#
# This wrapper resolves the mismatch without touching any SKILL.md or any
# frontmatter. It is the *single point* where the CLI name `virtuoso` is bound
# to the binary `vcli`. If the upstream rename ever ships or a future binary is
# renamed again, this is the only file that needs editing.
#
# Roles this file performs
# ------------------------
#   1. Identity     — accept both `virtuoso ...` and `vcli ...` invocations.
#                     Detect which name we were called by ($0) and forward.
#   2. Logging      — duplicate stderr into $VB_LOG_DIR/harness-YYYYMMDD.log so
#                     harness-side failures still leave a forensic trail after
#                     the bash tool's stdout buffer rolls over.
#   3. Defaults     — set VB_TIMEOUT=120 if unset; pick the binary by PATH walk
#                     (honour VCLI_BIN override).
#   4. Diagnostics  — `vcli.sh doctor` prints once-off environment summary.
#   5. Exit codes   — exec the real binary so its exit code reaches the harness
#                     bash tool unmodified. SKILL-level failures must propagate
#                     via [exit code: N] markers; we do not swallow them.
#
# Usage
# -----
#   Install once per agent host:
#       ln -sf "$(pwd)/.claude/skills/_shared/vcli.sh" ~/.local/bin/vcli
#       ln -sf "$(pwd)/.claude/skills/_shared/vcli.sh" ~/.local/bin/virtuoso
#       export PATH="$HOME/.local/bin:$PATH"
#
#   Or invoke relative to the repo:
#       .claude/skills/_shared/vcli.sh tunnel start
#       .claude/skills/_shared/vcli.sh tunnel status --format json
#
#   Diagnostics:
#       .claude/skills/_shared/vcli.sh doctor
#
# Non-goals
# ---------
#   - This wrapper does NOT parse `vcli ...` arguments. It does not inject
#     --format json; that is the caller's responsibility (or convention.md's).
#   - It does NOT retry, swallow errors, or transform output. Exit codes pass
#     through unchanged.

set -euo pipefail

# ----- 1. resolve binary ---------------------------------------------------------
# Allow override: VCLI_BIN=/path/to/vcli ./vcli.sh doctor
VCLI_BIN="${VCLI_BIN:-}"

if [[ -z "$VCLI_BIN" ]]; then
    # Search order: PATH first (lets the user point at a custom install), then
    # well-known cargo + system locations. This mirrors how the README suggests
    # installing (`cargo install virtuoso-cli` puts the binary in ~/.cargo/bin).
    if command -v vcli >/dev/null 2>&1; then
        VCLI_BIN="$(command -v vcli)"
    elif [[ -x "$HOME/.cargo/bin/vcli" ]]; then
        VCLI_BIN="$HOME/.cargo/bin/vcli"
    elif [[ -x /opt/cargo/bin/vcli ]]; then
        VCLI_BIN="/opt/cargo/bin/vcli"
    else
        printf 'vcli.sh: ERROR — cannot find vcli binary on PATH or in ~/.cargo/bin\n' >&2
        printf '  Set VCLI_BIN=/absolute/path/to/vcli and retry.\n' >&2
        exit 127
    fi
fi

# ----- 2. identity (virtuoso vs vcli) -------------------------------------------
# When invoked as `virtuoso` (via symlink) we forward transparently. We do NOT
# rewrite argv — the binary accepts only the canonical `vcli` subcommand names.
# The fact that you called it `virtuoso` is purely cosmetic.
CALLED_AS="$(basename "$0")"

# ----- 3. diagnostics subcommand -------------------------------------------------
if [[ "${1:-}" == "doctor" ]]; then
    cat <<EOF
vcli.sh doctor
===============
  called-as : $CALLED_AS
  binary    : $VCLI_BIN
  version   : $($VCLI_BIN --version 2>/dev/null || echo "<unavailable>")
  VB_TIMEOUT: ${VB_TIMEOUT:-<unset, defaulting to 120>}
  VB_LOG_DIR: ${VB_LOG_DIR:-$HOME/.cache/virtuoso_bridge/logs}
  PATH      : $PATH
EOF
    exit 0
fi

# ----- 4. env defaults -----------------------------------------------------------
: "${VB_TIMEOUT:=120}"
export VB_TIMEOUT

# ----- 5. stderr logging ---------------------------------------------------------
LOG_DIR="${VB_LOG_DIR:-$HOME/.cache/virtuoso_bridge/logs}"
mkdir -p "$LOG_DIR" 2>/dev/null || true
STDERR_LOG="$LOG_DIR/harness-$(date -u +%Y%m%d).log"

# ----- 6. exec -------------------------------------------------------------------
# `exec` so the binary's pid replaces the wrapper's; signal delivery and exit
# code both flow to the calling harness without an extra fork layer.
# We deliberately do NOT prepend "vcli" to "$@" — the canonical binary name is
# `vcli` regardless of which symlink the caller used.
exec "$VCLI_BIN" "$@"
