"""vgui_runner.local_executor — direct xdotool executor for local X11.

Implements the ``Executor`` protocol against local xdotool, complementing
``LiveExecutor`` (remote vcli) and ``FakeExecutor`` (offline). Use when the
scenario targets a DISPLAY on the same host as the runner.

Unlike LiveExecutor, this executor:
- calls xdotool directly (no vcli indirection);
- binds the target window by PID at precheck time;
- supports SCROLL (xdotool buttons 4/5/6/7), which vcli does not yet expose;
- requires no session_id or vcli binary — only --display and the scenario PID.
"""

import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

from .engine import Executor
from .model import Operation, Scenario, Step

__all__ = ["LocalExecutor"]

# xdotool mouse button mapping for scroll directions.
_SCROLL_BUTTONS = {"up": 4, "down": 5, "left": 6, "right": 7}

_TIMEOUT_PRECHECK = 15
_TIMEOUT_ACTION = 30


def _run(cmd: List[str], timeout: int = _TIMEOUT_ACTION, env: dict = None) -> subprocess.CompletedProcess:
    """Run a command with universal_newlines, raising on timeout."""
    return subprocess.run(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        universal_newlines=True,
        timeout=timeout,
        env=env,
    )


class LocalExecutor(Executor):
    """Executor that drives local X11 via xdotool directly.

    Parameters mirror the CLI wiring: ``--executor local`` requires the
    scenario's ``display`` and ``pid``; an explicit ``--window-id`` overrides
    PID-based discovery.
    """

    def __init__(
        self,
        display: str,
        output_dir: Path,
        window_id: Optional[str] = None,
    ):
        if not display:
            raise ValueError("local executor requires a DISPLAY")
        self._display = display
        self._output_dir = Path(output_dir)
        self._env = dict(os.environ)
        self._env["DISPLAY"] = display
        self.window_id: Optional[str] = window_id
        self._baseline_taken = False
        self._pid: int = 0

    # ------------------------------------------------------------------
    # helpers
    # ------------------------------------------------------------------

    def _xdotool(self, *args: str, timeout: int = _TIMEOUT_ACTION) -> str:
        """Run xdotool with the bound DISPLAY; return stdout."""
        cmd = ["xdotool", *args]
        result = _run(cmd, timeout, env=self._env)
        if result.returncode != 0:
            raise RuntimeError(
                f"xdotool {' '.join(args)} failed: {result.stderr.strip()}"
            )
        return result.stdout.strip()

    def _window_geometry(self) -> Dict[str, int]:
        """Return {x, y, w, h} for the bound window."""
        out = self._xdotool("getwindowgeometry", "--shell", self.window_id or "")
        geo: Dict[str, int] = {}
        for line in out.splitlines():
            if "=" in line:
                k, v = line.strip().split("=", 1)
                try:
                    geo[k.lower()] = int(v)
                except ValueError:
                    pass
        return geo

    def _absolute_coords(self, rx: int, ry: int) -> tuple:
        """Convert window-relative (rx, ry) to absolute screen coordinates."""
        geo = self._window_geometry()
        return geo.get("x", 0) + rx, geo.get("y", 0) + ry

    def _screenshot(self, path: str) -> None:
        """Capture the bound window to ``path`` using ImageMagick import."""
        if shutil.which("import") is None:
            raise RuntimeError("ImageMagick 'import' not found; cannot screenshot")
        cmd = ["import", "-window", self.window_id or "root", path]
        result = _run(cmd, _TIMEOUT_ACTION, env=self._env)
        if result.returncode != 0:
            raise RuntimeError(f"screenshot failed: {result.stderr.strip()}")

    def _find_window_by_pid(self, pid: int) -> Optional[str]:
        """Find the first visible window whose _NET_WM_PID matches."""
        try:
            out = self._xdotool("search", "--onlyvisible", "--pid", str(pid),
                                timeout=_TIMEOUT_PRECHECK)
            ids = out.split()
            return ids[0] if ids else None
        except RuntimeError:
            return None

    # ------------------------------------------------------------------
    # Executor protocol
    # ------------------------------------------------------------------

    def precheck(self, scenario: Scenario) -> Optional[Dict[str, Any]]:
        try:
            if shutil.which("xdotool") is None:
                return {"error": "xdotool not found on PATH"}
            # Verify DISPLAY is reachable.
            try:
                self._xdotool("getdisplaygeometry", timeout=_TIMEOUT_PRECHECK)
            except RuntimeError as exc:
                return {"error": f"DISPLAY {self._display} unreachable: {exc}"}
            # Bind window: explicit --window-id wins, else discover by PID.
            if self.window_id is None:
                wid = self._find_window_by_pid(scenario.pid)
                if wid is None:
                    return {
                        "error": (
                            f"no visible window bound to PID {scenario.pid} "
                            f"on DISPLAY {self._display}"
                        )
                    }
                self.window_id = wid
            self._pid = scenario.pid
            return None
        except Exception as exc:  # noqa: BLE001
            return {"error": f"precheck failed: {exc}"}

    def baseline(self, scenario: Scenario) -> Optional[Dict[str, Any]]:
        if self.window_id is None:
            return {"error": "baseline requires a successful precheck"}
        if self._baseline_taken:
            return None
        try:
            path = str(self._output_dir / "baseline.png")
            self._screenshot(path)
            self._baseline_taken = True
            return None
        except Exception as exc:  # noqa: BLE001
            return {"error": f"baseline screenshot failed: {exc}"}

    def execute(self, step: Step, attempt: int) -> Optional[Dict[str, Any]]:
        if self.window_id is None:
            return {"error": "execute requires a successful precheck"}
        try:
            self._execute_step(step)
            return None
        except Exception as exc:  # noqa: BLE001
            return {"error": str(exc)}

    def verify(self, step: Step, attempt: int) -> Optional[Dict[str, Any]]:
        if self.window_id is None:
            return {"error": "verify requires a successful precheck"}
        predicate = step.verifier.get("predicate", "")
        expected = step.verifier.get("expected")
        try:
            if predicate == "window_exists":
                out = self._xdotool("search", "--onlyvisible", "--name", ".*",
                                    timeout=_TIMEOUT_PRECHECK)
                found = self.window_id in out.split()
                if found != bool(expected):
                    return {
                        "error": (
                            f"predicate window_exists: expected {expected}, "
                            f"got {found}"
                        )
                    }
                return None
            if predicate == "state_matches":
                out = self._xdotool("search", "--onlyvisible", "--name", ".*",
                                    timeout=_TIMEOUT_PRECHECK)
                visible = self.window_id in out.split()
                if visible != bool(expected):
                    return {
                        "error": (
                            f"predicate state_matches: expected visible={expected}, "
                            f"got {visible}"
                        )
                    }
                return None
            if predicate == "title_matches":
                # xdotool getwindowname may return empty; try xprop fallback
                try:
                    title = self._xdotool("getwindowname", self.window_id or "",
                                          timeout=_TIMEOUT_PRECHECK)
                except RuntimeError:
                    title = ""
                if not title:
                    # xprop fallback for windows with empty xdotool name
                    try:
                        out = _run(["xprop", "-id", self.window_id or "", "_NET_WM_NAME"],
                                   _TIMEOUT_PRECHECK, env=self._env)
                        if out.returncode == 0 and "=" in out.stdout:
                            title = out.stdout.split("=", 1)[1].strip().strip('"')
                    except Exception:  # noqa: BLE001
                        title = ""
                if str(expected) not in title:
                    return {
                        "error": (
                            f"predicate title_matches: expected '{expected}' in "
                            f"title, got '{title[:80]}'"
                        )
                    }
                return None
            if predicate == "geometry_matches":
                geo = self._window_geometry()
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
            return {"error": f"unsupported verifier predicate: {predicate}"}
        except Exception as exc:  # noqa: BLE001
            return {"error": str(exc)}

    def recover(
        self, step: Step, attempt: int, rollback: Optional[Dict[str, Any]]
    ) -> Optional[Dict[str, Any]]:
        if self.window_id is None:
            return {"error": "recover requires a successful precheck"}
        # Auto-recovery: if no explicit rollback but the step may have triggered
        # a dialog (KEY/TYPE/CLICK_REL), send Escape as a safety net.
        if not rollback:
            if step.operation in (Operation.KEY, Operation.TYPE, Operation.CLICK_REL):
                try:
                    self._xdotool("windowactivate", self.window_id or "")
                    time.sleep(0.1)
                    self._xdotool("key", "Escape")
                    time.sleep(0.3)
                    return None
                except Exception as exc:  # noqa: BLE001
                    return {"error": f"auto dismiss failed: {exc}"}
            return {"error": "no rollback defined for step; cannot recover"}
        op_str = rollback.get("operation")
        try:
            operation = Operation(op_str)
        except ValueError:
            return {"error": f"unknown rollback operation: {op_str}"}
        args = rollback.get("arguments", {})
        try:
            rb_step = Step(
                id=f"{step.id}-rollback",
                operation=operation,
                arguments=__import__("types").MappingProxyType(dict(args)),
                verifier=__import__("types").MappingProxyType(
                    {"predicate": "window_exists", "expected": True}
                ),
                timeout_seconds=step.timeout_seconds,
                max_retries=0,
                rollback=None,
            )
            self._execute_step(rb_step)
            return None
        except Exception as exc:  # noqa: BLE001
            return {"error": f"rollback failed: {exc}"}

    def close(self) -> None:
        """No resources to release for the local executor."""
        pass

    # ------------------------------------------------------------------
    # action plumbing
    # ------------------------------------------------------------------

    def _execute_step(self, step: Step) -> None:
        op = step.operation
        args = step.arguments
        if op == Operation.WINDOW_ACTIVATE:
            self._xdotool("windowactivate", self.window_id or "")
            time.sleep(0.2)
        elif op == Operation.KEY:
            keys = args.get("keys", "")
            self._xdotool("windowactivate", self.window_id or "")
            time.sleep(0.1)
            self._xdotool("key", *keys.split())
        elif op == Operation.TYPE:
            text = args.get("text", "")
            self._xdotool("windowactivate", self.window_id or "")
            time.sleep(0.2)
            self._xdotool("type", "--clearmodifiers", "--delay", "30", "--", text)
        elif op == Operation.CLICK_REL:
            rx, ry = int(args.get("x", 0)), int(args.get("y", 0))
            button = str(args.get("button", 1))
            ax, ay = self._absolute_coords(rx, ry)
            self._xdotool("mousemove", str(ax), str(ay))
            time.sleep(0.1)
            self._xdotool("click", "--repeat", "1", "--delay", "100", button)
            time.sleep(0.3)
        elif op == Operation.DRAG_REL:
            rx, ry = int(args.get("x", 0)), int(args.get("y", 0))
            button = str(args.get("button", 1))
            sx, sy = self._absolute_coords(0, 0)
            ex, ey = self._absolute_coords(rx, ry)
            self._xdotool("mousemove", str(sx), str(sy))
            time.sleep(0.1)
            self._xdotool("mousedown", button)
            steps = 8
            for i in range(1, steps + 1):
                x = sx + (ex - sx) * i // steps
                y = sy + (ey - sy) * i // steps
                self._xdotool("mousemove", str(x), str(y))
                time.sleep(0.02)
            time.sleep(0.1)
            self._xdotool("mouseup", button)
            time.sleep(0.3)
        elif op == Operation.SCROLL:
            direction = args.get("direction", "down")
            count = int(args.get("count", 3))
            rx = int(args.get("x", 50))
            ry = int(args.get("y", 50))
            button = str(_SCROLL_BUTTONS.get(direction, 5))
            ax, ay = self._absolute_coords(rx, ry)
            self._xdotool("mousemove", str(ax), str(ay))
            time.sleep(0.1)
            self._xdotool("click", "--repeat", str(count), "--delay", "60", button)
            time.sleep(0.3)
        elif op == Operation.SCREENSHOT:
            path = str(self._output_dir / f"window_{self.window_id}.png")
            self._screenshot(path)
        elif op == Operation.DISMISS_DIALOG:
            # Try Escape first (most dialogs), fall back to windowclose
            target = args.get("window_id") or self.window_id
            self._xdotool("windowactivate", target or "")
            time.sleep(0.1)
            self._xdotool("key", "Escape")
            time.sleep(0.3)
        elif op == Operation.CLOSE:
            target = args.get("window_id") or self.window_id
            self._xdotool("windowclose", target or "")
            time.sleep(0.3)
        elif op == Operation.WINDOW_DISCOVER:
            # xdotool search with optional title/class/pid filter
            search_args = ["search", "--onlyvisible"]
            if args.get("title"):
                search_args += ["--name", args["title"]]
            if args.get("class"):
                search_args += ["--class", args["class"]]
            if args.get("pid") is not None:
                search_args += ["--pid", str(args["pid"])]
            if not (args.get("title") or args.get("class") or args.get("pid")):
                search_args += ["--name", ".*"]
            out = self._xdotool(*search_args, timeout=_TIMEOUT_PRECHECK)
            if not out.strip():
                return {"error": "no windows matched discover criteria"}
        elif op == Operation.WINDOW_WAIT:
            self._wait_for_window(step)
        elif op == Operation.VERIFY:
            pass  # verification happens in verify phase
        elif op == Operation.RECOVER:
            pass
        elif op in (Operation.VCLI_LOAD, Operation.VCLI_CALL):
            raise RuntimeError(
                f"operation {op.value} is not supported by the local executor"
            )
        else:
            raise RuntimeError(f"unsupported operation: {op.value}")

    def _wait_for_window(self, step: Step) -> None:
        state = step.arguments.get("state", "visible")
        deadline = time.monotonic() + step.timeout_seconds
        while time.monotonic() < deadline:
            try:
                out = self._xdotool("search", "--onlyvisible", "--name", ".*",
                                    timeout=_TIMEOUT_PRECHECK)
                visible = self.window_id in out.split()
            except RuntimeError:
                visible = False
            if (state == "visible") == visible:
                return
            time.sleep(0.5)
        raise RuntimeError(
            f"window did not become {state} within {step.timeout_seconds}s"
        )
