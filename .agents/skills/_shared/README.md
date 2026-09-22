# `_shared/` — the wrapper, conventions, and shared scripts

This directory is the **adapter layer** between the `.claude/skills/`
document corpus and the `vcli` Rust binary. Anything that is *cross-skill*
lives here; anything that belongs to a single skill lives in that skill's own
subdirectory.

| File               | Purpose                                                       |
| ------------------ | ------------------------------------------------------------- |
| `vcli.sh`          | Shell adapter. Accepts both `virtuoso` and `vcli` invocations, logs stderr, forwards the binary unchanged. |
| `convention.md`    | Naming contract (`virtuoso` vs `vcli`), install instructions, single-point migration rules. |
| `scripts/`         | Cross-skill Python helpers (e.g. `signal_matcher.py`). Skills reference `${CLAUDE_SKILL_DIR}/scripts/` for skill-local scripts; only put files here when 2+ skills need them. |

## One-time install

```bash
SHARED="$(pwd)/.claude/skills/_shared/vcli.sh"
mkdir -p ~/.local/bin
ln -sf "$SHARED" ~/.local/bin/vcli
ln -sf "$SHARED" ~/.local/bin/virtuoso
export PATH="$HOME/.local/bin:$PATH"

virtuoso doctor   # sanity-check
```

After install, every `virtuoso ...` example in the SKILL.md corpus Just Works
without further changes. If you remove the wrapper, those examples will
silently break — see `convention.md` for the migration contract.

## Editing rules

- Files in this directory affect **every** skill that calls the CLI. A change
  here is a "client SDK release". Anything that only one skill needs does
  *not* belong here.
- Keep `vcli.sh` a thin wrapper. Do not add JSON reformatting, retry logic,
  or argument rewriting — those belong in skill-local scripts.
- `convention.md` is the only file in this repo that owns the name mapping.
  Update it (and `vcli.sh`) atomically when renaming.
