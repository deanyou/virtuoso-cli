---
name: virtuoso-gui-debug
description: Replayable Virtuoso GUI debugging via strict JSON DSL with fake, live (vcli), and local (xdotool) executors — unified skill covering remote vcli-driven and direct local X11 GUI automation
allowed-tools: Bash(python3 *) Read
---

# Virtuoso GUI Debug Skill

## Purpose

This skill provides deterministic, replayable Virtuoso GUI debugging via a strict JSON DSL. It parses and validates scenarios, executes them through one of three executors, and writes machine-readable evidence files.

Three execution engines:

- `--executor fake` — offline-only, deterministic, for regression tests and automation logic verification. No subprocess side effects beyond `python3` itself.
- `--executor live` — drives the real `vcli` CLI through a fixed-argv command runner (local or SSH). All GUI input goes through `vcli window action-x11`, which re-validates window identity server-side on every action.
- `--executor local` — direct `xdotool` execution on a local X11 DISPLAY. Binds the target window by PID (or explicit `--window-id`). Supports `SCROLL` (xdotool buttons 4/5/6/7), which vcli does not yet expose. No vcli binary or session required.

## When to Use

- Replaying a validated GUI-debug scenario for regression testing (fake)
- Verifying GUI automation logic without a live Virtuoso environment (fake)
- Executing an already-validated scenario against a real Virtuoso session via vcli (live)
- Direct local X11 automation when vcli is unavailable or scroll/wheel input is needed (local)
- Generating deterministic audit trails for agentic GUI operations
- Quick manual GUI inspection via `scripts/xdotool_cli.py` (env/state/find/shot/click/type/key/drag/scroll/wait/smoke)

## Prerequisites

Each scenario requires explicit binding of:

| Parameter | Description |
|-----------|-------------|
| `session_id` | Unique session identifier (non-empty string, e.g. `dean-user1-34929`) |
| `pid` | Positive integer process ID |
| `display` | Valid DISPLAY string (e.g., `:0` or `:1.0`) |
| `cellview` | Target cellView in `lib/cell/view` format |

Live mode additionally requires: `--session` (must equal the scenario's `session_id`), `--vcli PATH` (the vcli binary on the Virtuoso host), and `--output DIR` (a fresh output directory). `--ssh-host HOST` is optional; when given, vcli runs over SSH with a safely-quoted fixed argv.

Local mode requires: `xdotool` on PATH, `DISPLAY` reachable, and `--output DIR`. `--window-id WID` optionally overrides PID-based window discovery. ImageMagick `import` is required for screenshots.

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

Run with local executor (direct xdotool):
```bash
python3 scripts/gui_runner.py run SCENARIO --output DIR \
    --executor local [--window-id 0x3000006]
```

Quick manual GUI inspection (standalone xdotool CLI):
```bash
python3 scripts/xdotool_cli.py state
python3 scripts/xdotool_cli.py find --name "Library Manager"
python3 scripts/xdotool_cli.py click --x 100 --y 50
python3 scripts/xdotool_cli.py scroll --direction down --count 5
```

## Live-Mode Contract (fail-closed rules)

Before any GUI input is sent, precheck verifies in order:

1. the session exists in `vcli session list` and its bridge port matches the session id's trailing number;
2. the session PID is positive — a zero PID (old bridge metadata) falls back to the scenario PID via window discovery, and is rejected if no unique window binds to it;
3. the DISPLAY reported by the X server matches the scenario exactly;
4. exactly one window is bound to the PID on that DISPLAY — zero or multiple matches abort;
5. an exclusive lock on the DISPLAY (lock file under `~/.cache/virtuoso_bridge/x11-locks/`) is acquired and held for the whole run.

Every GUI action (`KEY`, `TYPE`, `CLICK_REL`, `DRAG_REL`, `WINDOW_ACTIVATE`, `SCREENSHOT`) maps to a fixed `vcli window action-x11` argv carrying the resolved window id, PID, and DISPLAY — the Rust side re-resolves and re-validates the window before sending input. `verify` prefers database-first predicates via vcli; only visibility predicates use the X11 window list. `recover` executes only rollback operations that pass scenario validation.

Typed input text never appears in error payloads or logs — it is replaced by `text_length` markers.

Failures close the run: there is no fallback to "first title-matched window", root-window coordinates, or unbound xdotool calls.

## Local-Mode Contract

Before any GUI input is sent, precheck verifies:

1. `xdotool` is on PATH;
2. the scenario's `DISPLAY` is reachable (`xdotool getdisplaygeometry`);
3. a visible window is bound to the scenario PID — or the explicit `--window-id` is used.

Actions are sent directly via `xdotool` with the bound window activated first. `SCROLL` maps to xdotool mouse buttons 4 (up), 5 (down), 6 (left), 7 (right). Screenshots use ImageMagick `import -window <id>`.

## Output Files

Each run writes to the caller-specified output directory:

| File | Description |
|------|-------------|
| `task.json` | Validated scenario snapshot |
| `agent-actions.jsonl` | Append-only event log |
| `summary.json` | Final pass/fail with error details |
| `baseline.png` | Baseline screenshot (local mode) |
| `window_<id>.png` | Screenshots (live/local mode) |

## Allowed Operations

Only these operations are permitted:

- `VCLI_LOAD` / `VCLI_CALL` — accepted by the schema; **not executable** by live or local executors (fail-closed)
- `WINDOW_WAIT` — poll window state until the requested condition or timeout
- `WINDOW_ACTIVATE` — activate window
- `WINDOW_DISCOVER` — discover/filter windows (title/class/pid filters)
- `DISMISS_DIALOG` — dismiss a dialog (vcli dismiss-dialog / xdotool Escape)
- `CLOSE` — close a window
- `KEY` — send key event
- `TYPE` — type text
- `CLICK_REL` — relative click (window-relative coordinates)
- `DRAG_REL` — relative drag (window-relative vector)
- `SCROLL` — scroll wheel at window-relative position (directions: up/down/left/right, optional count 1-100; live mode via vcli scroll, local mode via xdotool buttons 4/5/6/7)
- `SCREENSHOT` — capture screenshot
- `VERIFY` — verify state (predicates: window_exists, state_matches, title_matches, geometry_matches)
- `RECOVER` — recovery action (auto-dismiss for KEY/TYPE/CLICK_REL when no rollback)

## Constraints

- Unknown fields are REJECTED (strict schema enforcement)
- Timeouts must be 1–300 seconds
- Retries must be 0 or 1
- Every action requires a verifier
- Fake executor performs no shell, vcli, X11, xdotool, or live process execution
- Live executor only runs the fixed vcli argv through the injected command runner — never ssh/xdotool/xprop/shell directly
- Local executor calls xdotool directly but only after precheck binds a specific window
- Live runs require an explicit fresh `--output` directory; nothing is written outside it

## Testing

```bash
python3 -m unittest tests.test_cli tests.test_command_runner \
    tests.test_live_executor tests.test_local_executor \
    tests.test_engine tests.test_model
```

## Schema Reference

See `references/scenario-schema.md` for the complete JSON DSL specification.
See `references/xdotool-cheatsheet.md` for xdotool command reference (local mode).
