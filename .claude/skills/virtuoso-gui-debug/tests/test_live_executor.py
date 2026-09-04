"""Tests for vgui_runner.live_executor — LiveExecutor (Task 3).

All tests run against an injected fake command runner; no ssh, vcli, or
X11 is ever invoked. Covers the 2026-09-01 plan requirements:

- precheck: session missing / port mismatch / PID zero / PID fallback
  failure / DISPLAY mismatch / zero or multiple windows / lock conflict;
- baseline: sanitized screenshot without typed input;
- execute: every DSL operation maps to a fixed vcli argv;
- verify: database-first predicate via vcli, X11 only for visibility;
- recover: only already-validated rollback operations;
- input sanitization: typed text never appears in errors or argv logs.
"""
import json
import sys
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).parent.parent / "scripts"
sys.path.insert(0, str(SCRIPTS_DIR))

from vgui_runner.command_runner import CommandResult  # noqa: E402
from vgui_runner.engine import Executor, StepPhase  # noqa: E402
from vgui_runner.live_executor import LiveExecutor, LockHeldError  # noqa: E402
from vgui_runner.model import CellView, Scenario, Step  # noqa: E402

VCLI = "/usr/bin/vcli"
SESSION = "dean-user1-34929"
PID = 45173
DISPLAY = ":0"
PORT = 34929
WINDOW_ID = "0x2e01f16"


def res(exit_code=0, stdout="{}", stderr="", timed_out=False):
    return CommandResult(
        exit_code=exit_code,
        stdout=stdout,
        stderr=stderr,
        duration_ms=4,
        timed_out=timed_out,
    )


def sessions_json(sessions):
    return json.dumps(
        {
            "status": "success",
            "count": len(sessions),
            "sessions": [
                {
                    "id": s[0],
                    "port": s[1],
                    "pid": s[2],
                    "host": "compute-eda-42",
                    "user": "user1",
                    "created": "2026-09-01T10:00:00Z",
                }
                for s in sessions
            ],
        }
    )


SESSIONS_OK = sessions_json([(SESSION, PORT, PID)])

WINDOWS_ONE = json.dumps(
    {
        "display": DISPLAY,
        "xauthority": "/home/user1/.Xauthority",
        "count": 1,
        "windows": [
            {
                "frame_id": "0x1a",
                "window_id": WINDOW_ID,
                "dismiss_id": WINDOW_ID,
                "display": DISPLAY,
                "title": "Virtuoso Schematic Editor : myLib/myCell/schematic",
                "class": ["virtuoso"],
                "geometry": {"x": 0, "y": 0, "w": 1200, "h": 800},
                "pid": PID,
                "visible": True,
            }
        ],
    }
)


class FakeRunner:
    """Records argvs; replies from a per-test script of CommandResults.

    A script entry may also be an Exception to raise, or a callable
    receiving the argv and returning a CommandResult.
    """

    def __init__(self, script=None):
        self.script = list(script or [])
        self.argvs = []
        self.lock_paths = []

    def run(self, argv, timeout_seconds):
        self.argvs.append(list(argv))
        if self.script:
            item = self.script.pop(0)
        else:
            item = res(stdout="{}")
        if isinstance(item, Exception):
            raise item
        if callable(item):
            return item(list(argv))
        return item


def make_scenario(steps=(), session_id=SESSION, pid=PID, display=DISPLAY):
    return Scenario(
        version="1.0",
        task_id="task-1",
        session_id=session_id,
        pid=pid,
        display=display,
        cellview=CellView(lib="myLib", cell="myCell", view="schematic"),
        steps=tuple(steps),
    )


def make_step(step_id="s1", operation="WINDOW_ACTIVATE", arguments=None,
              verifier=None, timeout_seconds=30, max_retries=0, rollback=None):
    from types import MappingProxyType

    return Step(
        id=step_id,
        operation=operation,
        arguments=MappingProxyType(arguments or {}),
        verifier=MappingProxyType(verifier or {"predicate": "window_exists", "expected": True}),
        timeout_seconds=timeout_seconds,
        max_retries=max_retries,
        rollback=MappingProxyType(rollback) if rollback else None,
    )


def make_executor(runner, tmpdir, **kwargs):
    params = dict(
        command_runner=runner,
        vcli_path=VCLI,
        ssh_host=None,
        session_id=SESSION,
        output_dir=Path(tmpdir),
    )
    params.update(kwargs)
    return LiveExecutor(**params)


class TestLiveExecutorConstruction(unittest.TestCase):
    def test_implements_executor_protocol(self):
        self.assertTrue(issubclass(LiveExecutor, Executor))

    def test_rejects_empty_session(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ValueError):
                make_executor(FakeRunner(), tmp, session_id="")

    def test_ssh_host_validated_by_runner(self):
        import tempfile

        from vgui_runner.command_runner import CommandError

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(CommandError):
                LiveExecutor(
                    command_runner=FakeRunner(),
                    vcli_path=VCLI,
                    ssh_host="bad host\n",
                    session_id=SESSION,
                    output_dir=Path(tmp),
                )


class TestPrecheck(unittest.TestCase):
    def _precheck(self, script, tmpdir):
        runner = FakeRunner(script)
        ex = make_executor(runner, tmpdir)
        return ex, ex.precheck(make_scenario()), runner

    def test_happy_path(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex, err, runner = self._precheck(
                [res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)], tmp
            )
            self.assertIsNone(err, f"precheck failed: {err}")
            # session list then window list
            self.assertIn("session", " ".join(runner.argvs[0]))
            self.assertIn("list-windows-x11", " ".join(runner.argvs[1]))
            # resolved window id is stored as run state
            self.assertEqual(ex.window_id, WINDOW_ID)

    def test_session_missing(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            _, err, _ = self._precheck([res(stdout=sessions_json([]))], tmp)
            self.assertIsNotNone(err)
            self.assertIn("session", err["error"])

    def test_port_mismatch(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            _, err, _ = self._precheck(
                [res(stdout=sessions_json([(SESSION, 9999, PID)]))], tmp
            )
            self.assertIsNotNone(err)
            self.assertIn("port", err["error"])

    def test_pid_zero_rejected(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            _, err, _ = self._precheck(
                [res(stdout=sessions_json([(SESSION, PORT, 0)]))], tmp
            )
            self.assertIsNotNone(err)
            self.assertIn("PID", err["error"])

    def test_pid_fallback_failure(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            # bridge reports zero PID; fallback window-list also has zero pid
            windows_no_pid = json.dumps(
                {
                    "display": DISPLAY,
                    "count": 1,
                    "windows": [
                        {
                            "frame_id": "0x1a",
                            "window_id": WINDOW_ID,
                            "dismiss_id": WINDOW_ID,
                            "title": "t",
                            "geometry": {"x": 0, "y": 0, "w": 10, "h": 10},
                            "visible": True,
                        }
                    ],
                }
            )
            _, err, _ = self._precheck(
                [res(stdout=sessions_json([(SESSION, PORT, 0)])),
                 res(stdout=windows_no_pid)], tmp
            )
            self.assertIsNotNone(err)

    def test_pid_binding_mismatch_rejected(self):
        """When session reports a positive PID, scenario must match it."""
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            MISMATCH_SESSION = sessions_json([(SESSION, PORT, 99999)])
            _, err, _ = self._precheck([res(stdout=MISMATCH_SESSION)], tmp)
            self.assertIsNotNone(err)
            # Must short-circuit BEFORE window list — no second call expected
            self.assertIn("PID binding", err["error"])

    def test_display_mismatch(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)])
            ex = make_executor(runner, tmp)
            err = ex.precheck(make_scenario(display=":99"))
            self.assertIsNotNone(err)
            self.assertIn("DISPLAY", err["error"])

    def test_zero_windows(self):
        import tempfile

        empty = json.dumps({"display": DISPLAY, "count": 0, "windows": []})
        with tempfile.TemporaryDirectory() as tmp:
            _, err, _ = self._precheck(
                [res(stdout=SESSIONS_OK), res(stdout=empty)], tmp
            )
            self.assertIsNotNone(err)
            self.assertIn("window", err["error"].lower())

    def test_multiple_windows_ambiguous(self):
        import tempfile

        two = json.loads(WINDOWS_ONE)
        two["count"] = 2
        two["windows"].append(dict(two["windows"][0], window_id="0xdead", dismiss_id="0xdead"))
        with tempfile.TemporaryDirectory() as tmp:
            _, err, _ = self._precheck(
                [res(stdout=SESSIONS_OK), res(stdout=json.dumps(two))], tmp
            )
            self.assertIsNotNone(err)
            self.assertIn("window", err["error"].lower())

    def test_nonzero_exit_structured_error(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            _, err, _ = self._precheck([res(exit_code=2, stdout="not json")], tmp)
            self.assertIsNotNone(err)

    def test_timeout_structured_error(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            _, err, _ = self._precheck([res(timed_out=True)], tmp)
            self.assertIsNotNone(err)
            self.assertIn("timed out", err["error"].lower())

    def test_lock_conflict(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            # First executor holds the lock; second one must fail.
            ex1 = make_executor(FakeRunner([res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)]), tmp)
            self.assertIsNone(ex1.precheck(make_scenario()))
            ex2 = make_executor(FakeRunner([res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)]), tmp)
            err = ex2.precheck(make_scenario())
            self.assertIsNotNone(err)
            self.assertIn("lock", err["error"].lower())


class TestBaseline(unittest.TestCase):
    def test_baseline_takes_sanitized_screenshot(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner(
                [res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE),
                 res(stdout=json.dumps({"status": "success", "operation": "screenshot"}))]
            )
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            err = ex.baseline(make_scenario())
            self.assertIsNone(err, f"baseline failed: {err}")
            argv = " ".join(runner.argvs[-1])
            self.assertIn("screenshot", argv)

    def test_baseline_requires_precheck(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex = make_executor(FakeRunner(), tmp)
            err = ex.baseline(make_scenario())
            self.assertIsNotNone(err)


class TestExecuteMapping(unittest.TestCase):
    def _setup(self, tmp):
        runner = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)])
        ex = make_executor(runner, tmp)
        self.assertIsNone(ex.precheck(make_scenario()))
        return ex, runner

    def _with_action(self, tmp, action_result=None):
        runner = FakeRunner(
            [res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE),
             action_result or res(stdout=json.dumps({"status": "success"}))]
        )
        ex = make_executor(runner, tmp)
        self.assertIsNone(ex.precheck(make_scenario()))
        return ex, runner

    def test_window_activate_maps_to_activate(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._with_action(tmp)
            step = make_step(operation="WINDOW_ACTIVATE", arguments={"window_title": "Virtuoso"})
            self.assertIsNone(ex.execute(step, 0))
            argv = " ".join(runner.argvs[-1])
            self.assertIn("action-x11", argv)
            self.assertIn("activate", argv)
            self.assertIn(WINDOW_ID, argv)

    def test_key_maps_to_key_operation(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._with_action(tmp)
            step = make_step(operation="KEY", arguments={"keys": "ctrl+s"})
            self.assertIsNone(ex.execute(step, 0))
            argv = " ".join(runner.argvs[-1])
            self.assertIn("action-x11", argv)
            self.assertIn("key", argv)
            self.assertIn("ctrl+s", argv)

    def test_type_maps_to_type_operation(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._with_action(tmp)
            step = make_step(operation="TYPE", arguments={"text": "hello"})
            self.assertIsNone(ex.execute(step, 0))
            argv = " ".join(runner.argvs[-1])
            self.assertIn("type", argv)
            self.assertIn("hello", argv)

    def test_click_rel_maps_with_coordinates(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._with_action(tmp)
            step = make_step(operation="CLICK_REL", arguments={"x": 10, "y": 20, "button": 1})
            self.assertIsNone(ex.execute(step, 0))
            argv = " ".join(runner.argvs[-1])
            self.assertIn("click-rel", argv)
            self.assertIn("10", argv)
            self.assertIn("20", argv)

    def test_drag_rel_maps_with_coordinates(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._with_action(tmp)
            step = make_step(
                operation="DRAG_REL",
                arguments={"x1": 1, "y1": 2, "x2": 3, "y2": 4, "button": 1},
            )
            self.assertIsNone(ex.execute(step, 0))
            argv = " ".join(runner.argvs[-1])
            self.assertIn("drag-rel", argv)

    def test_screenshot_maps_with_output_dir(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._with_action(tmp)
            step = make_step(operation="SCREENSHOT", arguments={"path": "shot.png"})
            self.assertIsNone(ex.execute(step, 0))
            argv = " ".join(runner.argvs[-1])
            self.assertIn("screenshot", argv)

    def test_failed_action_returns_error(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._with_action(
                tmp, res(exit_code=1, stdout=json.dumps({"status": "failed"}))
            )
            step = make_step(operation="WINDOW_ACTIVATE", arguments={"window_title": "x"})
            err = ex.execute(step, 0)
            self.assertIsNotNone(err)

    def test_execute_requires_precheck(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._setup(tmp)
            # fresh executor without precheck
            ex2 = make_executor(FakeRunner(), tmp)
            step = make_step(operation="WINDOW_ACTIVATE", arguments={})
            err = ex2.execute(step, 0)
            self.assertIsNotNone(err)


class TestVerify(unittest.TestCase):
    def test_database_first_verify_calls_vcli(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner(
                [res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE),
                 res(stdout=WINDOWS_ONE)]
            )
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            step = make_step(
                operation="VERIFY",
                arguments={"predicate": "window_exists", "expected": True},
                verifier={"predicate": "window_exists", "expected": True},
            )
            err = ex.verify(step, 0)
            self.assertIsNone(err, f"verify failed: {err}")
            argv = " ".join(runner.argvs[-1])
            self.assertIn("list-windows-x11", argv)

    def test_verify_failure_on_vcli_error(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner(
                [res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE),
                 res(exit_code=1, stdout="{}")]
            )
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            step = make_step(
                operation="VERIFY",
                arguments={"predicate": "window_exists", "expected": True},
                verifier={"predicate": "window_exists", "expected": True},
            )
            err = ex.verify(step, 0)
            self.assertIsNotNone(err)


class TestRecover(unittest.TestCase):
    def test_recover_rolls_back_with_validated_operation(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner(
                [res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE),
                 res(stdout=json.dumps({"status": "success"}))]
            )
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            step = make_step(
                operation="KEY",
                arguments={"keys": "ctrl+z"},
                rollback={
                    "operation": "KEY",
                    "arguments": {"keys": "ctrl+z"},
                },
            )
            err = ex.recover(step, 0, dict(step.rollback))
            self.assertIsNone(err, f"recover failed: {err}")
            argv = " ".join(runner.argvs[-1])
            self.assertIn("action-x11", argv)

    def test_recover_rejects_unknown_rollback(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)])
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            step = make_step(operation="KEY", arguments={"keys": "ctrl+z"})
            err = ex.recover(step, 0, {"operation": "BOGUS", "arguments": {}})
            self.assertIsNotNone(err)

    def test_recover_without_rollback_returns_error(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)])
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            # Non-dialog operation requires explicit rollback
            step = make_step(operation="WINDOW_ACTIVATE", arguments={"window_title": "x"})
            err = ex.recover(step, 0, None)
            self.assertIsNotNone(err)
            self.assertIn("no rollback", err["error"])

    def test_recover_without_rollback_returns_error_for_key_op(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner([
                res(stdout=SESSIONS_OK),
                res(stdout=WINDOWS_ONE),
            ])
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            step = make_step(operation="KEY", arguments={"keys": "ctrl+z"})
            err = ex.recover(step, 0, None)
            # Strict recovery: no rollback means no action, never auto-dismiss.
            self.assertIsNotNone(err)
            self.assertIn("no rollback", err["error"])


class TestSanitization(unittest.TestCase):
    def test_typed_text_not_in_error_output(self):
        import tempfile

        secret = "SECRETPASSWORD123"
        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner(
                [res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE),
                 res(exit_code=1, stderr=f"boom {secret}")]
            )
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            step = make_step(operation="TYPE", arguments={"text": secret})
            err = ex.execute(step, 0)
            self.assertIsNotNone(err)
            self.assertNotIn(secret, json.dumps(err), f"secret leaked: {err}")

    def test_typed_text_not_in_sanitize_helpers(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)])
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            sanitized = ex._sanitize_argv(
                [VCLI, "window", "action-x11", "--text", "secrettext"]
            )
            self.assertNotIn("secrettext", json.dumps(sanitized))
            self.assertIn("text_length", json.dumps(sanitized))


class TestLockLifecycle(unittest.TestCase):
    def test_release_lock_on_close(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)])
            ex = make_executor(runner, tmp)
            self.assertIsNone(ex.precheck(make_scenario()))
            ex.close()
            # After close, a new executor can take the lock.
            runner2 = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)])
            ex2 = make_executor(runner2, tmp)
            self.assertIsNone(ex2.precheck(make_scenario()))
            ex2.close()


class TestNewOperationsLive(unittest.TestCase):
    """Tests for DISMISS_DIALOG, CLOSE, WINDOW_DISCOVER in live executor."""

    def _prechecked_executor(self, tmp, extra_responses=None):
        script = [res(stdout=SESSIONS_OK), res(stdout=WINDOWS_ONE)]
        if extra_responses:
            script.extend(extra_responses)
        runner = FakeRunner(script)
        ex = make_executor(runner, tmp)
        self.assertIsNone(ex.precheck(make_scenario()))
        return ex, runner

    def test_dismiss_dialog_maps_to_vcli(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._prechecked_executor(
                tmp, [res(stdout='{"status":"success"}')])
            step = make_step(operation="DISMISS_DIALOG", arguments={})
            self.assertIsNone(ex.execute(step, 0))
            # Verify the argv contains dismiss-dialog (dedicated subcommand)
            action_argv = runner.argvs[-1]
            self.assertIn("dismiss-dialog", action_argv)

    def test_dismiss_dialog_with_window_id(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._prechecked_executor(
                tmp, [res(stdout='{"status":"success"}')])
            step = make_step(operation="DISMISS_DIALOG",
                             arguments={"window_id": "0xdeadbeef"})
            self.assertIsNone(ex.execute(step, 0))
            action_argv = runner.argvs[-1]
            self.assertIn("0xdeadbeef", action_argv)

    def test_close_maps_to_vcli(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._prechecked_executor(
                tmp, [res(stdout='{"status":"success"}')])
            step = make_step(operation="CLOSE", arguments={})
            self.assertIsNone(ex.execute(step, 0))
            action_argv = runner.argvs[-1]
            self.assertIn("close", action_argv)

    def test_window_discover_calls_discover_windows(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._prechecked_executor(
                tmp, [res(stdout='{"windows":[{"window_id":"0x1","title":"Virtuoso Editor"}]}')])
            step = make_step(operation="WINDOW_DISCOVER",
                             arguments={"title": "Virtuoso"})
            self.assertIsNone(ex.execute(step, 0))
            discover_argv = runner.argvs[-1]
            self.assertIn("list-windows-x11", discover_argv)

    def test_window_discover_no_match_returns_error(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._prechecked_executor(
                tmp, [res(stdout='{"windows":[]}')])
            step = make_step(operation="WINDOW_DISCOVER", arguments={})
            err = ex.execute(step, 0)
            self.assertIsNotNone(err)
            self.assertIn("no windows matched", err["error"])

    def test_scroll_maps_to_vcli_scroll(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._prechecked_executor(
                tmp, [res(stdout='{"status":"success"}')])
            step = make_step(operation="SCROLL",
                             arguments={"direction": "down", "count": 3,
                                        "x": 40, "y": 60})
            self.assertIsNone(ex.execute(step, 0))
            action_argv = runner.argvs[-1]
            self.assertIn("scroll", action_argv)
            # --text carries direction:count
            self.assertIn("down:3", action_argv)
            self.assertIn("--x", action_argv)
            self.assertIn("40", action_argv)
            self.assertIn("--y", action_argv)
            self.assertIn("60", action_argv)

    def test_scroll_defaults_to_down_1(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex, runner = self._prechecked_executor(
                tmp, [res(stdout='{"status":"success"}')])
            step = make_step(operation="SCROLL", arguments={})
            self.assertIsNone(ex.execute(step, 0))
            action_argv = runner.argvs[-1]
            self.assertIn("down:1", action_argv)


class TestNewVerifierPredicatesLive(unittest.TestCase):
    """Tests for title_matches and geometry_matches in live executor."""

    def _prechecked_executor(self, tmp, verify_response):
        runner = FakeRunner([
            res(stdout=SESSIONS_OK),
            res(stdout=WINDOWS_ONE),
            res(stdout=verify_response),
        ])
        ex = make_executor(runner, tmp)
        self.assertIsNone(ex.precheck(make_scenario()))
        return ex

    def test_title_matches_success(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex = self._prechecked_executor(tmp, WINDOWS_ONE)
            step = make_step(verifier={"predicate": "title_matches",
                                       "expected": "Virtuoso"})
            self.assertIsNone(ex.verify(step, 0))

    def test_title_matches_failure(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex = self._prechecked_executor(tmp, WINDOWS_ONE)
            step = make_step(verifier={"predicate": "title_matches",
                                       "expected": "Nonexistent"})
            err = ex.verify(step, 0)
            self.assertIsNotNone(err)
            self.assertIn("title_matches", err["error"])

    def test_geometry_matches_success(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex = self._prechecked_executor(tmp, WINDOWS_ONE)
            step = make_step(verifier={"predicate": "geometry_matches",
                                       "expected": {"w": 1200, "h": 800}})
            self.assertIsNone(ex.verify(step, 0))

    def test_geometry_matches_failure(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            ex = self._prechecked_executor(tmp, WINDOWS_ONE)
            step = make_step(verifier={"predicate": "geometry_matches",
                                       "expected": {"w": 9999}})
            err = ex.verify(step, 0)
            self.assertIsNotNone(err)
            self.assertIn("geometry_matches", err["error"])


class TestWindowIdDisambiguation(unittest.TestCase):
    """Tests for --window-id override in multi-window PID scenarios."""

    def test_multi_window_without_window_id_rejected(self):
        import tempfile
        windows_two = json.dumps({
            "display": DISPLAY,
            "count": 2,
            "windows": [
                {"window_id": "0x1", "dismiss_id": "0x1", "display": DISPLAY,
                 "title": "Window A", "pid": PID, "visible": True,
                 "geometry": {"x": 0, "y": 0, "w": 100, "h": 100}},
                {"window_id": "0x2", "dismiss_id": "0x2", "display": DISPLAY,
                 "title": "Window B", "pid": PID, "visible": True,
                 "geometry": {"x": 0, "y": 0, "w": 200, "h": 200}},
            ],
        })
        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=windows_two)])
            ex = make_executor(runner, tmp)
            err = ex.precheck(make_scenario())
            self.assertIsNotNone(err)
            self.assertIn("--window-id", err["error"])

    def test_multi_window_with_window_id_succeeds(self):
        import tempfile
        windows_two = json.dumps({
            "display": DISPLAY,
            "count": 2,
            "windows": [
                {"window_id": "0x1", "dismiss_id": "0x1", "display": DISPLAY,
                 "title": "Window A", "pid": PID, "visible": True,
                 "geometry": {"x": 0, "y": 0, "w": 100, "h": 100}},
                {"window_id": "0x2", "dismiss_id": "0x2", "display": DISPLAY,
                 "title": "Window B", "pid": PID, "visible": True,
                 "geometry": {"x": 0, "y": 0, "w": 200, "h": 200}},
            ],
        })
        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=windows_two)])
            ex = make_executor(runner, tmp, window_id="0x2")
            self.assertIsNone(ex.precheck(make_scenario()))
            self.assertEqual(ex.window_id, "0x2")

    def test_multi_window_with_bad_window_id_rejected(self):
        import tempfile
        windows_two = json.dumps({
            "display": DISPLAY,
            "count": 2,
            "windows": [
                {"window_id": "0x1", "dismiss_id": "0x1", "display": DISPLAY,
                 "title": "Window A", "pid": PID, "visible": True,
                 "geometry": {"x": 0, "y": 0, "w": 100, "h": 100}},
                {"window_id": "0x2", "dismiss_id": "0x2", "display": DISPLAY,
                 "title": "Window B", "pid": PID, "visible": True,
                 "geometry": {"x": 0, "y": 0, "w": 200, "h": 200}},
            ],
        })
        with tempfile.TemporaryDirectory() as tmp:
            runner = FakeRunner([res(stdout=SESSIONS_OK), res(stdout=windows_two)])
            ex = make_executor(runner, tmp, window_id="0xdead")
            err = ex.precheck(make_scenario())
            self.assertIsNotNone(err)
            self.assertIn("not found", err["error"])


if __name__ == "__main__":
    unittest.main()
