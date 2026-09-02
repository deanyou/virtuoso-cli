# PR Review Hardening — Workflow Reference

Detailed recipes and templates for the three-phase workflow in `SKILL.md`.
Load this file when executing the skill; keep it out of the main SKILL.md body.

---

## Phase 1 — Corrected-review template

Produce this BEFORE editing. Paste it back to the user so wrong claims are
visible before any code changes.

```markdown
## Corrected review — <PR/branch>

**Verdict:** Request-changes / Approved-with-nits / Accurate
**Scope:** src/transport/x11.rs only (the screenshot path). Files outside this
diff are flagged out-of-scope and raised separately.

### Original item #N — <one-line summary>
- Status: Confirmed | False-positive | Out-of-scope | Missed
- Evidence: src/transport/x11.rs:1340 — `std::fs::metadata` follows the link,
  so `is_symlink()` can never fire. Fix: `symlink_metadata`.
- Action: <what will be done / was wrong in the claim>

### New finding (reviewer missed)
- src/transport/x11.rs:977 — the fetch-error arm returns before cleaning the
  local /tmp PNG written by `import`. Leaks a temp file in local mode.
```

Severity legend used in this repo:
`🔴 Blocking` / `🟠 High` / `🟡 Important` / `⚪ Minor` / `✅ Done-well`.

Key checks (from SKILL.md Phase 1):
- **Existence** — read the real file; grep the symbol; don't trust line numbers
  copied from a stale read.
- **False-positive** — grep the whole file incl. the bottom test module before
  accepting "no test / missing" claims.
- **Scope** — `git diff --stat BASE...HEAD`; items outside the diff are
  out-of-scope, raise separately.
- **Missed items** — record real bugs the review omitted.
- **Blast radius** — rate severity from the *merged* data flow, not the worst
  theoretical case.

---

## Phase 2 — Single-seam fix commands

```bash
# One branch from the target base (verify HEAD first)
git fetch --quiet origin
git checkout -b fix/TOPIC origin/main

# Edit one concern, then gate BEFORE committing:
cargo fmt --check            # if it fails: cargo fmt && cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --lib MODULE    # targeted, not full suite, per seam

# Commit one seam; write body to /tmp to survive fish shell
cat > /tmp/msg.txt <<'EOF'
#3 LocalTransport::download_file: implement as std::fs::copy(remote, local)

VB_REMOTE_HOST unset selects LocalTransport, which returned
UnsupportedOperation from download_file, breaking local-mode screenshots.
EOF
git add src/transport/x11.rs
git commit -F /tmp/msg.txt

# Push + open PR with body from file (fish-safe)
git push -u origin fix/TOPIC
gh pr create --base main --head fix/TOPIC \
  --title "fix(x11): TOPIC" --body-file /tmp/pr_body.md
```

Seam-commit checklist (per commit):
- [ ] Exactly one concern (one review item, or one test group).
- [ ] No unrelated refactor / formatting mixed in.
- [ ] `cargo fmt --check` / `clippy -D warnings` / targeted test pass.
- [ ] Message names the review item ID it resolves.
- [ ] `git diff --stat BASE...HEAD` shows only intended files.

---

## Phase 3 — Real-CI verification commands

```bash
# Find runs for the PR branch
gh run list --branch fix/TOPIC --limit 5

# Watch both jobs to completion (Integration Matrix is the 6-job one)
gh run watch MATRIX_RUN_ID
gh run watch CI_RUN_ID

# Or poll conclusion directly
gh run view RUN_ID --json conclusion -q .conclusion
# → "success" / "failure"

# After merge, confirm main (not just the branch)
git fetch --quiet origin
gh run list --branch main --limit 4
gh run watch POST_MERGE_RUN_ID

# Verify a squash-merged commit touched only intended files
git show --stat MERGED_SHA
```

Post-merge rules:
- A green PR branch does **not** guarantee green `main` (matrix × merge
  interactions). Always re-verify `main`'s runs after `gh pr merge`.
- `gh run view --json conclusion` is the source of truth; the GitHub UI
  "checks" rollup can lag.

---

## Common clippy `-D warnings` traps

- `needless_borrows_for_generic_args`:
  `CommandRequest::untimed(&format!(...))` → `untimed(format!(...))`.
- `unused_borrows`: a `let x = &y;` that is never reborrowed — drop the `&`.
- After editing, a linter/hook may reformat the file; re-read before the next
  Edit so `old_string` still matches.

---

## Git-session safety (concurrent automation)

If a background automation switches branches or stashes mid-session:

```bash
# Diagnose
git branch --show-current
git stash list
git rev-parse HEAD
git merge-base --is-ancestor $(git rev-parse stash@{N}^1) HEAD \
  && echo "stash parent is ancestor of HEAD"

# Verify a stash is redundant BEFORE dropping (non-destructive)
git stash apply stash@{N}
git diff HEAD --stat          # empty => content already in HEAD
git status --short | grep -v '.workbuddy'
# If empty and no tracked changes: redundant, safe to drop
git stash drop stash@{N}

# If apply created a duplicate (e.g., duplicate `pub mod sys;`):
git restore FILE            # undo the duplicate, keep HEAD version
git stash drop stash@{N}
```

Never `git stash drop` before a non-destructive `git stash apply` + diff check.
Preserve unrelated stashes (old sessions) untouched.
