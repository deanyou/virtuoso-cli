#!/bin/bash
# Package the CentOS 7 build for offline transfer.
#   ./package-centos7.sh <outdir> [prefix]
# Produces <outdir>/vcli-centos7-<version>/ containing:
#   bin/{vcli,vtui,virtuoso-daemon}
#   share/virtuoso-cli/ramic_bridge.il   (with __DAEMON_PATH__ baked)
#   install-offline.sh                     (target-side installer)
#   SHA256SUMS
set -euo pipefail

OUTDIR="${1:?usage: package-centos7.sh <outdir> [prefix]}"
PREFIX="${2:-/opt/virtuoso-cli}"
BIN_SRC="${3:-/tmp/vcli-centos7-out}"
REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"

VERSION="$(grep -m1 '^version' "$REPO/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"
PKG="$OUTDIR/vcli-centos7-$VERSION"
rm -rf "$PKG"
mkdir -p "$PKG/bin" "$PKG/share/virtuoso-cli"

for b in vcli vtui virtuoso-daemon; do
    [ -x "$BIN_SRC/$b" ] || { echo "missing binary: $BIN_SRC/$b" >&2; exit 1; }
    install -m 0755 "$BIN_SRC/$b" "$PKG/bin/$b"
done

# Bake the daemon path into the bridge (same substitution install.sh does).
sed "s|__DAEMON_PATH__|$PREFIX/bin/virtuoso-daemon|g" \
    "$REPO/resources/ramic_bridge.il" > "$PKG/share/virtuoso-cli/ramic_bridge.il"
if grep -q '__DAEMON_PATH__' "$PKG/share/virtuoso-cli/ramic_bridge.il"; then
    echo "ERROR: unresolved __DAEMON_PATH__ in baked bridge" >&2
    exit 1
fi
chmod 0644 "$PKG/share/virtuoso-cli/ramic_bridge.il"

cat > "$PKG/install-offline.sh" <<EOF
#!/bin/bash
# Target-side installer for CentOS 7. Run as root:
#   ./install-offline.sh
set -euo pipefail
PREFIX="$PREFIX"
HERE="\$(cd -- "\$(dirname -- "\${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"

mkdir -p "\$PREFIX/bin" "\$PREFIX/share/virtuoso-cli"
for b in vcli vtui virtuoso-daemon; do
    install -m 0755 "\$HERE/bin/\$b" "\$PREFIX/bin/\$b"
    echo "installed \$PREFIX/bin/\$b"
done
install -m 0644 "\$HERE/share/virtuoso-cli/ramic_bridge.il" "\$PREFIX/share/virtuoso-cli/ramic_bridge.il"
echo "installed \$PREFIX/share/virtuoso-cli/ramic_bridge.il"

cat > /etc/profile.d/vcli.sh <<'PROF'
export PATH="$PREFIX/bin:\$PATH"
PROF
echo "installed /etc/profile.d/vcli.sh"

echo
echo "Each user adds this to ~/.cdsinit:"
echo "    load(\"\$PREFIX/share/virtuoso-cli/ramic_bridge.il\")"
echo
echo "Verify:"
echo "    \$PREFIX/bin/vcli --version"
echo "    \$PREFIX/bin/virtuoso-daemon --version"
EOF
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
