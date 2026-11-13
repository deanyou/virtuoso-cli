# Drive a Virtuoso window over X11

`vcli window` drives the X11 side of a running Virtuoso: it enumerates the
Virtuoso-related windows on a display, reports one window's client-area
geometry, and performs fixed-semantics actions (activate, key, type, click,
drag, scroll, screenshot, wait, …) against one specific window. The window id
and the DISPLAY must both be exact — the bridge never guesses which window you
meant.

## Sub-features

- `x11-list-windows` enumerates Virtuoso-related X11 windows with their id,
  title, geometry and the cookie the helper used.
- `x11-geometry` reports the client area of one window, the coordinate space
  `action-x11` validates `--x`/`--y` against.
- `x11-action` performs one fixed-semantics action on one window.
- `x11-action-batch` performs several actions in one process invocation.
- `x11-screenshot` captures a window to a PNG file.

## How to get to it (user POV)

- Run `vcli window list-windows-x11` to find the `dismiss_id` of the window to
  drive.
- Run `vcli window geometry --window-id <id>` to learn the client area.
- Run `vcli window action-x11 --window-id <id> --display <DISPLAY> --operation
  screenshot --output-dir <dir>` to act on a window.
- Run `vcli window action-x11-batch --file <batch.jsonl>` to feed a JSONL batch of actions.

## Driving it with vcli

Preconditions:

- A Virtuoso instance is running and its X11 windows are on a display this
  host can reach. The X11 leg talks to the X server directly; it is not the
  RAMIC bridge, but the window ids come from a Virtuoso that the bridge
  session also points at.
- `xdotool` is available on the host that runs the X server.
- The DISPLAY is known and exact. `list-windows-x11` reports it as `display`
  on every window; pass that exact string back to `action-x11`.
- `VB_HOME` points at a scratch directory so the run's state and artifacts do
  not mix with a real user's.

- **Enumerate.** Run `vcli --format json window list-windows-x11`. Exit code `0`
  and stdout is one JSON object with a `count` and a `windows` array. Each
  window carries `window_id`, `dismiss_id`, `title`, `class`, `pid`,
  `geometry`, `display` and `xauthority`. The count matches the number of
  Virtuoso-related windows the X server exposes — on a stock IC26.1 session
  with the Virtuoso AI Studio log window open that is `99`.
- **Read the cookie the helper actually used.** In the same JSON, each window's
  `xauthority` is the cookie path the helper ran with, or `null` when the
  helper used no cookie. A `null` here is the observable difference between a
  display reached by auto-detection and one reached by an explicit
  `--display`: the explicit path can be given a cookie, the auto path cannot.
- **Client geometry.** Run `vcli --format json window geometry --window-id
  <id>`. Exit code `0` and stdout has `"status": "success"`, the `window_id`,
  the `display`, and a `geometry` object with the **client area**'s `x`, `y`,
  `w` and `h`. This is the coordinate space `action-x11` bounds-checks
  `--x`/`--y` against; it is not the frame geometry that `list-windows-x11`
  reports, which includes the window-manager title bar and border. The two
  disagree by the decoration, not by noise: one observed layout window reported
  frame `x=960 y=303 w=970 h=783` against client `x=970 y=333 w=950 h=743`.
- **Screenshot.** Run `vcli --format json window action-x11 --window-id <id>
  --display <DISPLAY> --operation screenshot --output-dir <dir>`. Exit code `0`
  and stdout has `"status": "success"`, the `window_id`, the `display`, a
  `duration_ms`, and an `artifact` object with the PNG's `local_path`, `name`,
  `sha256` and `size_bytes`. The file named in `local_path` exists and is a
  PNG.
- **The action is bound to one window.** The `window_id` in the result is the
  same `window_id` that was requested. A screenshot of some other window is a
  wrong-window result, not a pass.
- **A wrong window id fails closed.** Run the same command with a window id
  that does not exist. Stdout is empty, stderr carries the structured error
  `{"error":"not_found","message":"not found: no window with id '0xdeadbeef'
  matching PID=any DISPLAY=:0", ...}`, the exit code is `3` (not found), and
  no PNG is written.
- **A wrong display fails closed too.** The same window id with a display
  that does not host it fails the same way — `not_found`, exit code `3`, and
  the message names the display it looked for. The window id is never matched
  loosely against a different display.
- **The side effect is in the history.** `$VB_HOME/cache/virtuoso_bridge/history/cmd.jsonl`
  gains one JSON record per invocation, carrying `ts`, `session`, `cmd` and
  `exit_code` — including the failed ones, with their `3`.
- **Batch.** Run `vcli --format json window action-x11-batch --file <batch.jsonl>`
  with a JSONL file of actions. Each line is one JSON object with `window_id`,
  `pid`, `display`, `operation`, `x`, `y`, `button`, `text`; `pid` and `display`
  default to the CLI flags. The actions run sequentially against a single
  `list-windows` scan, and the output reports a per-action result — one failing
  action does not silently become a success.

## Gotchas

- **The window id and the DISPLAY must both be exact.** `action-x11`
  re-validates the window before each action, and it matches the DISPLAY
  strictly — a window whose `display` differs from the one you pass is a
  mismatch, not a near-miss. Passing `None` for the display never matches.
- **A PID is optional but must be exact when given.** Windows that expose no
  `_NET_WM_PID` are listed with `pid: null`; those can only be driven by
  window id alone. When you do pass `--pid`, it must match the window exactly
  or the action is refused.
- **`--output-dir` needs an absolute path.** A relative path fails in the
  local file copy, with an error that has nothing to do with X11.
- **`--direct` is faster but narrower.** It skips helper upload, environment
  resolution and the `list-windows` scan, trusting the caller's window id. It
  is ~25x faster for click/key/type and it is not compatible with
  `--operation wait` or `--operation screenshot`.
- **The cookie is not enforced on every X server.** On a server that grants
  `SI:localuser:<user>` in `xhost`, an unauthenticated connection succeeds and
  a missing cookie is invisible in the result. The `xauthority` field is the
  place to look for the difference, not the success of the action.
- **`doctor.sh` probes the configured bridge port.** On a session whose port is
  not the configured one, the doctor reports `tier_b: unreachable` even though
  the bridge is alive. Prove liveness separately with
  `vcli --format json rpc call --method util.ping --params '{}'` before
  concluding the bridge is down.
- **Raw SKILL execution is gated.** Commands that evaluate SKILL directly
  refuse to run unless `VCLI_CAPABILITY=admin` is set. This is a client-side
  guard, and it fires before any bridge traffic.
- **This is a tier-B surface.** The X11 leg reaches the live Virtuoso window.
  A replayed or mocked harness proves nothing about it; a screenshot file
  existing on disk is not proof that it came from the window you named.
- **The dismiss commands are deliberately out of scope here.** `vcli window
  dismiss-dialog` and `vcli window dismiss-window-x11` mutate the live
  Virtuoso's window state. A verification run must not close a window it does
  not own, so drive them only against a session this run started.
