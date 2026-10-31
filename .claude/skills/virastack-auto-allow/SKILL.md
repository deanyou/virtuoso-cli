---
name: virastack-auto-allow
display_name: ViraStack Auto-Allow
description: Auto-approve the ViraStack (Claude Code UI) permission prompt in a running Cadence Virtuoso session. Detects the "1 Allow / 2 Don't Allow" block by template matching and clicks Allow through vcli window action-x11. Trigger when the user asks to auto click / auto allow / auto approve the ViraStack or Claude Code permission prompt, or to keep Virtuoso GUI automation running unattended.
description_zh: 在运行中的 Cadence Virtuoso 会话里自动批准 ViraStack（Claude Code UI）的权限提示：模板匹配 "1 Allow / 2 Don't Allow" 弹窗并通过 vcli window action-x11 点击 Allow。
argument-hint: '[optional session id / window id, e.g. "xunipc-userone-34269 0x2400330"]'
allowed-tools: Bash(python3 *) Bash(vcli *) Read
category: coding
version: 1.0.0
author: deanyou
---

# ViraStack Auto-Allow

## Purpose

The ViraStack agent inside Virtuoso asks for permission before running shell
commands. Each request renders an inline block at the bottom of the transcript:

```
1  Allow
2  Don't Allow
```

This skill keeps that block approved without a human in the loop: it polls the
ViraStack window, finds the block by normalized cross-correlation against a
template, and clicks "Allow" through `vcli window action-x11`.

Everything the GUI does goes through the `vcli` binary boundary — the skill
never drives X11 directly, so the Virtuoso host keeps re-validating the
window identity server-side on every action.

## When to Use

- "auto click the Allow button in ViraStack / Claude Code"
- "keep approving the permission prompts while the agent works"
- The agent is blocked on a permission prompt and the session must keep running

Not for: one-off approvals (use a single `vcli window action-x11` click), or
anything that does not involve the ViraStack GUI.

## Prerequisites

| Item | Requirement |
|------|-------------|
| `vcli` | On PATH or passed via `--vcli`; must be able to reach the Virtuoso session |
| Live session | `vcli session list` shows one session (the daemon must be running) |
| Python deps | `numpy`, `scipy`, `Pillow` on the machine running the script |
| Target window | A visible ViraStack window on the Virtuoso host's X display |

## Workflow

### 1. Resolve the target

```bash
python3 ${CLAUDE_SKILL_DIR}/scripts/resolve_target.py --vcli vcli \
    [--session <SESSION>] [--window-id <WID>]
```

Pass `--session` / `--window-id` when `$ARGUMENTS` supplied them (see
[Arguments](#arguments)); otherwise the script resolves them.

Prints the session id, DISPLAY and window id of the ViraStack window:

```json
{"session": "xunipc-userone-34269", "display": ":0", "window_id": "0x2400334",
 "pid": 5730, "title": "ViraStack : DeepSeekViaJedAI (0)",
 "alternatives": ["0x2400330", "0x24002ea"]}
```

Several ViraStack windows may share one title (they render the same client).
Any of them works; pass `--window-id` to `auto_allow.py` to pin one.

### 2. Dry run first

```bash
python3 ${CLAUDE_SKILL_DIR}/scripts/auto_allow.py \
    --vcli vcli --session <SESSION> --window-id <WID> --display :0 \
    --dry-run --idle-polls 3 --interval 2
```

`--dry-run` detects and prints the target coordinates without clicking and
without aborting on the post-click guard. Expect either
`dry-run: would click Allow at (x, y)` or `no pending prompt after N idle polls`.

### 3. Run the loop (background)

```bash
python3 -u ${CLAUDE_SKILL_DIR}/scripts/auto_allow.py \
    --vcli vcli --display :0 \
    --interval 3 --idle-polls 0 --timeout 0 --max-clicks 0
```

`--session` and `--window-id` may be omitted: the loop resolves the single
live session and the ViraStack window by title. `--display` may be omitted
too: the loop reads the DISPLAY off the `list-windows-x11` answer, so it
targets the display Virtuoso actually runs on (a hard-coded `:0` is wrong on
a host whose Virtuoso runs on another one). An explicit `--display` always
wins. Run it with
`run_in_background: true` and keep it running for as long as the agent works.
`-u` matters — without it the log stays buffered and looks empty.

`--timeout 0` runs until the process is stopped; a finite timeout silently
kills the loop after a few hours, which is how an unattended overnight run
ends with no approvals happening.

`--idle-polls 0` is the setting to use while the agent operates a cellview:
a layout or schematic run (`apRunDeviceGeneration` and friends) can occupy
the agent for far longer than any idle budget without asking for permission,
and a loop that quits on idle would stop approving exactly when the agent
comes back with the next command.

A Virtuoso or daemon restart invalidates both the session id and the X11
window id, so the loop re-resolves them itself when a capture fails (log line
`re-resolved target: session=… window=…`) instead of failing forever.

Key flags:

| Flag | Default | Meaning |
|------|---------|---------|
| `--interval` | 2.0 | Seconds between polls |
| `--idle-polls` | 3 | Consecutive polls without a prompt before exiting. Raise it (40 ≈ 2 min) so the loop survives the gap between two commands; `0` disables the idle stop entirely — use that while the agent runs a long cellview operation |
| `--timeout` | 600 | Overall wall-clock budget in seconds; `0` runs until the process is stopped (the setting for an unattended session) |
| `--max-clicks` | 0 (unlimited) | Stop after N clicks |
| `--settle-polls` | 5 | Polls to wait for the prompt area to change after a click |
| `--session` | resolved | Daemon session id; resolved when omitted |
| `--window-id` | resolved | X11 window id; resolved by title prefix when omitted |
| `--title-prefix` | `ViraStack` | Window title prefix used to find the window again |
| `--guide-threshold` | 3 | Consecutive approvals of the *same* command before the agent is steered; `0` never steers |
| `--guide-message` | English text with `%d` | Message typed into the chat composer; `%d` is replaced by the repeat count. ASCII only |
| `--dry-run` | off | Detect only, never click |

### 4. Monitor

Read the background task's output file. Each cycle logs one line per
detection:

```
prompt detected (ncc=0.995) -> Allow at (90, 710)
clicked Allow at (90, 710)
```

Exit codes: `0` = stopped normally (idle or max-clicks), `1` = the click did
not land (the prompt area never changed) or 3 consecutive clicks were rejected
by `vcli`, `2` = bad arguments.

## Steering a repeated command

Auto-approving forever hides the failure mode where the agent keeps re-running
one command that never succeeds. The loop therefore fingerprints the command
line printed above the option rows and counts how many *consecutive* approvals
show that command. At every multiple of `--guide-threshold` (default 3) the
loop queues a guidance message.

The command is the text line in the code block of the permission card — not
the status line ("Running script...") above the option rows, which is the
same for every prompt and would make every request look like a repeat of the
same command. The code block is the only part of the card with its own
background, so the band is the last text line whose background differs from
the background of the card. That background is read from the rows *below* the
last text line — the gap between the card and the option rows — and not from
the whole region: a long command fills the region with the colour of the code
block, and the command would then be taken for the background.

The band spans the whole client area, minus the scrollbar on the right: a
command that runs past the crop would lose its tail, and two different
commands would share a fingerprint.

The fingerprint is a tolerant pixel comparison, not a hash: the band of two
consecutive frames of the same command is aligned within a few pixels and the
glyphs are compared in a 3x3 neighbourhood (`band_similarity`). A re-rendered
command still matches, a changed command does not. Measured on the captured
frames: the same command scores 1.0, the most similar pair of different
commands 0.91 — the threshold is 0.95.

The message is only typed when **no** permission prompt is pending: a pending
prompt that uses the composer as its answer field shows the answer
placeholder ("pick an option above"), and writing there would answer the
prompt instead of talking to the agent. While the agent is busy the composer
is disabled, so the delivery is verified with a screenshot — the text must
appear before Return is sent, and the composer must be empty again
afterwards, otherwise the message stays queued for the next poll.

Log lines:

```
same command 3 times in a row — guidance queued
guided the agent: Auto-approve notice: the same command has been requested 3 times in a row. …
```

The input line moves with the composer layout, so a few candidate rows are
tried in turn (`COMPOSER_CLICK_OFFSETS`, counted from the bottom of the
client area); the first one where the text shows up in the screenshot wins.

A composer that already holds ink is never typed into: it means either a
prompt is pending or the user is writing, so the message stays queued
("guidance skipped: the composer is not empty"). This also means the loop
never clears text somebody else typed.

The one exception is the text the loop itself typed and could not submit.
The loop keeps a snapshot of the composer as it held its own message and
compares the composer with it before it clears it: when the pixels still
match, the text is the message of the loop and the attempt is repeated; when
they do not, the user has typed over it and the composer is left alone
("guidance skipped: the composer holds text that is not ours").

`--guide-message` overrides the text (`%d` is the repeat count). It must be
ASCII: the `type` action of `vcli window action-x11` rejects anything else
(clipboard paste would need `xclip`, which is not installed on the Virtuoso
host).

## Arguments

`$ARGUMENTS` may carry the session id and/or the window id, whitespace
separated: `xunipc-userone-34269 0x2400330`. The first token that looks like a
session id (contains a `-`) is used as `--session`; a token starting with `0x`
is used as `--window-id`. Anything else is ignored and resolved automatically
by `resolve_target.py`. When the user supplies no argument, resolve the target
from the single live session.

## Detection

The template (`data/allow_prompt_template.png`) spans **both** option rows, so
a match can never be the "Don't Allow" row on its own. Scoring is normalized
cross-correlation (mean-subtracted template, integral-image window sums via
`scipy.signal.fftconvolve`) and the threshold is `0.90`; the "Don't Allow"
row alone scores about `0.45`, and a window without any prompt peaks around
`0.34–0.41` — still a wide margin below the threshold.

`ALLOW_OFFSET = (32, 16)` is the position of the "Allow" label **relative to
the matched template**, so the click lands correctly in any window size — do
not turn it into an absolute coordinate.

## Safety

**This grants the ViraStack agent unrestricted shell permission** — every
prompt is answered "Allow", including ones for destructive commands. Before
starting the loop, tell the user this. If only specific commands should be
allowed, the prompt text has to be matched per command and the "Don't Allow"
branch implemented; the current template is deliberately command-agnostic.

The post-click guard is the safety net against a misfiring loop: after each
click the pixels around the prompt are compared with the pre-click snapshot
(the region is clamped to the screenshot, so a narrow window still gets
checked). If nothing changed, the click did not land and the loop aborts
(exit 1) instead of hammering the same coordinate. Likewise, three
consecutive clicks rejected by `vcli` abort the loop (exit 1) — retrying the
same coordinates would not help.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `cannot open display XAUTHORITY=...` | Helper parsed the XAUTHORITY token as the DISPLAY | Fixed on the `fix/x11-client-geometry-and-capture-fallback` branch — rebuild `vcli` (`cargo build --release`) and reinstall it |
| `template not found` | Wrong `--template` | Default is `data/allow_prompt_template.png` next to the skill |
| `prompt area unchanged after click — aborting` | Click did not land, or the window moved | Re-check `vcli window geometry` for the window; the template offset is relative, so a resize is fine |
| Loop exits after a few seconds | `--idle-polls` too small for the gap between commands | Raise `--idle-polls` to 40 |
| `FileNotFoundError: allow_prompt_template.png` | The template was moved while the loop ran | Restart the loop — the template is now loaded once at startup |
| Nothing detected, prompt visible | UI theme/font changed, or the prompt is rendered as the inline "Allow" pill instead of the numbered block | Re-crop the template from a fresh `vcli window action-x11 --operation screenshot` |
| `capture error: no window with id` | Virtuoso or the daemon was restarted and the window id changed | Nothing to do — the loop re-resolves the session and window on its own; check the log for `re-resolved target:` |
| `guidance typed but not submitted — keeping it for the next poll` | The composer was disabled (agent busy) or Return was swallowed | Nothing to do — the message is retried on the next poll with no prompt pending |
| `same command N times in a row — guidance queued` | The agent is stuck retrying one command | The message goes out as soon as no prompt is pending; the approvals themselves keep working |

## Files

| Path | Role |
|------|------|
| `scripts/auto_allow.py` | The polling/detection/click loop |
| `scripts/resolve_target.py` | Resolve session id, DISPLAY and window id |
| `data/allow_prompt_template.png` | Detection template (both option rows) |
