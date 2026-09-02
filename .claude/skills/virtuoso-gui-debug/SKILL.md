---
name: virtuoso-gui-debug
description: Replayable Virtuoso GUI debugging via strict JSON DSL with fake and live executors — live mode binds session/PID/DISPLAY and routes all GUI input through vcli window action-x11
allowed-tools: Bash(python3 *) Read
---

# Virtuoso GUI Debug Skill

## Purpose

This skill provides deterministic, replayable Virtuoso GUI debugging via a strict JSON DSL. It parses and validates scenarios, executes them through a fake executor (offline, deterministic) or a live executor (real `vcli`, bound to session/PID/DISPLAY with an exclusive DISPLAY lock), and writes machine-readable evidence files.

Two execution engines:

- `--executor fake` — offline-only, deterministic, for regression tests and automation logic verification. No subprocess side effects beyond `python3` itself.
- `--executor live` — drives the real `vcli` CLI through a fixed-argv command runner. All GUI input goes through `vcli window action-x11`, which re-validates window identity server-side on every action.

## When to Use

- Replaying a validated GUI-debug scenario for regression testing (fake)
- Verifying GUI automation logic without a live Virtuoso environment (fake)
- Executing an already-validated scenario against a real Virtuoso session (live)
- Generating deterministic audit trails for agentic GUI operations

## Prerequisites

Each scenario requires explicit binding of:

| Parameter | Description |
|-----------|-------------|
| `session_id` | Unique session identifier (non-empty string, e.g. `dean-user1-34929`) |
| `pid` | Positive integer process ID |
| `display` | Valid DISPLAY string (e.g., `:0` or `:1.0`) |
| `cellview` | Target cellView in `lib/cell/view` format |

Live mode additionally requires: `--session` (must equal the scenario's `session_id`), `--vcli PATH` (the vcli binary on the Virtuoso host), and `--output DIR` (a fresh output directory). `--ssh-host HOST` is optional; when given, vcli runs over SSH with a safely-quoted fixed argv.

## Usage

**IMPORTANT:** Always validate before running:
```bash
python3 scripts/gui_runner.py validate SCENARIO
```

Run with fake executor (offline):
```bash
python3 scripts/gui_runner.py run SCENARIO --output DIR --executor fake
```

Run with live executor (real vcli):
```bash
python3 scripts/gui_runner.py run SCENARIO --output DIR \
    --executor live --session dean-user1-34929 \
    --vcli /usr/local/bin/vcli [--ssh-host compute-eda-42]
```

## Live-Mode Contract (fail-closed rules)

Before any GUI input is sent, precheck verifies in order:

1. the session exists in `vcli session list` and its bridge port matches the session id's trailing number;
2. the session PID is positive — a zero PID (old bridge metadata) falls back to the scenario PID via window discovery, and is rejected if no unique window binds to it;
3. the DISPLAY reported by the X server matches the scenario exactly;
4. exactly one window is bound to the PID on that DISPLAY — zero or multiple matches abort;
5. an exclusive lock on the DISPLAY (lock file under `DIR/locks/`) is acquired and held for the whole run.

Every GUI action (`KEY`, `TYPE`, `CLICK_REL`, `DRAG_REL`, `WINDOW_ACTIVATE`, `SCREENSHOT`) maps to a fixed `vcli window action-x11` argv carrying the resolved window id, PID, and DISPLAY — the Rust side re-resolves and re-validates the window before sending input. `verify` prefers database-first predicates via vcli; only visibility predicates use the X11 window list. `recover` executes only rollback operations that pass scenario validation.

Typed input text never appears in error payloads or logs — it is replaced by `text_length` markers.

Failures close the run: there is no fallback to "first title-matched window", root-window coordinates, or unbound xdotool calls.

## Output Files

Each run writes to the caller-specified output directory:

| File | Description |
|------|-------------|
| `task.json` | Validated scenario snapshot |
| `agent-actions.jsonl` | Append-only event log |
| `summary.json` | Final pass/fail with error details |
| `locks/` | DISPLAY lock files (live mode) |
| `window_<id>.png` | Screenshots (live mode) |

## Allowed Operations

Only these operations are permitted:

- `VCLI_LOAD` / `VCLI_CALL` — accepted by the schema; **not executable** by the live executor (fail-closed) 
- `WINDOW_WAIT` — poll window state until the requested condition or timeout
- `WINDOW_ACTIVATE` — activate window
- `KEY` — send key event
- `TYPE` — type text
- `CLICK_REL` — relative click
- `DRAG_REL` — relative drag
- `SCREENSHOT` — capture screenshot
- `VERIFY` — verify state
- `RECOVER` — recovery action

## Constraints

- Unknown fields are REJECTED (strict schema enforcement)
- Timeouts must be 1–300 seconds
- Retries must be 0 or 1
- Every action requires a verifier
- Fake executor performs no shell, vcli, X11, xdotool, or live process execution
- Live executor only runs the fixed vcli argv through the injected command runner — never ssh/xdotool/xprop/shell directly
- Live runs require an explicit fresh `--output` directory; nothing is written outside it

## Testing

```bash
python3 -m unittest tests.test_cli tests.test_command_runner \
    tests.test_live_executor tests.test_engine tests.test_model
```

## Schema Reference

See `references/scenario-schema.md` for the complete JSON DSL specification.
