#!/bin/bash
# Package the CentOS 7 build for offline transfer.
#   ./package-centos7.sh <outdir> [bin_src]
# Produces <outdir>/vcli-centos7-<version>/ containing:
#   bin/{vcli,vtui,virtuoso-daemon}
#   share/virtuoso-cli/ramic_bridge.il   (bridge template, __DAEMON_PATH__ token intact)
#   install-offline.sh                     (target-side installer, prefix selectable)
#   SHA256SUMS
#
# The package is prefix-agnostic: the bridge keeps its __DAEMON_PATH__ token and
# install-offline.sh bakes the daemon path at install time, so the same drop
# installs under /opt/virtuoso-cli, ~/.local or any custom prefix.
set -euo pipefail

OUTDIR="${1:?usage: package-centos7.sh <outdir> [bin_src]}"
BIN_SRC="${2:-/tmp/vcli-centos7-out}"
REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"

VERSION="$(grep -m1 '^version' "$REPO/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
PKG="$OUTDIR/vcli-centos7-$VERSION"
rm -rf "$PKG"
mkdir -p "$PKG/bin" "$PKG/share/virtuoso-cli"

for b in vcli vtui virtuoso-daemon; do
    [ -x "$BIN_SRC/$b" ] || { echo "missing binary: $BIN_SRC/$b" >&2; exit 1; }
    install -m 0755 "$BIN_SRC/$b" "$PKG/bin/$b"
done

# Ship the bridge unbaked: the token is resolved by install-offline.sh against
# the prefix actually used on the target machine.
install -m 0644 "$REPO/resources/ramic_bridge.il" "$PKG/share/virtuoso-cli/ramic_bridge.il"
grep -q '__DAEMON_PATH__' "$PKG/share/virtuoso-cli/ramic_bridge.il" \
    || { echo "ERROR: bridge has no __DAEMON_PATH__ token to bake" >&2; exit 1; }

cat > "$PKG/install-offline.sh" <<'INSTALL_EOF'
#!/bin/bash
# Target-side installer for the vcli offline package. Run as root for a shared
# install, no root needed for --user:
#   ./install-offline.sh [--system | --user | --prefix DIR]
#
#   --system        install to /opt/virtuoso-cli   (shared; needs write perm)
#   --user          install to ~/.local            (per-user; no root needed)
#   --prefix DIR    install to DIR                 (any prefix)
#
# The daemon path is baked into the bridge at install time, so this one package
# installs correctly under any prefix.
set -euo pipefail

PREFIX="/opt/virtuoso-cli"
MODE="system"

die() { printf 'install-offline.sh: error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --system)      MODE="system" ;;
        --user)        MODE="user" ;;
        --prefix)      shift; [ $# -gt 0 ] || die "--prefix requires a directory argument"; PREFIX="$1"; MODE="prefix" ;;
        --prefix=*)    PREFIX="${1#--prefix=}"; MODE="prefix" ;;
        -h|--help)     sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)             die "unknown argument: $1 (try --help)" ;;
    esac
    shift
done

if [ "$MODE" = "user" ]; then
    PREFIX="${HOME}/.local"
fi
case "$PREFIX" in
    "~"/*) PREFIX="${HOME}/${PREFIX#~/}" ;;
esac

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
BIN_DIR="$PREFIX/bin"
SHARE_DIR="$PREFIX/share/virtuoso-cli"
DAEMON_BIN="$BIN_DIR/virtuoso-daemon"
IL_DEST="$SHARE_DIR/ramic_bridge.il"

printf '==> vcli offline install\n'
printf '    prefix : %s\n' "$PREFIX"

mkdir -p "$BIN_DIR" "$SHARE_DIR" || die "cannot create $BIN_DIR / $SHARE_DIR (need root for a shared prefix?)"

for b in vcli vtui virtuoso-daemon; do
    install -m 0755 "$HERE/bin/$b" "$BIN_DIR/$b"
    echo "    installed $BIN_DIR/$b"
done

# Bake the daemon path into the bridge so it matches the prefix actually used.
# Bash's ${var//pat/repl} is a *literal* substitution: unlike sed it has no `&`
# ("the whole match") magic and no delimiter, so a prefix containing `&`, `|`
# or `\` — all legal in a Unix path — cannot corrupt the baked path.
# The trailing `X` guard keeps any trailing newlines that $(...) would strip.
IL_CONTENT="$(cat "$HERE/share/virtuoso-cli/ramic_bridge.il"; printf X)"
IL_CONTENT="${IL_CONTENT%X}"
IL_CONTENT="${IL_CONTENT//__DAEMON_PATH__/$DAEMON_BIN}"
printf '%s' "$IL_CONTENT" > "$IL_DEST"
chmod 0644 "$IL_DEST"
# Fail loudly if the token survived: a half-substituted bridge makes the
# deploy-time injection silently no-op, which is exactly what this bakes.
if grep -q '__DAEMON_PATH__' "$IL_DEST"; then
    die "unresolved __DAEMON_PATH__ in $IL_DEST (prefix: $PREFIX)"
fi
echo "    installed $IL_DEST"

if [ "$MODE" = "user" ]; then
    echo "    add to ~/.bashrc:  export PATH=\"$BIN_DIR:\$PATH\""
else
    cat > /etc/profile.d/vcli.sh <<PROF
export PATH="$BIN_DIR:\$PATH"
PROF
    echo "    installed /etc/profile.d/vcli.sh"
fi

echo
echo "Each user adds this line to ~/.cdsinit (or a site-wide .cdsinit):"
echo "    load(\"$IL_DEST\")"
echo
echo "Verify:"
echo "    $BIN_DIR/vcli --version"
echo "    $DAEMON_BIN --version"
INSTALL_EOF
chmod 0755 "$PKG/install-offline.sh"

(
    cd "$PKG"
    if command -v sha256sum >/dev/null 2>&1; then
        find . -type f -not -name SHA256SUMS | sort | xargs sha256sum > SHA256SUMS
    else
        find . -type f -not -name SHA256SUMS | sort | xargs shasum -a 256 > SHA256SUMS
    fi
)

echo "packaged -> $PKG"
ls -lR "$PKG" | head -40
