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

## GUI Operation Playbook (Multi-Method Matrix)

> Every GUI operation has **at least two stable, independently-verified methods**. If one fails or is unreliable, fall through to the next. All methods below were validated on a real Virtuoso IC25.1 session (DISPLAY=:5.0) with the `ui_dynamic_form.il` dynamic form.

### Critical Environment Constraint

**`vcli skill exec` has NO UI library** — `hiCreateAppForm`, `hiDisplayForm`, `hiGetFieldInfo`, `hiCreateRadioField` are all nil in the daemon exec context. Therefore:

- GUI form creation/display MUST go through the **CIW** (xdotool type into the CIW input line).
- GUI interaction (clicks, typing) MUST go through **xdotool** or **`vcli window action-x11`**.
- Reading form state / setting field values can go through the **CIW** (form object access works there).

### 1. Window Discovery (2+ methods)

| Method | Command | Notes |
|--------|---------|-------|
| **A (recommended)** | `vcli window list-windows-x11 --display :5.0 --format json` | Returns window_id (hex), pid, title, geometry. Server-side validated. |
| **B** | `xdotool search --name "Layer Replace"` | Returns decimal window id (e.g. `39860167` = `0x26037c7`). Usable directly with xdotool. |
| C | `xwininfo -name "title"` | Returns geometry; useful for cross-checking absolute position. |

### 2. Coordinate Acquisition (2+ methods)

| Method | How | Precision |
|--------|-----|-----------|
| **A (recommended): SKILL reverse-engineering** | In CIW: `hiGetFieldInfo(form (quote fieldName))` → returns `((x y) (w h))` in **form-client-relative coordinates**. Field center = `(x + w/2, y + h/2)`. | Exact (±0px) |
| **B: pixel-level crop** | `import -window <wid> out.png` then `convert out.png -crop WxH+X+Y -resize 200%` to visually confirm element position. | Exact after 2 rounds of cross-checking |
| ❌ OCR percentage boxes | Do NOT rely on OCR's relative-percent bounding boxes — drift of ±40px observed across repeated captures of the same window. | Unreliable |

**Coordinate reverse-engineering example** (validated on `udfLayerReplaceForm`):
```skill
hiGetFieldInfo(udfLayerReplaceForm (quote oldLayer))   ; → ((5 150) (590 35))
hiGetFieldInfo(udfLayerReplaceForm (quote newLayer))   ; → ((5 187) (590 35))
hiGetFieldInfo(udfLayerReplaceForm (quote layerOp))    ; → ((5 41) (590 33))
hiGetFieldInfo(udfLayerReplaceForm (quote filePath))   ; → ((5 76) (590 35))
```
Field centers (form-relative): oldLayer=(300,167), newLayer=(300,204), layerOp=(300,57), filePath=(300,93).
Use these directly with `xdotool mousemove --window <wid>` (method 3B) — no xwininfo needed.

### 3. Click Operation (3 methods)

| Method | Command | When to use |
|--------|---------|-------------|
| **A** | `vcli window action-x11 --window-id <hex> --pid <pid> --display :5.0 --operation click-rel --x <cx> --y <cy>` | Need server-side window re-validation; session-bound. |
| **B (recommended, lightweight)** | `xdotool mousemove --window <wid> <cx> <cy>; sleep 0.3; xdotool click 1` | Window-relative coords; no xwininfo/absolute math; works with decimal or hex wid. |
| C | `xdotool mousemove <abs_x> <abs_y>; xdotool click 1` | Only when you already have absolute coords from xwininfo. |

> `cx, cy` are **form-client-relative** coordinates (from method 2A or 2B). For radio buttons inside a field, distribute evenly across the field width.

### 4. Text Input (3 stable methods, 1 unreliable)

| Method | How | Reliability |
|--------|-----|-------------|
| **A (recommended)** | Click/navigate to field, then `xdotool type --clearmodifiers --delay 50 "text"` | ✅ High — validated with "TABTEST", "METAL1" |
| **B (coordinate-free)** | `xdotool key Tab` (repeat to reach target field), then `xdotool type` | ✅ High — 4 Tabs reached Target Layer in the test form |
| **C (most reliable, bypasses GUI)** | In CIW: `form->field->value = "text"` | ✅ Highest — direct object assignment; no focus needed |
| ❌ `vcli window action-x11 --operation type --text` | — | ❌ **Unreliable** — injected garbled/clipboard content instead of specified text on IC25.1. Do not use. |

### 5. Button Submit / Confirm (3 methods)

| Method | How | Notes |
|--------|-----|-------|
| **A** | Click the button (method 3A or 3B) | Works for OK/Apply when `?buttonLayout` callback is correctly bound. |
| **B (recommended for dialogs)** | `xdotool key Return` (with dialog focused) | Equivalent to Open/OK in file dialogs; more reliable than clicking the Open button (which had coordinate-sensitivity issues). |
| C | In CIW: call the callback directly, e.g. `udfApplyCB()` | Bypasses GUI entirely; useful for verifying callback logic independent of button wiring. |

### 6. Close / Cancel (3 methods)

| Method | How | Notes |
|--------|-----|-------|
| **A (recommended)** | `xdotool key Escape` (with window focused) | Dismisses most dialogs; falls back to windowclose if no response. |
| **B** | In CIW: `hiFormCancel(form)` | Clean form dismissal; note: cannot cancel a form that is mapped (returns nil with WARNING). |
| C | Click Cancel button (method 3) | Coordinate-dependent. |

### 7. Modal Dialog Handling (CRITICAL)

Modal dialogs (e.g. "Choose a File" from `hiDisplayFileDialog`) **intercept ALL input** — clicks and typing on the parent form will silently fail or go to the dialog.

**Detection**: After any Browse/Open action, run `vcli window list-windows-x11` and check for unexpected dialog windows (title contains "Choose", "Confirm", "Error", etc.).

**Resolution order**:
1. `xdotool windowactivate <dialog_wid>; sleep 0.5; xdotool key Return` (submit) — or `Escape` (cancel)
2. If Return doesn't close it, click the dialog's Open/Cancel button using method 3 with the **dialog's** window id and geometry
3. Only after the dialog is gone should you resume operating the parent form

### 8. CIW Input (the bootstrap channel)

Since `vcli skill exec` cannot drive GUI, the CIW is the bootstrap for form creation and state inspection.

**CIW input line coordinates** (must be re-verified if the CIW window moves):
- Click the input line, then `ctrl+a` (clear), then type the SKILL expression, then `Return`.
- Always `xwininfo -id <ciw_wid>` before typing — the CIW can be moved/resized by the user.
- The input line is at the **bottom** of the CIW window; compute `y = abs_y + height - 30` (approximate), then verify with a screenshot crop.

**CIW input pattern**:
```bash
xdotool windowactivate <ciw_wid>
sleep 0.8
xdotool mousemove <input_x> <input_y>
sleep 0.4
xdotool click 1
sleep 0.6
xdotool key --clearmodifiers ctrl+a
sleep 0.2
xdotool type --clearmodifiers --delay 50 'load("/path/to/file.il")'
sleep 0.4
xdotool key Return
sleep 2
```

### Recommended Debug Loop

```
1. debug_wrapper.py validate file.il              # syntax layer
2. scp file.il ubuntu-docker:/home/user1/
3. CIW input: load(".../file.il")                 # deploy
4. CIW input: udfShowForm()                       # display
5. vcli list-windows-x11 → get form wid           # locate
6. CIW: hiGetFieldInfo(form (quote field))        # reverse-engineer coords
7. xdotool mousemove --window + click             # interact (method 3B)
8. xdotool type / Tab+type / CIW assign           # input (method 4A/B/C)
9. ImageMagick crop screenshot                    # visual verify
10. CIW screenshot → read callback output         # behavioral verify
11. Modal dialog? → handle first (section 7)
12. Bug found → fix SKILL → repeat from 2
```

## Testing

```bash
python3 -m unittest tests.test_cli tests.test_command_runner \
    tests.test_live_executor tests.test_local_executor \
    tests.test_engine tests.test_model
```

## Schema Reference

See `references/scenario-schema.md` for the complete JSON DSL specification.
See `references/xdotool-cheatsheet.md` for xdotool command reference (local mode).
