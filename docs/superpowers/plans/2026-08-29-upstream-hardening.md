# Upstream Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the pulled Maestro execution path, restore strict Clippy compliance, and add an opt-in strict PSF API without breaking legacy parsers.

**Architecture:** Three independent TDD slices are implemented serially. Maestro keeps its current one-call behavior but safely quotes dynamic shell paths and removes diagnostic leakage; lint fixes preserve behavior; strict PSF parsing is additive and returns typed project errors.

**Tech Stack:** Rust 2021, standard library, `thiserror`, existing `regex` and `tempfile` dependencies.

**Spec:** `docs/superpowers/specs/2026-08-29-upstream-hardening-design.md`

## Global Constraints

- Do not add dependencies, CLI commands, environment variables, commits, or pushes.
- Preserve all existing public behavior except the removal of `/tmp/vcli_dec_skill.txt` output.
- All SKILL-bound strings continue through `escape_skill_string()`.
- Use `VirtuosoError`; do not introduce `anyhow`.
- Production changes follow a witnessed RED, then minimal GREEN, then refactor.

---

### Task 1: Harden Maestro `dec` injection

**Files:**
- Modify: `src/client/maestro_ops.rs`
- Modify: `src/commands/maestro.rs`

**Interfaces:**
- Consumes: existing `MaestroOps::run_with_dec(session, analysis_type, options_skill_alist, dec)`.
- Produces: the same signature and simulation flow, with shell-safe dynamic paths and no fixed diagnostic write.

- [ ] **Step 1: Add failing safety tests**

Add unit tests beside existing Maestro tests that generate `run_with_dec` and assert that the constructed SKILL defines and uses a shell-word quoting operation for `netPath`, including apostrophe escaping. The fixed diagnostic write is removed directly in Step 3 and verified during Codex's source review because the command function requires a live Virtuoso client and does not have an injectable filesystem boundary.

- [ ] **Step 2: Witness RED**

Run `cargo test --lib client::maestro_ops` and confirm the new path-safety assertion fails against the pulled implementation.

- [ ] **Step 3: Implement minimal safe quoting**

Add a private generated-SKILL helper that converts every dynamic shell path to a POSIX single-quoted word and escapes embedded apostrophes as `'"'"'`. Use the quoted value in both `sed` path operands and `grep`. Keep `dec` numeric. Delete the fixed `/tmp/vcli_dec_skill.txt` write from `run_with_analysis`.

- [ ] **Step 4: Verify GREEN**

Run `cargo test --lib client::maestro_ops` and require all tests to pass.

### Task 2: Restore Clippy compliance at all four lint sites

**Files:**
- Modify: `src/client/layout_ops.rs`
- Modify: `src/commands/library.rs`
- Modify: `src/client/library_ops.rs`
- Modify: `src/transport/x11.rs`
- Modify: `src/spectre/runner.rs`

**Interfaces:**
- Produces: `XstreamOutRequest<'a>` containing `library`, `top_cell`, `view`, `stream_file`, `layer_map`, `log_file`, and `run_dir`; `LayoutOps::xstream_out(&self, request: &XstreamOutRequest<'_>) -> String`.

- [ ] **Step 1: Add/refactor tests before production signature change**

Change the XStream tests in `src/client/layout_ops.rs` first to construct a literal `XstreamOutRequest` and preserve their hand-written expected escaping assertions. There are no production callers. At this point compilation must fail because the request type/signature does not yet exist.

- [ ] **Step 2: Witness RED**

Run `cargo test --lib client::layout_ops` and confirm the missing type/signature causes the expected failure.

- [ ] **Step 3: Implement the request object and unit-struct cleanup**

Add the request type and update the builder's in-file tests. Replace both `LibraryOps::default()` sites with `LibraryOps`. Collapse the nested condition at `src/transport/x11.rs:367` without changing its error behavior. Replace `assert_eq!(..., false)` at `src/spectre/runner.rs:1825` with `assert!(!...)`. Do not suppress any Clippy lint.

- [ ] **Step 4: Verify GREEN and the lint target**

Run `cargo test --lib client::layout_ops`, then `cargo clippy --all-targets --all-features -- -D warnings`.

### Task 3: Add strict PSF APIs

**Files:**
- Modify: `src/spectre/parsers.rs`
- Modify: `src/spectre/mod.rs` only if re-export is required by the existing module pattern.

**Interfaces:**
- Produces exactly the `PsfDataset`, `read_psf_ascii_strict`, `find_exact_result_file`, `require_scalar`, `require_vector`, and `require_frequency_hz` APIs in the design spec.

- [ ] **Step 1: Add failing dataset accessor tests**

Use literal datasets in the existing test module to cover missing keys, scalar cardinality, empty vectors, `NaN`/infinity, negative frequency, duplicate frequency, descending frequency, and a valid strictly increasing frequency vector.

- [ ] **Step 2: Witness accessor RED**

Run `cargo test --lib spectre::parsers` and confirm compilation fails because `PsfDataset` and its accessors do not exist.

- [ ] **Step 3: Implement minimal accessors**

Add `PsfDataset` and accessors with `VirtuosoError::NotFound` for missing keys and `Execution` for invalid values/cardinality.

- [ ] **Step 4: Verify accessor GREEN**

Run `cargo test --lib spectre::parsers`.

- [ ] **Step 5: Add failing strict-reader and path tests**

With `TempDir`, cover valid simple and `SWEEP` files, malformed numeric lines, empty input, non-finite values, missing files, absolute names, parent traversal, directory targets, symlink targets on Unix, and a valid contained regular file. Each expected value must be a hand-written literal.

- [ ] **Step 6: Witness reader RED**

Run `cargo test --lib spectre::parsers` and confirm failures identify missing strict reader/path behavior.

- [ ] **Step 7: Implement strict reading and containment**

Implement exact relative-path validation, `symlink_metadata` rejection, canonical containment, strict format parsing, finite-number validation, and file-stem dataset naming. Do not route strict parsing through the legacy `Option` helpers because they intentionally discard errors.

- [ ] **Step 8: Verify strict PSF GREEN**

Run `cargo test --lib spectre::parsers` and require all parser tests to pass.

### Task 4: Codex acceptance gate

**Files:**
- Review every changed file; no new production scope.

- [ ] **Step 1: Inspect scope and diff**

Run `git status --short`, `git diff --check`, and `git diff --stat`. Reject commits, generated artifacts, dependency changes, or edits outside the explicit allowlist.

- [ ] **Step 2: Review security and compatibility**

Confirm shell quoting handles apostrophes, `rg "vcli_dec_skill" src` returns no matches, `skill_ok`/`ok_or_exec` semantics are preserved, legacy PSF functions are unchanged, strict missing keys return `NotFound`, and strict path checks return a canonical contained path while rejecting symlinks and escape.

- [ ] **Step 3: Run full verification**

Run, independently and freshly:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

- [ ] **Step 4: Integrate only accepted files**

Use the CCB worktree integration helper after all checks pass, then repeat `git diff --check` and `git status --short` in the primary worktree. Do not commit or push.
