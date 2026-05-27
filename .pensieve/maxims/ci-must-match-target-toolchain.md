---
id: ci-must-match-target-toolchain
type: maxim
title: CI Must Match Target Toolchain
status: active
created: 2026-05-27
updated: 2026-05-27
tags: ["maxim"]
---

# CI Must Match Target Toolchain

## One-line Conclusion
> Always run CI lints on the same toolchain version users will compile with; local nightly passes do not prove stable passes.

## Guidance
- `cargo fix` and `cargo clippy --fix` skip lints not present in the current rustc — nightly may lack lints that stable has (and vice versa)
- After running auto-fix locally, verify the result against CI's toolchain version before pushing
- Pin CI to `stable` (not nightly) unless the project requires nightly features

## Boundaries
- Does not apply if the project intentionally targets nightly-only

## Context Links
- Based on: CI clippy failures after local `cargo fix` on nightly 1.96 vs stable 1.94
- Related: [[skill-nil-is-not-an-error]]
