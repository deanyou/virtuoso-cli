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
- `--executor local` — direct `xdotool` execution on a local X11 DISPLAY. Binds the target window by PID (or explicit `--window-id`). Supports `SCROLL` (xdotool buttons 4/5/6/7). No vcli binary or session required. Live mode also supports `SCROLL` via `vcli window action-x11 --operation scroll --text direction[:count]`.

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

## Auto-Discovery (SSH Remote)

自动发现 Virtuoso 的 DISPLAY 和 PID：

```bash
# 方法1: 从 daemon log 直接获取
ssh ubuntu-docker "tail /tmp/virtuoso-daemon.log"

# 方法2: 查找 virtuoso 进程并获取 DISPLAY
ssh ubuntu-docker "ps aux | grep virtuoso | grep -v grep | awk '{print \$2}' | head -1"
ssh ubuntu-docker "strings /proc/<PID>/environ | grep DISPLAY"

# 方法3: 从 daemon 获取当前会话端口
ssh ubuntu-docker "cat /tmp/virtuoso-daemon.log | grep PORT"
```

**快速发现脚本** (在 skill-dev 目录执行):
```bash
./scripts/vssh.sh --discover
```

**典型结果**:
- PID: `12784`
- DISPLAY: `:5.0`
- Session ID: `dean-user1-<PORT>`

## vcli GUI Debug 快速指南

### 一、连接 Session
```bash
# 列出所有 session
VCLI_CAPABILITY=admin VB_PORT=XXXXX VB_REMOTE_HOST=localhost vcli session list

# 查看 session 详情
VCLI_CAPABILITY=admin VB_PORT=XXXXX VB_REMOTE_HOST=localhost vcli session show dean-user1-XXXXX
```

关键字段：
- `alive: true` — session 存活
- `pid: 0` — 旧 bridge 元数据，需通过窗口发现回退

### 二、发现 DISPLAY（云电脑关键！）

⚠️ vcli 的 `--display :0` 经常不对！云电脑上 Virtuoso 可能运行在其他 DISPLAY：

```bash
# 方法1：查看 X11 socket 文件
ssh ubuntu-docker "ls /tmp/.X11-unix/"
# 输出 X99 → DISPLAY=:99

# 方法2：查看 Virtuoso 进程
ssh ubuntu-docker "ps aux | grep virtuoso | grep -v grep"

# 方法3：逐个尝试（常见 :0, :1, :99）
vcli window list-windows-x11 --display :99 --session dean-user1-XXXXX
```

### 三、发现窗口
```bash
# 列出指定 DISPLAY 上的所有窗口
vcli window list-windows-x11 --display :99 --session dean-user1-XXXXX
```

输出字段说明：
```
{
  "window_id": "0x3000000",  // ← 操作时用这个字段（不是 id）
  "pid": 393027,              // 进程 ID
  "title": "VCLI_XDOTOOL_TEST",
  "geometry": {"x":960,"y":446,"w":810,"h":634},
  "visible": true
}
```

快速筛选：
```bash
vcli window list-windows-x11 --display :99 --session dean-user1-XXXXX | python3 -c "
import json,sys
for w in json.load(sys.stdin)['windows']:
    print(w['window_id'], w['pid'], w['title'][:40])
"
```

### 四、执行 GUI 操作

```bash
# 通用格式（--direct 跳过 helper 上传，快 5 倍）
vcli window action-x11 \
  --window-id 0x3000000 \
  --display :99 \
  --session dean-user1-XXXXX \
  --pid 393027 \
  --operation <OP> \
  --direct
```

常用操作：

| 操作 | 额外参数 | 示例 |
|------|---------|------|
| activate | 无 | 激活窗口 |
| click-rel | --x --y | 相对坐标点击 |
| click-abs | --x --y | 绝对坐标点击 |
| double-click | --x --y | 双击 |
| key | --text Escape | 发送按键 |
| type | --text "hello" | 输入文本 |

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

Every GUI action (`KEY`, `TYPE`, `CLICK_REL`, `CLICK_ABS`, `DOUBLE_CLICK`, `DRAG_REL`, `WINDOW_ACTIVATE`, `MINIMIZE`, `MAXIMIZE`, `CLOSE`, `SCROLL`) maps to a fixed `vcli window action-x11` argv carrying the resolved window id, PID, and DISPLAY. **`--direct` is enabled by default** (~5x faster, skips helper upload/env resolution/list-windows); use `--no-direct` for full server-side re-validation. `--pid` is optional since v1.3.1 (windows without `_NET_WM_PID` are reachable). `verify` prefers database-first predicates via vcli; the `ciw_eval` predicate executes SKILL via `vcli skill exec` and compares output. `recover` executes only rollback operations that pass scenario validation.

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

- `VCLI_LOAD` — load a SKILL file via `vcli skill load` (supports `skillpp: true` for SKILL++ mode). **Executable** by live executor.
- `VCLI_CALL` — accepted by the schema; **not executable** by live or local executors (use `CIW_INPUT` for ad-hoc SKILL evaluation).
- `WINDOW_WAIT` — poll window state until the requested condition or timeout
- `WINDOW_ACTIVATE` — activate window
- `WINDOW_DISCOVER` — discover/filter windows (title/class/pid filters)
- `DISMISS_DIALOG` — dismiss a dialog (vcli dismiss-dialog / xdotool Escape)
- `CLOSE` — close a window
- `KEY` — send key event
- `TYPE` — type text
- `CLICK_REL` — relative click (window-relative coordinates)
- `CLICK_ABS` — absolute click (screen coordinates)
- `DOUBLE_CLICK` — double-click (window-relative coordinates)
- `DRAG_REL` — relative drag (window-relative vector)
- `SCROLL` — scroll wheel at window-relative position (directions: up/down/left/right, optional count 1-100; live mode via vcli scroll, local mode via xdotool buttons 4/5/6/7)
- `MINIMIZE` — minimize/iconify the window
- `MAXIMIZE` — maximize the window (requires xdotool ≥ 3.20210804.1; clear error on older versions)
- `CIW_INPUT` — type a SKILL expression into the CIW input line and press Return (encapsulates activate→click input line→clear→type→Return)
- `SCREENSHOT` — capture screenshot
- `VERIFY` — verify state (predicates: window_exists, state_matches, title_matches, geometry_matches, ciw_eval)
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

**DSL operation `CIW_INPUT`** encapsulates the full flow: activate → click input line → clear → type → Return. Use this in scenarios instead of manual xdotool sequences.

```json
{"operation": "CIW_INPUT", "arguments": {"text": "load(\"/tmp/form.il\")"}}
{"operation": "CIW_INPUT", "arguments": {"text": "udfShowForm()", "delay_ms": 10, "clear_first": true}}
```

**Manual CIW input pattern** (when not using the DSL):
```bash
xdotool windowactivate <ciw_wid>
sleep 0.3
xdotool mousemove --window <ciw_wid> 400 870
xdotool click 1
sleep 0.1
xdotool key ctrl+a
xdotool key Delete
xdotool type --clearmodifiers --delay 10 'load("/path/to/file.il")'
xdotool key Return
sleep 1
```

**CIW input line coordinates** (must be re-verified if the CIW window moves):
- The input line is at the **bottom** of the CIW window; compute `y = height - 20` (approximate), then verify with a screenshot crop.
- Always `xwininfo -id <ciw_wid>` before typing — the CIW can be moved/resized by the user.

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

## Performance Optimization (measured on Virtuoso IC25.1, DISPLAY=:5.0)

### Latency baseline

| Operation | Latency | Notes |
|-----------|---------|-------|
| `vcli window action-x11 click-rel` | **~1350 ms** | Per call — Rust binary startup + X11 reconnect + server-side window re-resolution |
| `vcli window list-windows-x11` | **~940 ms** | Per call — full window tree scan |
| `xdotool mousemove --window + click` | **~10 ms** | 135× faster than vcli |
| `xwininfo -id <wid>` | **~3 ms** | 313× faster than vcli list-windows |
| `xdotool type --delay 50` (20 chars) | ~530 ms | Default in earlier scripts |
| `xdotool type --delay 10` (20 chars) | ~120 ms | 4.4× faster; verified no char loss |
| `xdotool type --delay 5` (20 chars) | ~70 ms | Reliable for ASCII; use 10 for safety |
| `import -window <wid>` (screenshot) | ~20 ms | Fast; occasional failure on unmapped windows |
| CIW input + exec (click+ctrl+a+type+Return) | ~425 ms | With delay=10; ~800 ms with delay=50 |

### P0 — Use xdotool by default, vcli only when server-side validation is required

The `vcli window action-x11` path pays a **1.3 second per-call tax** because every invocation starts the Rust binary, reconnects to X11, and re-resolves the window. For rapid GUI interaction (clicks, typing, dragging), use direct `xdotool` with `--window <wid>`:

```bash
# Fast path (10 ms):
xdotool mousemove --window 0x26037c7 300 167
xdotool click 1

# Slow path (1350 ms) — only when you need the Rust side to re-validate window identity:
vcli window action-x11 --window-id 0x26037c7 --pid 114668 --display :5.0 \
  --operation click-rel --x 300 --y 167
```

Use vcli when: (a) the window identity must be server-verified for safety, (b) you are in `--executor live` mode of the DSL runner, or (c) xdotool is unavailable.

### P0 — `--direct` is now the DEFAULT in live executor (5× faster)

The live executor uses `vcli window action-x11 --direct` by default, skipping helper upload, env resolution, and list-windows scan. This reduces per-action latency from ~1350ms to ~260ms. Use `--no-direct` CLI flag only when you need full server-side window re-validation (e.g., untrusted window ids).

```bash
# Default (fast, 260ms):
python3 scripts/gui_runner.py run scenario.json --output out --executor live \
    --session dean-user1-XXXXX --vcli ~/.cargo/bin/vcli --ssh-host ubuntu-docker

# Full validation (slow, 1350ms, use --no-direct):
python3 scripts/gui_runner.py run scenario.json --output out --executor live \
    --session dean-user1-XXXXX --vcli ~/.cargo/bin/vcli --ssh-host ubuntu-docker \
    --no-direct
```

`--direct` supports: `activate`, `key`, `type`, `click-rel`, `drag-rel`, `scroll`, `close`. It **rejects** `wait` (needs window-list polling) and `screenshot` (needs artifact fetch) with a clear config error. Verified on IC25.1: click/type/key all succeed with correct field values and callback firing.

### P0 — Use `action-x11-batch` for consecutive operations (6.3× faster)

When you have a sequence of GUI operations (click → type → click → type...), use `action-x11-batch` with `--direct` to execute them all in **one process invocation and one SSH round-trip**. All xdotool commands are merged into a single shell script with per-command exit-code markers.

```bash
# batch.jsonl — one JSON action per line:
{"window_id": "0x2603839", "operation": "click-rel", "x": 116, "y": 59}
{"window_id": "0x2603839", "operation": "click-rel", "x": 300, "y": 167}
{"window_id": "0x2603839", "operation": "type", "text": "METAL1"}
{"window_id": "0x2603839", "operation": "click-rel", "x": 300, "y": 204}
{"window_id": "0x2603839", "operation": "type", "text": "METAL2"}

# Execute all 5 in one call (260ms total vs 1300ms for 5 separate --direct calls):
vcli window action-x11-batch --file batch.jsonl --direct --pid 114668 --display :5.0
```

Result includes per-action status, duration, and error. A single action failure does not abort the batch. Each action may override `pid` and `display`; CLI flags are defaults.

**Performance comparison (6 actions, IC25.1 remote):**

| Mode | Total | Per-action | Speedup |
|------|-------|-----------|---------|
| 6× separate `action-x11` (normal) | ~7300 ms | ~1213 ms | 1× |
| 6× separate `action-x11 --direct` | ~1650 ms | ~275 ms | 4.4× |
| `action-x11-batch --direct` (merged shell) | **260 ms** | ~43 ms | **28×** |

### P0 — Use xwininfo for geometry, not vcli list-windows

```bash
# Fast (3 ms):
xwininfo -id 0x26037c7 | grep -E "Absolute|Width|Height"

# Slow (940 ms) — only when you need to discover windows by title/pid:
vcli window list-windows-x11 --display :5.0 --format json
```

Reserve `list-windows-x11` for **window discovery** (finding a window you don't have the id for). Once you have the id, all geometry checks use `xwininfo`.

### P1 — Reduce type delay to 10–15 ms

`--delay 50` was conservative. `--delay 10` is verified reliable for ASCII input into both form fields and the CIW (no dropped characters across 6 repeated rounds). Use `--delay 15` for non-ASCII or complex strings.

```bash
# Before (530 ms for 20 chars):
xdotool type --clearmodifiers --delay 50 "METAL1"

# After (120 ms for 20 chars):
xdotool type --clearmodifiers --delay 10 "METAL1"
```

### P1 — Eliminate inter-operation sleep for consecutive xdotool calls

Consecutive `xdotool mousemove` + `click` calls with **zero sleep** are reliable (verified: 6 rapid radio clicks all fired callbacks and changed form height correctly). Only sleep when waiting for Virtuoso to respond asynchronously:

- **No sleep needed**: consecutive clicks, consecutive type, mousemove→click
- **Sleep / poll needed**: after triggering a form redraw (radio callback changes layout), after opening a modal dialog, after CIW Return (wait for eval result)
- **Prefer conditional polling** over fixed sleep: `xwininfo` loop waiting for height change, or `vcli list-windows` waiting for dialog appearance

```bash
# Bad: fixed 800ms sleep after every click
xdotool click 1; sleep 0.8

# Good: poll for the expected state change
for i in $(seq 1 20); do
  h=$(xwininfo -id $WID 2>/dev/null | grep Height | awk '{print $2}')
  [ "$h" = "250" ] && break
  sleep 0.05
done
```

### P2 — vcli-side optimizations (Rust changes, all implemented)

- **✅ `--direct` flag (implemented, commit 513f929)**: skips helper upload, env resolution, and list-windows scan. 4.7× faster (1213ms → 260ms). Use when vcli is required but window identity is already known.
- **✅ `action-x11-batch` (implemented, commit bab1809 + 10c88dc)**: JSONL batch mode with merged shell execution. 6 actions in 260ms (28× vs normal mode). All xdotool commands merged into one SSH round-trip with per-command exit-code markers.
- **✅ Geometry precheck in `--direct` mode (PR #68)**: before sending a `click-rel`/`drag-rel`/`scroll` with coordinates, runs `xwininfo` to verify the window is not zero-sized (minimized/unmapped) and the coordinates are within bounds. Out-of-bounds coordinates are rejected with exit code 2 and a clear error message (`"coordinates (x, y) out of bounds for window size WxH"`). Prevents sending clicks to stale coordinates after a window moves/resizes.
- **✅ Batch non-direct shared list-windows (PR #68)**: in non-direct batch mode, the helper upload, env resolution, and list-windows scan are done **once per unique DISPLAY** and reused across all actions in the batch. 3 actions in 1527ms (per-action only 134–271ms vs ~940ms each without sharing).
- **✅ Geometry file cache (PR #68)**: direct-mode writes a `/tmp/vcli_geom_<display>_<wid>.json` cache (cross-platform via `std::env::temp_dir()`) with 500ms TTL. Used for zero-size fast-reject (avoids xwininfo round-trip on repeated calls to a minimized window). Coordinate bounds checking always uses fresh xwininfo.
- **Daemon mode (deferred)**: persistent `vcli gui-daemon` holding X11 connection over a local socket. Batch mode already covers the main use case (consecutive operations in one process); daemon's marginal gain is small.

### P2 — Coordinate caching

`hiGetFieldInfo` reverse-engineering costs one CIW round-trip (~425ms). The live executor provides a coordinate cache API:

```python
executor.cache_coords("myForm", "oldLayer", x=5, y=150, w=590, h=35)
coords = executor.get_cached_coords("myForm", "oldLayer")  # → {"x":5, "y":150, "w":590, "h":35}
executor.invalidate_coords("myForm")  # call after window resize/layout change
```

Cache the resulting `((x y) (w h))` per field for the lifetime of the form; only re-query if `xwininfo` detects the window was resized or a radio callback changed the layout.

### P2 — Modal dialog auto-detection (default ON)

After any GUI action that may spawn a dialog (`CLICK_REL`, `CLICK_ABS`, `DOUBLE_CLICK`, `KEY`, `TYPE`, `CIW_INPUT`), the live executor automatically scans for new dialog windows (titles containing Choose/Confirm/Error/Warning/Dialog/Message/Alert/Question) and dismisses them via `vcli dismiss-window-x11`. This prevents silent failures when a Browse/Open action spawns a file chooser that intercepts all subsequent input.

Disable with `--no-auto-dismiss` if you need to interact with dialogs explicitly.

### P2 — Verification: prefer CIW state reads over screenshot+OCR

Reading a field value via CIW (`form->field->value`) costs ~425ms and is deterministic. Screenshot+OCR costs ~20ms but is unreliable (±40px drift, garbled text). Use CIW reads for behavioral verification; use screenshots only for visual evidence in reports.

## Virtuoso GUI API Semantics (verified on IC25.1)

> Hard-won findings from testing 13 example GUI programs (ui_dynamic_form, ui_callback_patterns, ui_color_picker, ui_listbox_*, ui_multipage_form, ui_progress_*, ui_table_form, ui_toggle_combo_form, menu_demo/*). These are NOT in the Cadence docs — they were discovered by breaking things.

### Form Field Access Paths

| Field type | Access pattern | Gotcha |
|---|---|---|
| Top-level field | `form->fieldName->value` | Direct access works |
| Field inside **tab field** | `form->tabField->pageName->fieldName->value` | **Direct `form->fieldName->value` returns nil** — must go through tab→page |
| ListBox field | `form->listbox->value` is always a **list** | Read with `car()`, set with `(list val)` |
| Cyclic field | `form->cyclic->value` is a **single string** | Not a list |
| Toggle field | `form->toggle->value` is `t`/`nil` list | `?choices` each item must be `(symbol label)` list |

### Widget Creation Dependencies

| Widget | Requires | Gotcha |
|---|---|---|
| `hiCreateLayerCyclicField` | Open cellview (`geGetEditRep()` non-nil) | `techGetTechFile(nil)` crashes. Guard: `when(rep Tech=techGetTechFile(rep) ...)` |
| `hiCreateReportField` | None | Data is static list of lists; no dynamic update API |
| `hiCreateTabField` | None | Pages are symbols; fields inside need tab→page access path |
| `hiCreateSpinBox` | None | Arrows are ~10px, hard to click via xdotool. Prefer CIW: `form->spinbox->value = n` |
| `hiCreatePointField` / `hiCreatePointListField` | None | Values are `(x y)` lists; render as read-only text |

### Modal Form Behavior

- `hiDisplayForm` creates a **modal** form that **blocks `vcli skill exec`** (30s timeout). While the form is open, all `vcli skill exec` calls hang until the form is dismissed.
- To read form state while a modal form is open: use **CIW input** (xdotool type into CIW), not `vcli skill exec`.
- `Escape` does NOT always close a form — click Cancel/OK button, or use `hiFormCancel(form)` via CIW.
- `alt+F4` and `xdotool windowclose` may not work on Virtuoso modal forms.

### Menu System

- `hiInsertBannerMenu((hiGetCIWindow) menu)` inserts a pulldown into the CIW menu bar.
- `hiCreateSliderMenuItem` with `?subMenu` renders a right-arrow (▶) indicating a submenu.
- `hiCreateSeparatorMenuItem` renders a horizontal divider.
- Menu `?callback` is a **string** that gets `eval`'d on click. If the function is undefined, CIW shows `undefined variable - funcName` (callback DID fire).
- Submenus open on hover in most cases; if not, click the parent item.

### CIW Input Reliability

- `ctrl+a` does NOT select-all in Virtuoso form fields. Use `Escape` to clear the CIW input line.
- `vcli skill load` times out on large files (>~50 lines). Use CIW `load("/path/file.il")` instead.
- Semicolon-separated multi-expression CIW input: the second assignment may not execute. Run expressions one at a time.
- `lambda((x) body)` fails — must be `lambda( (x) body)` with a space after `lambda(`.
- `return` only works inside `prog()` blocks, not `let()` blocks. In `let`, the last expression is the implicit return.

## Testing

```bash
python3 -m unittest tests.test_cli tests.test_command_runner \
    tests.test_live_executor tests.test_local_executor \
    tests.test_engine tests.test_model
```

## Schema Reference

See `references/scenario-schema.md` for the complete JSON DSL specification.
See `references/xdotool-cheatsheet.md` for xdotool command reference (local mode).
