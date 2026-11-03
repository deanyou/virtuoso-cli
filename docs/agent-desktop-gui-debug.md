# agent-desktop × VCLI GUI Debug: read-only discovery gate

This is an optional experiment for a **real Linux Virtuoso desktop**. It does
not change GUI Runner execution, routing, retry, or verifier semantics. The
probe reports whether an accessibility snapshot exposes interactive nodes;
it does not prove that a node corresponds to a particular cellview, that an
action works, or that a design database changed as intended. Snapshot sends
no clicks or keystrokes. On older at-spi2-atk it can still abort the observed
process; see Host compatibility.

## Scope and limits

- **Older at-spi2-atk can abort the target.** Upstream names Ubuntu 24.04+ /
  GNOME 46. On RHEL 9.7 with at-spi2-atk 2.38, read-only `snapshot` delivered
  SIGABRT to `gnome-terminal-server` and `nautilus` (`coredumpctl`). Confirm
  the library on a non-critical app before probing a production or demo
  desktop.
- **The release binary is v1.0.2 (JSON envelope 2.5, 2026-10-30).**
  `identity_unproven` and envelope 2.6 exist only in unreleased `main` docs.
  Installing the release does not provide that field.
- **Cadence dfII does not register on AT-SPI.** dfII is Motif/C++. Linux
  AT-SPI bridges GTK, Qt, and Java; there is no Motif bridge. `list-apps`,
  `snapshot`, `find`, and `screenshot --app virtuoso` return `APP_NOT_FOUND`
  with `detail=no_accessibility_connection` while the process is alive.
  Virtuoso automation stays on X11 (`xdotool` / `xwd`) or Cadence SKILL. A
  portal whole-screen screenshot needs a human grant and returns pixels only
  (no ref, no tree), so it does not justify an action adapter. The probe
  cannot tell that the target crashed mid-read and may still report
  `partial`.

## Why this boundary

VCLI already has SKILL queries for design state and a bounded X11 executor for
GUI operations. `agent-desktop` adds a different observation source: a native
accessibility tree with snapshot-qualified refs and structured error delivery.
That can help find forms and dialogs when the application exposes semantic
controls. It cannot replace a SKILL assertion over the database. Cadence dfII
(Motif/C++) has no AT-SPI implementation, so it never registers on the bus.
Restarting the process or setting toolkit accessibility variables does not
add a Motif bridge. The IC26.1 host check below is that negative result.
Virtuoso automation stays on X11 or Cadence SKILL.

The intended future division, if evidence warrants it for an application that
does register:

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

## Known unsupported: Motif/C++ applications (Cadence dfII)

Read-only check on one EDA host: RHEL 9.7, at-spi2-atk 2.38, Wayland +
XWayland, `DISPLAY=:0`, agent-desktop v1.0.2, Cadence dfII IC26.1. No clicks
or keystrokes were sent to Virtuoso. The process stayed alive.

| Command | Result |
|---|---|
| `list-apps` | 16 apps on the bus; Virtuoso absent |
| `list-apps` / `snapshot` / `find` / `screenshot --app virtuoso` | `APP_NOT_FOUND`, `detail=no_accessibility_connection` |
| `list-windows --app virtuoso` | `[]` |
| `screenshot --window-id <X11 id>` | `WINDOW_NOT_FOUND` (AT-SPI `w-…` ids only, not X11 ids) |
| `screenshot --screen 0` (Wayland portal) | `TIMEOUT` until a human grants the portal |
| `screenshot --screen 0` (`WAYLAND_DISPLAY` unset, X11 path) | `PLATFORM_NOT_SUPPORTED` |
| `probe_agent_desktop.py --app virtuoso` | `availability=unavailable`, `reason_code=snapshot_APP_NOT_FOUND`, exit 2 |

X11 on that same desktop still saw the windows. `xdotool search --name
Virtuoso` found a viewable window (`0x2400010`, 660×901), and `xwd -id`
captured the CIW. The process was healthy, and agent-desktop had no
accessibility connection to it.

A portal screenshot that a person grants is a pixel buffer of the whole
screen. It carries no ref and no tree. It can serve as a human glance at the
foreground. It does not justify an agent-desktop action adapter. Virtuoso
automation stays on X11 (`xdotool` / `xwd` / `import`) or Cadence SKILL.

## Host compatibility

`snapshot` walks AT-SPI objects inside the target. It sends no input, and on
unpatched older at-spi2-atk that walk is enough to abort the app. On RHEL 9.7
/ at-spi2-atk 2.38, snapshot of `gnome-terminal-server` and of `nautilus`
each ended in SIGABRT, confirmed with `coredumpctl`. Upstream's stated Linux
target is Ubuntu 24.04+ / GNOME 46. Treat every other desktop, including
typical EDA hosts, as unproven until its at-spi2-atk build is checked.
Before a production or demo session, run snapshot against a disposable
application and confirm the target process is still alive afterward.

The probe has no target-health signal. If the app aborts during the read,
the report can still say `partial`. Check the target process separately.

## Probe on the Virtuoso desktop host

Install `agent-desktop` separately and pin a tested build. The current
release is **v1.0.2** (2026-10-30, JSON envelope **2.5**). Run it as the
**same desktop user**, with the target `DBUS_SESSION_BUS_ADDRESS`, `DISPLAY`,
and `XDG_RUNTIME_DIR`. A plain SSH shell, jump host, or container lacking the
desktop's session bus cannot establish absence of UI controls. Do not restart
an existing bus as part of the probe.

```bash
agent-desktop status
agent-desktop list-apps                 # find the exact app name
python3 .claude/skills/virtuoso-gui-debug/scripts/probe_agent_desktop.py \
  --app '<name from list-apps>'
```

For Virtuoso, `list-apps` does not show dfII. `--app virtuoso` is expected to
exit 2 with `snapshot_APP_NOT_FOUND`: the process can be alive and still have
no accessibility connection.

The script runs only `status` and `snapshot --app ... -i --compact`, each with
a bounded process timeout. The 4 MB response check is applied after capture;
it is a parsing guard, not a streaming memory limit. It prints one JSON report
with `availability`, `snapshot_complete`, node/ref counts, and role counts. It
keeps the raw tree in memory and emits no UI names, values, refs, screenshots,
or command output. It returns exit 0 only for a valid complete snapshot with
no reported `identity_unproven` flag; partial, inaccessible, and invalid
responses return exit 2. On the v1.0.2 binary that flag is absent; see
Release v1.0.2 below. A complete snapshot with zero actionable nodes is
**not** a reason to promote an accessibility route.

This probe deliberately cannot equate an agent-desktop window id with VCLI's
X11 `window_id` or a Virtuoso session PID. Record those bindings separately
in a controlled local investigation; never derive them by title alone.
Keep raw snapshots out of public CI logs and issue attachments.

## Decision for a later action adapter

Virtuoso is outside this adapter. dfII exposes no AT-SPI tree, and a portal
screenshot does not supply refs. Keep Virtuoso on SKILL and on VCLI X11
(`xdotool` / `xwd` are the tools that still see the windows).

For a different application that does register, use one disposable target and
a known workflow. Before adding an action route, obtain live evidence for:

1. Complete snapshots of the exact target window on repeated open/close cycles,
   with unique actionable controls and stable enough identities for fresh refs.
   Re-check the process after each snapshot. The probe may still report
   `partial` when the target aborted mid-read.
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
SKILL → VCLI X11 path.

The links below are **unreleased `main` docs**, not the v1.0.2 release:
[agent-desktop README](https://github.com/vercel-labs/agent-desktop/blob/main/README.md),
[Linux platform guide](https://github.com/vercel-labs/agent-desktop/blob/main/skills/agent-desktop-linux/SKILL.md),
[JSON/disposition contract](https://github.com/vercel-labs/agent-desktop/blob/main/docs/json-output.md).

## Release v1.0.2 (envelope 2.5) and unreleased 2.6 docs

Published agent-desktop **v1.0.2** (2026-10-30) speaks JSON envelope **2.5**.
That binary has no `identity_unproven` field. Envelope **2.6** and
`data.identity_unproven: true` are documented only on unreleased `main`.
Installing the release will not show the field. A missing field on v1.0.2 is
the published contract, not a probe defect.

When a build does emit the flag, `true` means complete presentation still
does not establish capture-time ref identity. The probe keeps the flag,
reports `partial` with reason `identity_unproven`, and exits 2. An omitted
flag, which is what v1.0.2 returns, is not a VCLI identity proof: this
experiment never authorizes action routing. A future adapter must pin the
build it tested. Handling 2.6 ref-identity semantics applies only to a build
newer than the v1.0.2 release. Missing/malformed trees and mismatched command
envelopes are rejected rather than counted as empty successful trees.
