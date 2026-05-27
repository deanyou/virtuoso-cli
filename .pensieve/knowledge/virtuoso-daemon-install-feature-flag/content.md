---
id: content
type: knowledge
title: virtuoso-daemon Requires `--features daemon` for cargo install
status: active
created: 2026-05-27
updated: 2026-05-27
tags: ["knowledge"]
---

# virtuoso-daemon Requires `--features daemon` for cargo install

## Source
2026-05-02: `cargo install virtuoso-cli` produced a daemon binary that ignored `--version`
and returned `usage: virtuoso-daemon <host> <port>` — identical to the pre-flag binary.
Root cause: daemon target is gated behind the `daemon` feature flag in `Cargo.toml`.

## Summary
Without `--features daemon`, cargo install silently installs the wrong binary — no error,
no warning. The resulting `virtuoso-daemon` has no `--version` and none of the daemon logic.

## Content

### Correct Installation Commands

```bash
# From crates.io:
cargo install virtuoso-cli --features daemon        # virtuoso-daemon
cargo install virtuoso-cli                          # vcli + vtui

# From local source (after code changes):
cargo install --path . --bin virtuoso-daemon --features daemon
cargo install --path . --bin vcli
cargo install --path . --bin vtui
```

### Symptom of Missing Feature Flag

```
$ virtuoso-daemon --version
usage: virtuoso-daemon <host> <port>     ← exit code 1, should print version string
```

Also: `~/.cargo/bin/virtuoso-daemon` file timestamp stays unchanged (cargo skips rebuild
when nothing changed from its perspective, even though the feature set differs).

### Why It's Easy to Miss

`cargo install virtuoso-cli` installs all default-feature binaries (`vcli`, `vtui`).
The daemon is intentionally non-default to avoid shipping it to users who only want
the CLI. But `cargo install` gives no warning that a non-default binary was skipped.

### Verification

```bash
virtuoso-daemon --version   # should print e.g. "0.3.18"
```

If it prints usage instead, reinstall with `--features daemon`.

### Which Binary is Running

```bash
which virtuoso-daemon          # should be ~/.cargo/bin/virtuoso-daemon
ls -la ~/.local/bin/virtuoso-daemon 2>/dev/null  # old manual install may shadow it
```

`~/.local/bin/` may contain an older binary that shadows `~/.cargo/bin/` depending on PATH.

## When to Use
- After `cargo install virtuoso-cli` if daemon fails to start or `--version` returns usage
- When deploying to a new server / updating to a new release
- When `ramic_bridge.il` reports "daemon exited immediately" or port never appears

## Context Links
- Related: [[vcli-bridge-cli-name]] — correct binary naming
