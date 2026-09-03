# Scenario JSON DSL Schema

This document defines the strict JSON DSL for Virtuoso GUI Runner scenarios.

## Top-Level Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `version` | string | Yes | Schema version. MUST be exactly `"1.0"`. |
| `task_id` | string | Yes | Unique task identifier (non-empty) |
| `session_id` | string | Yes | Session ID (non-empty string) |
| `pid` | integer | Yes | Process ID (positive integer, bool rejected) |
| `display` | string | Yes | DISPLAY string (`:N` or `:N.M` format) |
| `cellview` | object | Yes | Target cellview (see below) |
| `steps` | array | Yes | Array of step objects (non-empty) |

**Extra fields are REJECTED.** The parser will raise `ScenarioValidationError` for any unknown field.

## CellView Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `lib` | string | Yes | Library name (non-empty) |
| `cell` | string | Yes | Cell name (non-empty) |
| `view` | string | Yes | View name (non-empty) |

## Step Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Step identifier (unique within scenario) |
| `operation` | string | Yes | Operation name (see allowed operations) |
| `arguments` | object | Yes | Operation-specific arguments (see per-op tables) |
| `verifier` | object | Yes | Verification predicate and expected value (non-empty, see below) |
| `timeout_seconds` | integer | Yes | Timeout (1-300 seconds) |
| `max_retries` | integer | Yes | Retry count (0 or 1 only) |
| `rollback` | object | No | Rollback action on failure (see below) |

**Extra fields are REJECTED.**

## Allowed Operations

### VCLI_LOAD
Load a VCLI command.
- **Required arguments:** `command` (string)

### VCLI_CALL
Call a VCLI function.
- **Required arguments:** `function` (string), `args` (array of JSON values)
- **Optional arguments:** `kwargs` (object of str -> JSON value)

### WINDOW_WAIT
Wait for a window to reach a state.
- **Required arguments:** `window_title` (string), `state` (string, must be `visible` or `hidden`)

### WINDOW_ACTIVATE
Activate a window by title.
- **Required arguments:** `window_title` (string)

### WINDOW_DISCOVER
Discover and filter windows.
- **Optional arguments:** `title` (string, substring match), `class` (string, WM_CLASS), `pid` (integer)

### DISMISS_DIALOG
Dismiss a dialog window.
- **Optional arguments:** `window_title` (string), `window_id` (string, explicit target)

### CLOSE
Close a window.
- **Optional arguments:** `window_id` (string, explicit target; defaults to bound window)

### KEY
Send a key event.
- **Required arguments:** `keys` (string)

### TYPE
Type text.
- **Required arguments:** `text` (string)

### CLICK_REL
Perform a relative click.
- **Required arguments:** `x` (integer), `y` (integer) — window-relative coordinates
- **Optional arguments:** `button` (integer, 1/2/3, default 1)

### DRAG_REL
Perform a relative drag.
- **Required arguments:** `x` (integer), `y` (integer) — relative move vector from window origin
- **Optional arguments:** `button` (integer, 1/2/3, default 1)

### SCROLL
Scroll the mouse wheel at a window-relative position. **Local executor only.**
- **Required arguments:** `direction` (string, must be `up`, `down`, `left`, or `right`)
- **Optional arguments:** `count` (integer, default 3), `x` (integer, default 50), `y` (integer, default 50)

Maps to xdotool mouse buttons: up=4, down=5, left=6, right=7.

### SCREENSHOT
Capture a screenshot.
- **Optional arguments:** `path` (string)

### VERIFY
Verify a condition.
- **Required arguments:** `predicate` (string, must be `window_exists` or `state_matches`), `expected` (any JSON value)

### RECOVER
Perform a recovery action.
- **Required arguments:** `action` (string), `target` (string)

## Verifier Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `predicate` | string | Yes | Supported predicates: `window_exists` (window is in the X11 window list), `state_matches` (window visible flag equals expected), `title_matches` (window title contains expected substring), `geometry_matches` (window geometry x/y/w/h matches expected dict) |
| `expected` | any JSON | Yes | Expected value (boolean for `window_exists`/`state_matches`, string for `title_matches`, dict for `geometry_matches`) |

Verifier must be a non-empty object. Unknown keys are rejected. Any predicate other than `window_exists`, `state_matches`, `title_matches`, or `geometry_matches` is rejected at parse time.

## Rollback Object

Rollback lets a step declare how to undo on failure.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `operation` | string | Yes | Operation to run for rollback (one of the allowed operations) |
| `arguments` | object | Yes | Arguments for that operation; validated with the same rules |

Other top-level fields on the rollback object are rejected. The chosen operation's arguments are validated against the same schema as regular steps.

## Validation Rules

1. **Version** — Must be exactly `"1.0"`. Any other value is rejected.
2. **Unknown fields** — Any field not listed above causes validation failure.
3. **Empty session_id / task_id** — Must be non-empty strings.
4. **Invalid PID** — Must be a positive integer (bool rejected).
5. **Invalid DISPLAY** — Must match `^:[0-9]+(\.[0-9]+)?$`.
6. **Empty cellview components** — lib, cell, view must all be non-empty strings.
7. **Timeout bounds** — Must be 1–300 seconds (bool rejected).
8. **Retry bounds** — Must be 0 or 1 only (bool rejected).
9. **Duplicate step IDs** — Each step must have a unique id.
10. **Verifier** — Non-empty object containing `predicate` (must be `window_exists` or `state_matches`) and `expected` (any JSON value); unknown keys rejected.
11. **Operation arguments** — Required arguments must be present; unknown arguments are rejected; types checked per the per-operation rules.
12. **WINDOW_WAIT state** — Must be `visible` or `hidden`. Any other value is rejected at parse time.
13. **CLICK_REL / DRAG_REL** — Coordinates are strict integers; `button` must be a positive integer. Bool rejected.
14. **Non-JSON values** — Argument, verifier, and rollback values that are not JSON-serializable are rejected.
15. **Rollback** — Must have `operation` (allowed op) and `arguments` (validated by the same rules).
16. **Deep immutability** — All parsed values are recursively frozen: lists become tuples, dicts become `MappingProxyType`, scalars pass through.

## Complete Example

```json
{
  "version": "1.0",
  "task_id": "example-task-001",
  "session_id": "sess-abc123",
  "pid": 12345,
  "display": ":0",
  "cellview": {
    "lib": "myLib",
    "cell": "myCell",
    "view": "schematic"
  },
  "steps": [
    {
      "id": "load_cmd",
      "operation": "VCLI_LOAD",
      "arguments": {"command": "loadDesign"},
      "verifier": {"predicate": "window_exists", "expected": true},
      "timeout_seconds": 30,
      "max_retries": 1
    },
    {
      "id": "wait_window",
      "operation": "WINDOW_WAIT",
      "arguments": {"window_title": "Virtuoso", "state": "visible"},
      "verifier": {"predicate": "state_matches", "expected": "visible"},
      "timeout_seconds": 60,
      "max_retries": 0,
      "rollback": {
        "operation": "KEY",
        "arguments": {"keys": "Escape"}
      }
    }
  ]
}
```
## Executor Semantics

The schema is executor-agnostic; executor selection happens on the CLI:

```bash
python3 scripts/gui_runner.py run SCENARIO --output DIR --executor fake
python3 scripts/gui_runner.py run SCENARIO --output DIR \
    --executor live --session SESSION_ID --vcli VCLI_PATH [--ssh-host HOST]
python3 scripts/gui_runner.py run SCENARIO --output DIR \
    --executor local [--window-id WID]
```

| Executor | Behavior |
|----------|----------|
| `fake` | Deterministic offline replay via injected outcomes (`--fake-outcomes`). No subprocess side effects. |
| `live` | Real `vcli` execution. Requires `--session` (must equal the scenario `session_id`), `--vcli`, and `--output`. Optional `--ssh-host` runs vcli over SSH with a safely-quoted fixed argv. |
| `local` | Direct `xdotool` execution on a local DISPLAY. Binds window by PID or `--window-id`. Supports `SCROLL`. Requires `xdotool` and a reachable `DISPLAY`. |

Live-mode operation mapping:

| DSL operation | Live behavior |
|---------------|---------------|
| `WINDOW_ACTIVATE` | `vcli window action-x11 --operation activate` |
| `KEY` | `vcli window action-x11 --operation key --text <keys>` |
| `TYPE` | `vcli window action-x11 --operation type --text <text>` |
| `CLICK_REL` | `vcli window action-x11 --operation click-rel --x --y` |
| `DRAG_REL` | `vcli window action-x11 --operation drag-rel --x --y` (start coordinates) |
| `SCREENSHOT` | `vcli window action-x11 --operation screenshot --output-dir` |
| `WINDOW_WAIT` | Condition polling via `vcli window list-windows-x11` |
| `VERIFY` | Database-first predicates via vcli; visibility via X11 window list |
| `RECOVER` | Executes only the step's validated `rollback` |
| `VCLI_LOAD` / `VCLI_CALL` | Accepted by the schema; rejected by the live executor (fail-closed) |

Live precheck enforces (in order): session exists and port matches, positive PID (with window-discovery fallback), DISPLAY equality, unique PID-bound window, and an exclusive DISPLAY lock. Any violation aborts the run before GUI input is sent.

Local-mode operation mapping:

| DSL operation | Local behavior |
|---------------|----------------|
| `WINDOW_ACTIVATE` | `xdotool windowactivate <id>` |
| `KEY` | `xdotool key <keys>` (window activated first) |
| `TYPE` | `xdotool type --clearmodifiers --delay 30 -- <text>` |
| `CLICK_REL` | resolve window geometry → absolute coords → `xdotool mousemove` + `click` |
| `DRAG_REL` | resolve window geometry → `mousedown` → interpolated `mousemove` → `mouseup` |
| `SCROLL` | `xdotool click --repeat N --delay 60 <button>` (buttons: up=4, down=5, left=6, right=7) |
| `SCREENSHOT` | `import -window <id> <path>` (ImageMagick) |
| `WINDOW_WAIT` | Poll `xdotool search --onlyvisible` until state matches or timeout |
| `VERIFY` | `xdotool search --onlyvisible` for window_exists / state_matches |
| `RECOVER` | Executes only the step's validated `rollback` |
| `VCLI_LOAD` / `VCLI_CALL` | Accepted by the schema; rejected by the local executor (fail-closed) |

Local precheck enforces: `xdotool` on PATH, DISPLAY reachable, and a visible window bound to the PID (or explicit `--window-id`).
