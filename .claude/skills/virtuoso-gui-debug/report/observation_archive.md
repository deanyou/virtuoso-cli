# Observation Period Archive — 2026-09-20

## Baseline
- Scope: IC25.1 / vcli 1.3.5 / low-risk read-only only
- Runs: 50 (real-000 ~ real-049)
- End-to-end success: 49/50 = 98%
- Precondition failure: 1/50 = 2% (real-004: window selector no match)
- Operation failures: 0

## Operation Types vs Scene Variants

9 labels total = 50 runs. "screenshot_*" are variants of one operation.

| Operation type | Variants | Count |
|---|---|---|
| window_list | — | 9 |
| screenshot | CIW / Layout / Schematic | 9+4+3+3 = 19 |
| session_list | — | 8 |
| skill_exec | getCurrentTime / hiGetCurrentWindow / version | 6+4 = 10 |
| lib_list | — | 4 |
| **Total** | | **50** |

## Evidence Closure

150/150 = 3 required events × 50 runs:
1. STARTED (run initiated, session/executor identity)
2. EXECUTE (command dispatched, exit code)
3. VERIFY (postcondition observed, PASSED/FAILED)

"File complete" means all 3 events exist per case.
"Content/identity/hash verified" is NOT claimed — this is structural closure, not semantic proof.

## Regression Constraint (precise)

Window selector no-match (real-004):
- Do NOT execute target action (screenshot not called)
- Do NOT auto-retry on UNKNOWN
- Preserve UNKNOWN + reason (WINDOW_SELECTOR_NO_MATCH)
- This is NOT a global "all UNKNOWN prohibits retry" policy.
  Only applies to selector-no-match precondition failure.

## Out of Scope

- Write operations
- Destructive actions
- Windows live executor
- Long-term reliability guarantee