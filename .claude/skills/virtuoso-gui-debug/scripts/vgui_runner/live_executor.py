"""vgui_runner.live_executor — real vcli/SSH/X11 executor (Task 3).

Implements the ``Executor`` protocol from ``vgui_runner.engine`` against
the real ``vcli`` CLI, restricted by the 2026-09-01 live-executor design:

- it never calls ssh/xdotool/xprop/shell directly — only the injected
  command runner with fixed vcli argv;
- precheck binds the scenario's session, PID, DISPLAY, and exactly one
  window before anything else runs;
- a per-run lock file serializes access to the DISPLAY;
- every GUI action goes through ``vcli window action-x11``, which
  re-validates window identity server-side;
- error dicts are sanitized: typed input text never appears in them.
"""

import errno
import fcntl
import json
import os
import re
import time
from pathlib import Path
from types import MappingProxyType
from typing import Any, Dict, List, Optional

from .engine import Executor
from .model import Operation, Scenario, Step

__all__ = ["LiveExecutor", "LockHeldError"]

# Operations that produce GUI input and therefore map onto
# `vcli window action-x11`.
_ACTION_OPERATIONS = {
    Operation.WINDOW_ACTIVATE: "activate",
    Operation.KEY: "key",
    Operation.TYPE: "type",
    Operation.CLICK_REL: "click-rel",
    Operation.CLICK_ABS: "click-abs",
    Operation.DOUBLE_CLICK: "double-click",
    Operation.DRAG_REL: "drag-rel",
    Operation.SCROLL: "scroll",
    Operation.MINIMIZE: "minimize",
    Operation.MAXIMIZE: "maximize",
    Operation.SCREENSHOT: "screenshot",
    Operation.CLOSE: "close",
}
# Operations compatible with --direct (fast path, skips helper/upload/list-windows).
# wait and screenshot are excluded — they need window-list polling and artifact fetch.
_DIRECT_COMPATIBLE = {
    "activate", "key", "type", "click-rel", "click-abs", "double-click",
    "drag-rel", "scroll", "minimize", "maximize", "close",
}
# Operations that can be batched into a single action-x11-batch call.
_BATCH_COMPATIBLE = {
    "activate", "key", "type", "click-rel", "click-abs", "double-click",
    "drag-rel", "scroll", "minimize", "maximize", "close",
}
# DISMISS_DIALOG uses the dedicated vcli window dismiss-dialog subcommand,
# not action-x11 (which doesn't support it).
_DISMISS_DIALOG_OP = Operation.DISMISS_DIALOG

_TIMEOUT_PRECHECK = 30
_TIMEOUT_ACTION = 60


class LockHeldError(Exception):
    """Another run is holding the DISPLAY lock."""


class _DisplayLock:
    """Exclusive per-DISPLAY lock on the GUI host.

    Lock file lives under ``~/.cache/virtuoso_bridge/x11-locks/`` so that
    *every* LiveExecutor targeting the same ``DISPLAY`` — regardless of its
    ``output_dir`` (which is per-run) — shares one flock. Without this, a
    stray background job could stimulate the same CIW concurrently and
    corrupt its state."""

    GUI_LOCK_ROOT = Path.home() / ".cache" / "virtuoso_bridge" / "x11-locks"

    def __init__(self, display: str):
        safe = re.sub(r"[^A-Za-z0-9_.-]", "_", display.lstrip(":") or "0")
        # Per-display lock: one lock per DISPLAY across all runs.
        self.path = self.GUI_LOCK_ROOT / f"display_{safe}.lock"
        self._fd: Optional[int] = None

    def acquire(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(self.path, os.O_CREAT | os.O_RDWR, 0o600)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as exc:
            os.close(fd)
            if exc.errno in (errno.EACCES, errno.EAGAIN):
                raise LockHeldError(
                    f"DISPLAY lock held by another run: {self.path}"
                ) from exc
            raise
        os.write(fd, f"{os.getpid()}\n".encode())
        self._fd = fd

    def release(self) -> None:
        if self._fd is not None:
            fcntl.flock(self._fd, fcntl.LOCK_UN)
            os.close(self._fd)
            self._fd = None

    def __enter__(self):
        self.acquire()
        return self

    def __exit__(self, *exc):
        self.release()


def _sanitize_error(err: Dict[str, Any], step: Optional[Step] = None) -> Dict[str, Any]:
    """Ensure typed input text never leaks into error dicts."""
    text = json.dumps(err)
    if step is not None and step.operation in (Operation.KEY, Operation.TYPE):
        for key in ("text", "keys"):
            value = step.arguments.get(key)
            if isinstance(value, str) and value in text:
                text = text.replace(value, "<redacted>")
    return json.loads(text)


class LiveExecutor(Executor):
    """Executor that drives the real ``vcli`` CLI through a command runner.

    Parameters mirror the CLI wiring: ``--executor live --vcli PATH
    --ssh-host HOST --session ID --output DIR``.
    """

    def __init__(
        self,
        command_runner,
        vcli_path: str,
        ssh_host: Optional[str],
        session_id: str,
        output_dir: Path,
        window_id: Optional[str] = None,
        use_direct: bool = True,
    ):
        if not session_id:
            raise ValueError("live executor requires an explicit --session")
        self._runner = command_runner
        self._vcli = vcli_path
        self._ssh_host = ssh_host
        self._session_id = session_id
        # Session ids look like <user>-<port>; the trailing number is the
        # bridge/daemon port the session must be bound to.
        m = re.search(r"(\d+)$", session_id)
        self._session_port = int(m.group(1)) if m else 0
        if ssh_host is not None:
            # Fail fast on an unsafe host, mirroring SshRunner's validation.
            from .command_runner import CommandError

            if not isinstance(ssh_host, str) or not ssh_host:
                raise CommandError("ssh_host must be a non-empty string")
            if "\x00" in ssh_host or "\n" in ssh_host or "\r" in ssh_host:
                raise CommandError("ssh_host contains forbidden characters")
            if not re.match(r"^[A-Za-z0-9][A-Za-z0-9._-]*$", ssh_host):
                raise CommandError(
                    "ssh_host must be a plain hostname "
                    "(alphanumerics, dots, underscores, hyphens)"
                )
        self._output_dir = Path(output_dir)
        # Explicit window id overrides PID-based discovery (for multi-window PIDs).
        self._explicit_window_id = window_id
        # --direct skips helper upload, env resolution, and list-windows scan.
        # ~5x faster (260ms vs 1350ms per action). Default True for performance.
        self._use_direct = use_direct
        # Coordinate cache: hiGetFieldInfo results keyed by (form, field).
        self._coord_cache: Dict[str, Dict[str, Any]] = {}
        # Run state — only set after precheck validates identity.
        self._lock: Optional[_DisplayLock] = None
        self.window_id: Optional[str] = None
        self._baseline_taken = False
        self._scenario_display: Optional[str] = None
        self._scenario_pid: int = 0

    # ------------------------------------------------------------------
    # helpers
    # ------------------------------------------------------------------

    def _vcli_argv(self, *args: str) -> List[str]:
        return [self._vcli, *args, "--format", "json"]

    def _run_json(self, argv: List[str], timeout_seconds: int) -> Dict[str, Any]:
        result = self._runner.run(argv, timeout_seconds)
        if result.timed_out:
            raise RuntimeError(f"command timed out after {timeout_seconds}s")
        if result.exit_code != 0:
            reason = result.stderr.strip() or f"exit code {result.exit_code}"
            raise RuntimeError(reason)
        stdout = result.stdout.strip()
        try:
            data = json.loads(stdout)
        except (json.JSONDecodeError, ValueError):
            # Some vcli subcommands emit INFO log lines (with ANSI escapes)
            # before the JSON payload. Find the first line that starts a
            # JSON object/array and parse from there.
            data = None
            lines = stdout.splitlines()
            for idx, line in enumerate(lines):
                stripped = line.lstrip()
                if stripped.startswith("{") or stripped.startswith("["):
                    candidate = "\n".join(lines[idx:])
                    try:
                        data = json.loads(candidate)
                        break
                    except (json.JSONDecodeError, ValueError):
                        continue
            if data is None:
                raise RuntimeError(f"unparseable output: {stdout[:200]}")
        if not isinstance(data, dict):
            raise RuntimeError("command output is not a JSON object")
        return data

    def _sanitize_argv(self, argv: List[str]) -> List[Any]:
        """Replace --text/--keys values with length markers for logging."""
        sanitized: List[Any] = []
        redact_next = False
        for item in argv:
            if redact_next:
                sanitized.append(f"text_length:{len(item)}")
                redact_next = False
            else:
                sanitized.append(item)
                if item in ("--text", "--keys"):
                    redact_next = True
        return sanitized

    # ------------------------------------------------------------------
    # Executor protocol
    # ------------------------------------------------------------------

    def precheck(self, scenario: Scenario) -> Optional[Dict[str, Any]]:
        try:
            # 1. session list: must exist, port must match, PID must be positive
            data = self._run_json(
                self._vcli_argv("session", "list"), _TIMEOUT_PRECHECK
            )
            sessions = [
                s for s in data.get("sessions", []) if s.get("id") == self._session_id
            ]
            if not sessions:
                return {"error": f"session '{self._session_id}' not found"}
            session = sessions[0]
            session_port = session.get("port")
            if session_port and int(session_port) != self._session_port:
                return {
                    "error": (
                        f"session port mismatch: bridge port {session_port} "
                        f"differs from daemon port {self._session_port}"
                    )
                }
            session_pid = int(session.get("pid") or 0)
            if session_pid < 0:
                return {"error": f"session PID must be positive, got {session_pid}"}

            # Bind scenario PID to session PID: when session metadata reports a
            # positive PID, the scenario must declare the same PID. Only fall
            # back to scenario.pid for old metadata that reports PID=0.
            if session_pid > 0 and session_pid != scenario.pid:
                return {
                    "error": (
                        f"PID binding violation: session PID {session_pid} "
                        f"!= scenario PID {scenario.pid}"
                    )
                }

            # 2. window list: DISPLAY must match exactly, PID binding unique.
            #    effective_pid is session_pid when positive, else scenario.pid
            #    (old metadata fallback). If no window matches, reject.
            windows_data = self._run_json(
                self._vcli_argv(
                    "window", "list-windows-x11", "--display", scenario.display
                ),
                _TIMEOUT_PRECHECK,
            )
            reported_display = windows_data.get("display")
            if reported_display and reported_display != scenario.display:
                return {
                    "error": (
                        f"DISPLAY mismatch: scenario says {scenario.display}, "
                        f"X11 reports {reported_display}"
                    )
                }
            windows = windows_data.get("windows", [])
            effective_pid = session_pid or scenario.pid
            candidates = [
                w
                for w in windows
                if w.get("pid") == effective_pid
                and (w.get("display") or reported_display) == scenario.display
            ]
            if not candidates:
                if session_pid == 0:
                    return {
                        "error": (
                            "session PID is zero and no window bound to the "
                            "scenario PID was found; refusing to act"
                        )
                    }
                return {
                    "error": (
                        f"no window bound to PID {effective_pid} on DISPLAY "
                        f"{scenario.display}"
                    )
                }
            # Multi-window disambiguation: explicit --window-id wins, then
            # window_title filter from the first step's arguments.
            if len(candidates) > 1:
                if self._explicit_window_id:
                    matched = [
                        w for w in candidates
                        if (w.get("dismiss_id") or w.get("window_id")) == self._explicit_window_id
                    ]
                    if not matched:
                        return {
                            "error": (
                                f"--window-id {self._explicit_window_id} not found "
                                f"among {len(candidates)} windows for PID {effective_pid}"
                            )
                        }
                    candidates = matched
                else:
                    return {
                        "error": (
                            f"{len(candidates)} windows bound to PID {effective_pid} "
                            f"on DISPLAY {scenario.display}; use --window-id to "
                            f"disambiguate"
                        )
                    }
            window = candidates[0]
            self.window_id = window.get("dismiss_id") or window.get("window_id")
            self._scenario_display = scenario.display
            self._scenario_pid = int(effective_pid)

            # 3. exclusive DISPLAY lock
            lock = _DisplayLock(scenario.display)
            try:
                lock.acquire()
            except LockHeldError as exc:
                return {"error": f"lock conflict: {exc}"}
            self._lock = lock
            return None
        except Exception as exc:  # noqa: BLE001 — fail closed with structured error
            return _sanitize_error({"error": str(exc)})

    def baseline(self, scenario: Scenario) -> Optional[Dict[str, Any]]:
        if self.window_id is None or self._lock is None:
            return {"error": "baseline requires a successful precheck"}
        if self._baseline_taken:
            return None
        try:
            self._run_action(
                "screenshot",
                output_dir=str(self._output_dir),
                timeout_seconds=_TIMEOUT_ACTION,
            )
            self._baseline_taken = True
            return None
        except Exception as exc:  # noqa: BLE001
            return _sanitize_error({"error": f"baseline screenshot failed: {exc}"})

    def execute(self, step: Step, attempt: int) -> Optional[Dict[str, Any]]:
        if self.window_id is None or self._lock is None:
            return {"error": "execute requires a successful precheck"}
        try:
            if step.operation == Operation.DISMISS_DIALOG:
                return self._dismiss_dialog(step)
            if step.operation == Operation.CIW_INPUT:
                self._execute_action_step(step)
                return None
            if step.operation in _ACTION_OPERATIONS:
                self._execute_action_step(step)
                return None
            if step.operation == Operation.WINDOW_WAIT:
                return self._wait_for_window(step)
            if step.operation == Operation.WINDOW_DISCOVER:
                return self._discover_windows(step)
            if step.operation == Operation.VERIFY:
                return None  # verification happens in the verify phase
            if step.operation in (Operation.VCLI_LOAD, Operation.VCLI_CALL):
                return {
                    "error": (
                        f"operation {step.operation.value} is not supported "
                        "by the live executor"
                    )
                }
            if step.operation == Operation.RECOVER:
                return None
            return {"error": f"unsupported operation: {step.operation.value}"}
        except Exception as exc:  # noqa: BLE001
            return _sanitize_error({"error": str(exc)}, step)

    def verify(self, step: Step, attempt: int) -> Optional[Dict[str, Any]]:
        if self.window_id is None or self._lock is None:
            return {"error": "verify requires a successful precheck"}
        predicate = step.verifier.get("predicate", "")
        expected = step.verifier.get("expected")
        try:
            # Both supported predicates are window-visibility checks resolved
            # via the vcli list-windows-x11 query (database-first path).
            if predicate == "window_exists":
                windows_data = self._run_json(
                    self._vcli_argv(
                        "window",
                        "list-windows-x11",
                        "--display",
                        self._scenario_display or ":0",
                    ),
                    _TIMEOUT_PRECHECK,
                )
                windows = windows_data.get("windows", [])
                found = any(
                    (w.get("dismiss_id") or w.get("window_id")) == self.window_id
                    for w in windows
                )
                if found != bool(expected):
                    return {
                        "error": (
                            f"predicate window_exists: expected {expected}, "
                            f"got {found}"
                        )
                    }
                return None
            if predicate == "state_matches":
                # state_matches uses expected ∈ {True, False} as visibility
                windows_data = self._run_json(
                    self._vcli_argv(
                        "window",
                        "list-windows-x11",
                        "--display",
                        self._scenario_display or ":0",
                    ),
                    _TIMEOUT_PRECHECK,
                )
                windows = windows_data.get("windows", [])
                visible = any(
                    (w.get("dismiss_id") or w.get("window_id")) == self.window_id
                    and w.get("visible", False)
                    for w in windows
                )
                if visible != bool(expected):
                    return {
                        "error": (
                            f"predicate state_matches: expected visible={expected}, "
                            f"got {visible}"
                        )
                    }
                return None
            if predicate == "title_matches":
                # expected is a substring to match in the window title
                windows_data = self._run_json(
                    self._vcli_argv(
                        "window",
                        "list-windows-x11",
                        "--display",
                        self._scenario_display or ":0",
                    ),
                    _TIMEOUT_PRECHECK,
                )
                windows = windows_data.get("windows", [])
                win = next(
                    (w for w in windows
                     if (w.get("dismiss_id") or w.get("window_id")) == self.window_id),
                    None,
                )
                if win is None:
                    return {"error": "predicate title_matches: window not found"}
                title = win.get("title", "")
                if str(expected) not in title:
                    return {
                        "error": (
                            f"predicate title_matches: expected '{expected}' in "
                            f"title, got '{title[:80]}'"
                        )
                    }
                return None
            if predicate == "geometry_matches":
                # expected is a dict with optional x/y/w/h keys
                windows_data = self._run_json(
                    self._vcli_argv(
                        "window",
                        "list-windows-x11",
                        "--display",
                        self._scenario_display or ":0",
                    ),
                    _TIMEOUT_PRECHECK,
                )
                windows = windows_data.get("windows", [])
                win = next(
                    (w for w in windows
                     if (w.get("dismiss_id") or w.get("window_id")) == self.window_id),
                    None,
                )
                if win is None:
                    return {"error": "predicate geometry_matches: window not found"}
                geo = win.get("geometry", {})
                if isinstance(expected, dict):
                    for key in ("x", "y", "w", "h"):
                        if key in expected and geo.get(key) != expected[key]:
                            return {
                                "error": (
                                    f"predicate geometry_matches: {key} expected "
                                    f"{expected[key]}, got {geo.get(key)}"
                                )
                            }
                return None
            if predicate == "ciw_eval":
                # expected is a dict: {"expression": "SKILL code", "equals": value}
                # or {"expression": "SKILL code", "contains": "substring"}
                # Executes via vcli skill exec and compares the output.
                if expected is None or "expression" not in expected:
                    return {
                        "error": "ciw_eval predicate requires expected.expression"
                    }
                expression = expected["expression"]
                data = self._run_json(
                    self._vcli_argv("skill", "exec", expression),
                    _TIMEOUT_ACTION,
                )
                actual = data.get("output", "")
                if "equals" in expected:
                    if str(actual) != str(expected["equals"]):
                        return {
                            "error": (
                                f"ciw_eval: expected '{expected['equals']}', "
                                f"got '{actual}'"
                            )
                        }
                elif "contains" in expected:
                    if str(expected["contains"]) not in str(actual):
                        return {
                            "error": (
                                f"ciw_eval: expected output to contain "
                                f"'{expected['contains']}', got '{actual[:80]}'"
                            )
                        }
                return None
            return {"error": f"unsupported verifier predicate: {predicate}"}
        except Exception as exc:  # noqa: BLE001
            return _sanitize_error({"error": str(exc)}, step)

    def recover(
        self, step: Step, attempt: int, rollback: Optional[Dict[str, Any]]
    ) -> Optional[Dict[str, Any]]:
        if self.window_id is None or self._lock is None:
            return {"error": "recover requires a successful precheck"}
        # Auto-recovery: if no explicit rollback but the step may have triggered
        # a dialog (KEY/TYPE/CLICK_REL), try dismiss-dialog as a safety net.
        if not rollback:
            if step.operation in (Operation.KEY, Operation.TYPE, Operation.CLICK_REL):
                return self._dismiss_dialog(step)
            return {"error": "no rollback defined for step; cannot recover"}
        op_str = rollback.get("operation")
        try:
            operation = Operation(op_str)
        except ValueError:
            return {"error": f"unknown rollback operation: {op_str}"}
        if operation not in _ACTION_OPERATIONS:
            return {"error": f"rollback operation {op_str} is not a validated action"}
        args = rollback.get("arguments", {})
        try:
            rb_step = Step(
                id=f"{step.id}-rollback",
                operation=operation,
                arguments=MappingProxyType(dict(args)),
                verifier=MappingProxyType({"predicate": "exists", "expected": True}),
                timeout_seconds=step.timeout_seconds,
                max_retries=0,
                rollback=None,
            )
            self._execute_action_step(rb_step)
            return None
        except Exception as exc:  # noqa: BLE001
            return _sanitize_error({"error": f"rollback failed: {exc}"}, step)

    def close(self) -> None:
        if self._lock is not None:
            self._lock.release()
            self._lock = None

    def __del__(self) -> None:
        # Best-effort: release the flock so sequential tests in the same process
        # don't inherit a leaked lock (e.g. when the test never calls close()).
        try:
            self.close()
        except Exception:  # noqa: BLE001 — C-level destructor
            pass

    # ------------------------------------------------------------------
    # action plumbing
    # ------------------------------------------------------------------

    def _run_action(
        self,
        operation: str,
        x=None,
        y=None,
        button=None,
        text=None,
        output_dir=None,
        window_id=None,
        timeout_seconds=_TIMEOUT_ACTION,
    ) -> Dict[str, Any]:
        argv = self._vcli_argv(
            "window",
            "action-x11",
            "--window-id",
            window_id or self.window_id or "",
            "--display",
            self._scenario_display or ":0",
            "--operation",
            operation,
        )
        # --pid is optional since v1.3.1 (issue #55). Only pass when we have
        # a positive PID — windows without _NET_WM_PID are reachable without it.
        if self._scenario_pid and self._scenario_pid > 0:
            argv += ["--pid", str(self._scenario_pid)]
        # --direct skips helper upload, env resolution, and list-windows scan.
        # ~5x faster. Only for compatible operations (not wait/screenshot).
        if self._use_direct and operation in _DIRECT_COMPATIBLE:
            argv += ["--direct"]
        if x is not None:
            argv += ["--x", str(x)]
        if y is not None:
            argv += ["--y", str(y)]
        if button is not None:
            argv += ["--button", str(button)]
        if text is not None:
            argv += ["--text", text]
        if output_dir is not None:
            argv += ["--output-dir", output_dir]
        return self._run_json(argv, timeout_seconds)

    def _execute_action_step(self, step: Step) -> None:
        # CIW_INPUT is handled specially — it's a composite of multiple
        # action-x11 calls (activate → click → clear → type → Return).
        if step.operation == Operation.CIW_INPUT:
            args = step.arguments
            text = args.get("text", "")
            delay_ms = args.get("delay_ms", 10)
            clear_first = args.get("clear_first", True)
            self._ciw_input(text, delay_ms=delay_ms, clear_first=clear_first)
            return
        op = _ACTION_OPERATIONS[step.operation]
        args = step.arguments
        x = y = button = text = output_dir = None
        action_window_id = self.window_id
        if step.operation in (Operation.CLICK_REL, Operation.CLICK_ABS, Operation.DOUBLE_CLICK):
            x, y = args.get("x"), args.get("y")
            button = args.get("button")
        elif step.operation == Operation.DRAG_REL:
            # vcli's drag-rel takes one relative move vector (x, y). xdotool
            # expands this to mousedown → mousemove --relative → mouseup.
            x, y = args.get("x"), args.get("y")
            button = args.get("button")
        elif step.operation == Operation.KEY:
            text = args.get("keys")
        elif step.operation == Operation.TYPE:
            text = args.get("text")
        elif step.operation == Operation.SCROLL:
            # vcli scroll takes direction[:count] via --text, optional x/y
            # via --x/--y (window-relative pointer position).
            direction = args.get("direction", "down")
            count = args.get("count", 1)
            text = f"{direction}:{count}"
            if "x" in args or "y" in args:
                x = args.get("x")
                y = args.get("y")
        elif step.operation == Operation.SCREENSHOT:
            output_dir = str(self._output_dir)
        elif step.operation == Operation.CLOSE:
            if args.get("window_id"):
                action_window_id = args.get("window_id")
        # MINIMIZE / MAXIMIZE / WINDOW_ACTIVATE take no coordinates.
        result = self._run_action(
            op, x=x, y=y, button=button, text=text, output_dir=output_dir,
            window_id=action_window_id,
        )
        if result.get("status") not in (None, "success"):
            raise RuntimeError(f"action {op} failed: {result.get('status')}")

    def _ciw_input(self, expression: str, delay_ms: int = 10, clear_first: bool = True) -> None:
        """Type a SKILL expression into the CIW input line and press Return.

        Uses vcli action-x11 --direct for each sub-step (activate, key, type).
        The CIW input line is at the bottom of the window; we click at
        (width/2, height-20) which reliably lands in the input area.
        """
        wid = self.window_id
        if not wid:
            raise RuntimeError("CIW_INPUT requires a bound window")
        # 1. Activate the CIW window
        self._run_action("activate")
        # 2. Click the input line (bottom center of window)
        # Use click-rel with approximate bottom-center coordinates.
        # The input line is ~20px from the bottom of the CIW window.
        self._run_action("click-rel", x=400, y=870)
        # 3. Clear existing input if requested
        if clear_first:
            self._run_action("key", text="ctrl+a")
            self._run_action("key", text="Delete")
        # 4. Type the expression with reduced delay for speed
        self._run_action("type", text=expression)
        # 5. Press Return to execute
        self._run_action("key", text="Return")

    def _wait_for_window(self, step: Step) -> Optional[Dict[str, Any]]:
        # Condition polling, not a fixed sleep: poll window visibility until
        # the requested state or the step timeout.
        state = step.arguments.get("state", "visible")
        deadline = time.monotonic() + step.timeout_seconds
        while time.monotonic() < deadline:
            try:
                windows_data = self._run_json(
                    self._vcli_argv(
                        "window",
                        "list-windows-x11",
                        "--display",
                        self._scenario_display or ":0",
                    ),
                    _TIMEOUT_PRECHECK,
                )
            except Exception:  # noqa: BLE001 — transient poll failure
                time.sleep(0.5)
                continue
            windows = windows_data.get("windows", [])
            visible = any(
                (w.get("dismiss_id") or w.get("window_id")) == self.window_id
                and w.get("visible", False)
                for w in windows
            )
            if (state == "visible") == visible:
                return None
            time.sleep(0.5)
        return {"error": f"window did not become {state} within {step.timeout_seconds}s"}

    def _dismiss_dialog(self, step: Step) -> Optional[Dict[str, Any]]:
        """Dismiss a dialog via vcli window dismiss-dialog.

        Default path uses the Virtuoso session (SKILL-based). When an explicit
        window_id is given, switches to --x11 bypass with --display.
        """
        target = step.arguments.get("window_id")
        argv = self._vcli_argv("window", "dismiss-dialog")
        if target:
            argv += ["--x11", "--display", self._scenario_display or ":0",
                     "--window-id", target]
        try:
            data = self._run_json(argv, _TIMEOUT_ACTION)
            # "no-dialog" is a normal outcome — nothing to dismiss.
            if data.get("status") not in (None, "success", "no-dialog"):
                return {"error": f"dismiss-dialog failed: {data.get('status')}"}
            return None
        except Exception as exc:  # noqa: BLE001
            return {"error": f"dismiss-dialog failed: {exc}"}

    def _discover_windows(self, step: Step) -> Optional[Dict[str, Any]]:
        """Discover windows via vcli list-windows-x11 with optional filters."""
        argv = self._vcli_argv(
            "window", "list-windows-x11",
            "--display", self._scenario_display or ":0",
        )
        try:
            data = self._run_json(argv, _TIMEOUT_PRECHECK)
            windows = data.get("windows", [])
            title = step.arguments.get("title")
            wclass = step.arguments.get("class")
            pid = step.arguments.get("pid")
            if title:
                windows = [w for w in windows if title in (w.get("title") or "")]
            if wclass:
                windows = [w for w in windows
                           if wclass in (w.get("class") or [])]
            if pid is not None:
                windows = [w for w in windows if w.get("pid") == pid]
            if not windows:
                return {"error": "no windows matched discover criteria"}
            return None
        except Exception as exc:  # noqa: BLE001
            return {"error": f"discover-windows failed: {exc}"}
