# agent-desktop × VCLI GUI Debug: read-only discovery gate

This is an optional experiment for a **real Linux Virtuoso desktop**. It does
not change GUI Runner execution, routing, retry, or verifier semantics. The
probe reports whether an accessibility snapshot exposes interactive nodes;
it does not prove that a node corresponds to a particular cellview, that an
action works, or that a design database changed as intended.

## Why this boundary

VCLI already has SKILL queries for design state and a bounded X11 executor for
GUI operations. `agent-desktop` adds a different observation source: a native
accessibility tree with snapshot-qualified refs and structured error delivery.
That can help find forms and dialogs when they expose semantic controls. It
cannot replace a SKILL assertion over the database. Cadence's Motif/Xt widgets
may expose little or no useful AT-SPI structure; this must be measured on the
target installation before introducing a new action route.

The intended future division, if evidence warrants it:

| Layer | Responsibility |
|---|---|
| Python GUI Runner | Scenario, policy, budgets, trace, independent verifier |
| VCLI/SKILL | Cellview and form semantic readback; preferred design assertions |
| VCLI Rust X11 | Atomic X11 actions and window binding on the compute host |
| agent-desktop | Optional AT-SPI observation, then a separately gated ref action |

No `agent-desktop` action is wired into the Router in this PR. Its `ok: true`
may mean delivery without a verified application effect. Its
`error.disposition.retry` is relevant only to a future adapter: an uncertain
or unsafe delivery must not be retried through X11 or SKILL. A stale ref needs
a new snapshot and a fresh policy decision. The Runner's DISPLAY lock and
session/PID/window/cellview binding remain mandatory for any future mutation.

## Probe on the Virtuoso desktop host

Install `agent-desktop` separately, pinned to a tested release. Run it as the
**same desktop user**, with the target `DBUS_SESSION_BUS_ADDRESS`, `DISPLAY`,
and `XDG_RUNTIME_DIR`. A plain SSH shell, jump host, or container lacking the
desktop's session bus cannot establish absence of UI controls. The upstream
Linux target is Ubuntu 24.04+ / GNOME 46 with AT-SPI; other EDA hosts need
their own compatibility check. Do not restart an existing bus as part of the
probe.

```bash
agent-desktop status
agent-desktop list-apps                 # find the exact app name
python3 .claude/skills/virtuoso-gui-debug/scripts/probe_agent_desktop.py \
  --app '<name from list-apps>'
```

The script runs only `status` and `snapshot --app ... -i --compact`, each with
a bounded process timeout. It prints one JSON report with `availability`,
`snapshot_complete`, node/ref counts, and role counts. It keeps the raw tree
in memory and emits no UI names, values, refs, screenshots, or command output.
It returns exit 0 only for a complete snapshot; partial, inaccessible, and
invalid responses return exit 2. A complete snapshot with zero actionable
nodes is **not** a reason to promote an accessibility route.

This probe deliberately cannot equate an agent-desktop window id with VCLI's
X11 `window_id` or a Virtuoso session PID. Record those bindings separately
in a controlled local investigation; never derive them by title alone.
Keep raw snapshots out of public CI logs and issue attachments.

## Decision for a later action adapter

Use one disposable test cellview and a known CIW/form workflow. Before adding
an action route, obtain live evidence for:

1. Complete snapshots of the exact target window on repeated open/close cycles,
   with unique actionable controls and stable enough identities for fresh refs.
2. A ref action delivered to the intended session/window, followed by a fresh
   observation and independent SKILL/form readback. A screen change alone is
   insufficient for a design-state PASS.
3. Stale/ambiguous refs, a partially observed tree, unavailable AT-SPI, a
   modal dialog, and a command timeout. Each must stop or refresh according to
   the agent-desktop disposition without replaying a potentially delivered
   action through another executor.
4. No extra action after the Runner's DISPLAY lock is lost, and no raw UI
   content in trace/evidence by default.

If these checks fail or AT-SPI exposes only a generic canvas, keep the current
SKILL → VCLI X11 path. A visual screenshot remains a human-inspection aid.

Upstream references: [agent-desktop README](https://github.com/vercel-labs/agent-desktop/blob/main/README.md),
[Linux platform guide](https://github.com/vercel-labs/agent-desktop/blob/main/skills/agent-desktop-linux/SKILL.md),
[JSON/disposition contract](https://github.com/vercel-labs/agent-desktop/blob/main/docs/json-output.md).
