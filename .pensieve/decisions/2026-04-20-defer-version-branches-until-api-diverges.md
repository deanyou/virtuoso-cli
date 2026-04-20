# Defer version-specific code branches until API actually diverges

## One-line Conclusion
> Add IC23/IC25 branching only after observing a concrete API incompatibility — not in anticipation.

## Context Links
- Based on: [[maestro-session-types]]
- Related: [[2026-04-19-standalone-spectre-for-one-off-verification]]

## Context

During PR #4 review and subsequent refactoring (2026-04-20), version-aware branches were added
to `get_analyses` and `set_analysis` based on IC25 documentation. IC25.1 ISR4 was available
in the test environment.

## Problem

Every `get_analyses` call made 2 extra SKILL RTTs for version detection (`getVersion(t)` +
possible fallback), even though `is_ic25()` always returned `false` — making the IC25 branch
permanently unreachable dead code.

## Alternatives Considered

- **Keep the branch, pay the cost**: adds 2 RTTs per call, every invocation, for a branch
  that never fires. Not acceptable for hot paths.
- **Hardcode IC23**: works but loses the ability to detect future divergence.
- **Lazy detection (chosen)**: only call `client.version()` in the code path that actually
  needs it (e.g., inside `Some(opts)` branch for `set_analysis`).

## Decision

1. Remove IC25 branches from ops methods until real API divergence is confirmed by testing.
2. When adding version-aware code, call `client.version()` **inside the branch that needs it**,
   not unconditionally at the top of the function.
3. Version infrastructure (`VirtuosoVersion`, `detect_version`, `client.version()`) stays —
   it's cheap to keep as scaffolding.

## Consequence

- `get_analyses` saves 2 SKILL RTTs per call (significant for interactive use).
- `set_analysis` with no `--options` saves 2 SKILL RTTs.
- When IC25 API truly diverges, add the branch at the ops layer with a targeted test.

## Exploration Reduction

- **What to ask less next time**: "Should I add IC25 handling now?" → No. Wait until
  `spectre -version` / `getVersion(t)` output + test shows a different behavior.
- **What to look up less next time**: IC25.1 ISR4 Maestro API — confirmed same as IC23 as of
  2026-04-20. `is_ic25()` intentionally returns false until a real divergence is found.
- **Invalidation condition**: If `maeSetAnalysis` or `maeGetEnabledAnalysis` behavior changes
  in a future IC25 ISR and tests confirm it, add the branch at that point.
