# aarch64 Cross-Compile Setup

How we build the `virtuoso-daemon-aarch64` binary that ships in
`resources/daemons/` for tunnel deploys to ARM hosts.

## TL;DR

```bash
cargo build --release --bin virtuoso-daemon --features daemon \
  --target aarch64-unknown-linux-gnu
cp target/aarch64-unknown-linux-gnu/release/virtuoso-daemon \
  resources/daemons/virtuoso-daemon-aarch64
```

`~/.cargo/config.toml` points `aarch64-unknown-linux-gnu` at
`/tmp/aarch64-wrapper/aarch64-linux-gnu-gcc`, so the build just works.

## Why a wrapper is needed

The host is Rocky 8 with `gcc-aarch64-linux-gnu` installed (gdc 12.1.1).
Rocky 8's repos do **not** ship `glibc-devel-aarch64-linux-gnu`, so there's
no aarch64 glibc available via `dnf`. We had to build a partial sysroot by
hand:

- `/usr/aarch64-linux-gnu/sys-root/` — Ubuntu 22.04 aarch64 base + libc6-dev

Contents of the sysroot (the bits that matter for linking):

| Path | Source |
|---|---|
| `usr/lib/aarch64-linux-gnu/crt1.o`, `crti.o`, `crtn.o` | libc6-dev .deb |
| `usr/lib/aarch64-linux-gnu/libc.so`, `libc.a` | libc6-dev .deb |
| `usr/lib/aarch64-linux-gnu/libc_nonshared.a` | libc6-dev .deb |
| `usr/lib/aarch64-linux-gnu/libm.so`, `libm.a` | libc6-dev .deb |
| `usr/lib/aarch64-linux-gnu/libpthread.so`, `librt.so`, `libdl.so`, `libutil.so` | libc6-dev .deb |
| `usr/lib/aarch64-linux-gnu/libgcc_s.so.1` + `libgcc_s.so` symlink | libc6-dev .deb |
| `usr/lib/ld-linux-aarch64.so.1` | libc6-dev .deb |

## The "cannot find -lgcc_s" trap

Even with the right files in place under the sysroot, `cargo build` fails with:

```
/usr/bin/aarch64-linux-gnu-ld: cannot find -lgcc_s
```

Root cause: gcc's link line for Rust binaries looks like:

```
gcc <objs> -Wl,-Bstatic <rustlibs> -Wl,-Bdynamic -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc
```

The `-lgcc_s` is asking for the **shared** `libgcc_s.so`. The linker consults
its hardcoded search paths, but for cross-tooling on this host those don't all
resolve cleanly through `--sysroot` alone — particularly the `gcc/<triple>/12/`
compiler-runtime path lookup. The file exists in the sysroot at
`/usr/aarch64-linux-gnu/sys-root/usr/lib/aarch64-linux-gnu/libgcc_s.so.1` (with
a proper `libgcc_s.so -> libgcc_s.so.1` symlink), but the linker still fails.

**Fix:** the wrapper script (`/tmp/aarch64-wrapper/aarch64-linux-gnu-gcc`)
explicitly adds `-L/usr/aarch64-linux-gnu/lib/aarch64-linux-gnu` so the linker
has a guaranteed place to find libgcc_s.so.

This is brittle. If you see the same error again after a toolchain update, the
fix is to confirm:

1. `libgcc_s.so.1` exists in the sysroot's `usr/lib/aarch64-linux-gnu/`
2. The `libgcc_s.so -> libgcc_s.so.1` symlink exists
3. The wrapper passes `-L/usr/aarch64-linux-gnu/lib/aarch64-linux-gnu`

## Rebuilding the sysroot from scratch

If `/usr/aarch64-linux-gnu/sys-root/` ever gets nuked, here's the recipe:

```bash
# 1. Download Ubuntu 22.04 aarch64 base + libc6-dev
mkdir -p /tmp/aarch64-sysroot && cd /tmp/aarch64-sysroot
for pkg in libc6 libgcc-s1 libc6-dev; do
  url=$(curl -s "https://packages.ubuntu.com/jammy/$pkg/download" \
        | grep -oE 'http[^"]*aarch64[^"]*\.deb' | head -1)
  curl -O "$url"
done

# 2. Extract (ar + zstd, because Ubuntu 22.04 uses zstd-compressed data.tar)
for deb in *.deb; do
  ar x "$deb"
  tar -xf data.tar.* -C /tmp/aarch64-sysroot --strip-components=1
  rm -f data.tar.* control.tar.* debian-binary
done

# 3. Install sysroot in the standard location
sudo cp -r /tmp/aarch64-sysroot/* /usr/aarch64-linux-gnu/sys-root/

# 4. Create the libgcc_s.so symlink if missing
cd /usr/aarch64-linux-gnu/sys-root/usr/lib/aarch64-linux-gnu/
[ -e libgcc_s.so ] || ln -s libgcc_s.so.1 libgcc_s.so
```

## What was *not* needed

- `cargo-cross` / Docker — we have the sysroot, no container dance required
- `musl` target — the glibc-based sysroot works
- Recompiling gcc from source — the distro's `gcc-aarch64-linux-gnu` is fine

## Status

| Target | Status | Binary | Size | Built |
|---|---|---|---|---|
| `x86_64-unknown-linux-gnu` | ✅ shipped | `resources/daemons/virtuoso-daemon-x86_64` | 402 KB | v0.4.0-alpha.7 |
| `aarch64-unknown-linux-gnu` | ✅ shipped | `resources/daemons/virtuoso-daemon-aarch64` | 398 KB | v0.4.0-alpha.7 |

Both binaries print `VERSION:0.4.0-alpha.7` on stderr when run on a matching
host.
