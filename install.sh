#!/usr/bin/env bash
#
# install.sh — shared / multi-user installer for virtuoso-cli.
#
# Installs vcli, vtui and virtuoso-daemon to a shared or per-user prefix, and
# installs ramic_bridge.il with the daemon path baked in (the __DAEMON_PATH__
# token is substituted to $PREFIX/bin/virtuoso-daemon). One shared install then
# serves every user on the host with zero per-user setup — each user only adds a
# single load(...) line to their ~/.cdsinit.
#
# Usage:
#   ./install.sh [--system | --user | --prefix DIR] [--no-build]
#
#   --system        install to /opt/virtuoso-cli   (shared; needs write perm)
#   --user          install to ~/.local            (per-user; default)
#   --prefix DIR    install to DIR                 (any prefix)
#   --no-build      skip `cargo build`; install the existing target/release bins
#   -h, --help      show this help
#
# Layout created:
#   $PREFIX/bin/{vcli,vtui,virtuoso-daemon}
#   $PREFIX/share/virtuoso-cli/ramic_bridge.il   (with __DAEMON_PATH__ resolved)

set -euo pipefail

# --- locate the repo (this script lives at the crate root) ------------------
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
IL_SRC="$SCRIPT_DIR/resources/ramic_bridge.il"
TARGET_DIR="$SCRIPT_DIR/target/release"
BINS=(vcli vtui virtuoso-daemon)

# --- defaults ---------------------------------------------------------------
PREFIX=""
MODE=""          # system | user | prefix
NO_BUILD=0

die() { printf 'install.sh: error: %s\n' "$*" >&2; exit 1; }

usage() {
    sed -n '3,28p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

# --- parse args -------------------------------------------------------------
while [ $# -gt 0 ]; do
    case "$1" in
        --system)      MODE="system" ;;
        --user)        MODE="user" ;;
        --prefix)      shift; [ $# -gt 0 ] || die "--prefix requires a directory argument"; PREFIX="$1"; MODE="prefix" ;;
        --prefix=*)    PREFIX="${1#--prefix=}"; MODE="prefix" ;;
        --no-build)    NO_BUILD=1 ;;
        -h|--help)     usage 0 ;;
        *)             die "unknown argument: $1 (try --help)" ;;
    esac
    shift
done

# --- resolve prefix ---------------------------------------------------------
case "$MODE" in
    system)  PREFIX="/opt/virtuoso-cli" ;;
    user|"") PREFIX="${PREFIX:-$HOME/.local}"; MODE="${MODE:-user}" ;;
    prefix)  [ -n "$PREFIX" ] || die "empty --prefix" ;;
esac
# Expand a leading ~ if the caller quoted it.
case "$PREFIX" in "~"/*) PREFIX="$HOME/${PREFIX#~/}" ;; esac

BIN_DIR="$PREFIX/bin"
SHARE_DIR="$PREFIX/share/virtuoso-cli"
DAEMON_BIN="$BIN_DIR/virtuoso-daemon"
IL_DEST="$SHARE_DIR/ramic_bridge.il"

printf '==> virtuoso-cli install\n'
printf '    prefix : %s\n' "$PREFIX"
printf '    binaries -> %s\n' "$BIN_DIR"
printf '    bridge   -> %s\n' "$IL_DEST"

# --- build ------------------------------------------------------------------
if [ "$NO_BUILD" -eq 0 ]; then
    command -v cargo >/dev/null 2>&1 || die "cargo not found in PATH; install Rust or pass --no-build"
    printf '==> cargo build --release --features daemon\n'
    ( cd "$SCRIPT_DIR" && cargo build --release --features daemon )
else
    printf '==> --no-build: using existing %s\n' "$TARGET_DIR"
fi

# --- verify build artifacts -------------------------------------------------
for b in "${BINS[@]}"; do
    [ -x "$TARGET_DIR/$b" ] || die "missing built binary: $TARGET_DIR/$b (run without --no-build, or build with: cargo build --release --features daemon)"
done
[ -f "$IL_SRC" ] || die "missing bridge script: $IL_SRC"
grep -q '__DAEMON_PATH__' "$IL_SRC" || printf 'install.sh: warning: __DAEMON_PATH__ token not found in %s; the bridge will fall back to its search chain\n' "$IL_SRC" >&2

# --- install ----------------------------------------------------------------
mkdir -p "$BIN_DIR" "$SHARE_DIR" || die "cannot create $BIN_DIR / $SHARE_DIR (need sudo for a system prefix?)"

printf '==> installing binaries\n'
for b in "${BINS[@]}"; do
    install -m 0755 "$TARGET_DIR/$b" "$BIN_DIR/$b"
    printf '    %s\n' "$BIN_DIR/$b"
done

printf '==> installing bridge (baking __DAEMON_PATH__ = %s)\n' "$DAEMON_BIN"
# '|' delimiter: filesystem paths do not contain it.
sed "s|__DAEMON_PATH__|$DAEMON_BIN|g" "$IL_SRC" > "$IL_DEST"
chmod 0644 "$IL_DEST"
printf '    %s\n' "$IL_DEST"

# --- done -------------------------------------------------------------------
cat <<EOF

==> Done. Each user adds this line to their ~/.cdsinit (or a site-wide .cdsinit):

    load("$IL_DEST")

If $BIN_DIR is not on PATH, add it (e.g. in ~/.bashrc):

    export PATH="$BIN_DIR:\$PATH"

Verify:

    "$BIN_DIR/vcli" --version        # expect 1.3.4
    "$DAEMON_BIN" --version          # expect 1.3.4
EOF
