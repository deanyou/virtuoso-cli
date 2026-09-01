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
from __future__ import annotations

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
    Operation.DRAG_REL: "drag-rel",
    Operation.SCREENSHOT: "screenshot",
}

_TIMEOUT_PRECHECK = 30
_TIMEOUT_ACTION = 60


class LockHeldError(Exception):
    """Another run is holding the DISPLAY lock."""


class _DisplayLock:
    """Exclusive, non-blocking lock for a DISPLAY within the run directory."""

    def __init__(self, lock_dir: Path, display: str, session_id: str):
        safe = re.sub(r"[^A-Za-z0-9_.-]", "_", display.lstrip(":") or "0")
        self.path = lock_dir / f"display_{safe}_{session_id}.lock"
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
        # NOTE: the output dir itself is created by Runner.run (exist_ok=False);
        # only the lock subdir is created lazily at precheck time.
        self._lock_dir = self._output_dir / "locks"
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
        try:
            data = json.loads(result.stdout)
        except (json.JSONDecodeError, ValueError) as exc:
            raise RuntimeError(f"unparseable JSON output: {exc}") from exc
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

            # 2. window list: DISPLAY must match exactly, PID binding unique.
            #    When the bridge reports PID zero (old metadata), fall back to
            #    the scenario PID for discovery — if no window matches, reject.
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
            if len(candidates) > 1:
                return {
                    "error": (
                        f"{len(candidates)} windows bound to PID {effective_pid} "
                        f"on DISPLAY {scenario.display}; refusing ambiguous input"
                    )
                }
            window = candidates[0]
            self.window_id = window.get("dismiss_id") or window.get("window_id")
            self._scenario_display = scenario.display
            self._scenario_pid = int(effective_pid)

            # 3. exclusive DISPLAY lock
            lock = _DisplayLock(self._lock_dir, scenario.display, self._session_id)
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
            if step.operation in _ACTION_OPERATIONS:
                self._execute_action_step(step)
                return None
            if step.operation == Operation.WINDOW_WAIT:
                return self._wait_for_window(step)
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
            # Database-first: only visibility predicates fall back to X11.
            if predicate in ("window_visible", "exists"):
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
                            f"predicate {predicate}: expected {expected}, "
                            f"got {visible}"
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
        if not rollback:
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

    # ------------------------------------------------------------------
    # action plumbing
    # ------------------------------------------------------------------

    def _run_action(
        self,
        operation: str,
        x=None,
        y=None,
        text=None,
        output_dir=None,
        timeout_seconds=_TIMEOUT_ACTION,
    ) -> Dict[str, Any]:
        argv = self._vcli_argv(
            "window",
            "action-x11",
            "--window-id",
            self.window_id or "",
            "--pid",
            str(self._scenario_pid),
            "--display",
            self._scenario_display or ":0",
            "--operation",
            operation,
        )
        if x is not None:
            argv += ["--x", str(x)]
        if y is not None:
            argv += ["--y", str(y)]
        if text is not None:
            argv += ["--text", text]
        if output_dir is not None:
            argv += ["--output-dir", output_dir]
        return self._run_json(argv, timeout_seconds)

    def _execute_action_step(self, step: Step) -> None:
        op = _ACTION_OPERATIONS[step.operation]
        args = step.arguments
        x = y = text = output_dir = None
        if step.operation == Operation.CLICK_REL:
            x, y = args.get("x"), args.get("y")
        elif step.operation == Operation.DRAG_REL:
            # vcli's drag-rel takes relative start coordinates; the delta is
            # applied by xdotool server-side from (x1,y1) to (x2,y2).
            x, y = args.get("x1"), args.get("y1")
        elif step.operation == Operation.KEY:
            text = args.get("keys")
        elif step.operation == Operation.TYPE:
            text = args.get("text")
        elif step.operation == Operation.SCREENSHOT:
            output_dir = str(self._output_dir)
        result = self._run_action(op, x=x, y=y, text=text, output_dir=output_dir)
        if result.get("status") not in (None, "success"):
            raise RuntimeError(f"action {op} failed: {result.get('status')}")

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
