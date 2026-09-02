---
name: pr-review-hardening
description: "Verify the accuracy of a GitHub PR review (or a pasted review draft) against the actual code before acting, fix the confirmed bugs one seam at a time, and confirm with the real CI run rather than trusting local-green. Use when the user hands over a PR review and asks to fix the review comments, apply these changes, review this and merge, or confirm whether a review is accurate. This skill applies when the task involves acting on a code review, a GitHub PR, the gh CLI, CI verification, or single-seam commit discipline."
agent_created: true
---

# PR Review Hardening

## Overview

Turn a submitted PR review into a trustworthy, merge-ready change set. The
default failure mode is to blindly apply every review comment — including
false positives, out-of-diff items, and severity over-ratings — then declare
victory on a green local test run that the real CI later rejects. This skill
enforces three gates: **verify accuracy first**, **fix one seam per commit**,
and **confirm with the actual CI run**, not the local build.

## When to Use

- The user pastes a PR review (their own draft or a teammate's) and asks to act
  on it ("fix these", "apply", "is this accurate?", "review and merge").
- A review is in Request-changes state and the next step is remediation.
- Any task touching a GitHub PR where CI status must be trusted (not assumed).

## Workflow

### Phase 1 — Verify review accuracy before touching code

Do not start editing on the first reading of the review. For each numbered
item, verify against the real repository state:

1. **Existence.** Does the cited bug exist at the cited file/line? Read the
   actual file on the relevant branch/commit; grep the symbol. Line numbers in
   the review may be from a stale read or a different branch.
2. **False-positive.** Did the review claim something is missing when it
   already exists? Reviewers frequently read only part of a large file. Grep
   the whole file (especially the `#[cfg(test)]` / test module at the bottom)
   before accepting "no test / not implemented" claims.
3. **Scope.** Is the item inside this PR's diff? Run
   `git diff --stat BASE..HEAD` (or the PR's merge-base). Files/lines
   outside the diff are out of scope — flag them, do not fix them in this PR.
4. **Missed items.** While reading, record real bugs the review omitted (e.g.,
   an error path that leaks a temp file, a dead-code branch that can never
   trigger). Add them as additional findings.
5. **Blast radius.** Don't inherit the review's severity. A "symlink rejection
   is dead code" sounds like a remote exploit but is defense-in-depth if it
   only validates a locally-fetched file.

Produce a **corrected review** before editing: for each original item mark
`Confirmed` / `False-positive (evidence: …)` / `Out-of-scope (not in diff)` /
`Missed (new finding)`, and list new findings. See
`references/workflow.md` for the template. This protects the user from acting on
wrong claims.

### Phase 2 — Fix one seam per commit

- One commit per concern. Zero behavior change within a seam. The commit
  message references the review item ID, e.g.
  `#3 LocalTransport::download_file → std::fs::copy(remote, local)`.
- Prefer `git commit -F FILE` (and `gh pr create --body-file FILE`): the
  default shell is **fish**, where heredocs, `VAR=x cmd` prefixes, and long
  inline bodies fail. Write bodies to a temp file under `/tmp`.
- Run the acceptance gates per seam (fmt / clippy `-D warnings` / targeted
  test) before committing so a failure is attributable to one change.

### Phase 3 — Confirm with the real CI run (never local-green alone)

Local `cargo test` / `npm test` green is necessary but not sufficient.

1. After opening the PR: `gh run list --branch BRANCH --limit 5` to find the
   run IDs, then `gh run watch RUN_ID` (or
   `gh run view ID --json conclusion -q .conclusion`). Watch BOTH the "CI"
   run (includes security/audit jobs) and the "Integration Matrix" run.
2. After `gh pr merge`, `git fetch` and re-verify `main`'s post-merge runs — a
   green PR branch does not guarantee green `main` (matrix/merge interactions).
3. Verify a squash-merged commit's scope: `git show --stat SHA`. GitHub's
   merge echo can look like it touched many files when it changed one.

## Environment Gotchas

- **Shell is fish.** Avoid multi-line `if`/`for` and heredocs; use `bash` for
  control flow; write message bodies to files and pass `--body-file`.
- **clippy `-D warnings`** flags `needless_borrows_for_generic_args`:
  `untimed(&format!(...))` → drop the `&` → `untimed(format!(...))`.
- **Concurrent automation may switch git branches / stash your work**
  mid-session. Commit early; if preserving a WIP, use a named stash and verify
  non-destructively (`git stash apply` then `git diff HEAD --stat`) BEFORE
  `git stash drop`. If the apply is a no-op (content already in HEAD), the
  stash is redundant and safe to drop.
- **Baseline env failures.** `cargo test` may show a pre-existing,
  env-dependent failure unrelated to the change (e.g., `test_config_timeout_default`
  when `VB_TIMEOUT` is set). Confirm it is pre-existing, not a regression.

## Resources

`references/workflow.md` — corrected-review template, per-phase command
recipes, and a seam-commit checklist.
