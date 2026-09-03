"""Tests for vgui_runner.local_executor — LocalExecutor.

All tests mock subprocess.run; no real xdotool or X11 is ever invoked.
Covers: precheck (xdotool missing / DISPLAY unreachable / no window / window
found), baseline screenshot, execute (every supported operation including
SCROLL), verify predicates, recover rollback, and VCLI_LOAD/VCLI_CALL
fail-closed.
"""
import sys
import unittest
from pathlib import Path
from types import MappingProxyType
from unittest import mock

SCRIPTS_DIR = Path(__file__).parent.parent / "scripts"
sys.path.insert(0, str(SCRIPTS_DIR))

from vgui_runner.local_executor import LocalExecutor  # noqa: E402
from vgui_runner.model import CellView, Operation, Scenario, Step  # noqa: E402

PID = 45173
DISPLAY = ":99"
WINDOW_ID = "0x2e01f16"
OUTPUT_DIR = Path("/tmp/vgui_local_test_output")


def _make_scenario() -> Scenario:
    return Scenario(
        version="1.0",
        task_id="test-task",
        session_id="sess-test",
        pid=PID,
        display=DISPLAY,
        cellview=CellView(lib="lib", cell="cell", view="schematic"),
        steps=(),
    )


def _make_step(op: Operation, args: dict = None, verifier: dict = None) -> Step:
    return Step(
        id="step-1",
        operation=op,
        arguments=MappingProxyType(args or {}),
        verifier=MappingProxyType(verifier or {"predicate": "window_exists", "expected": True}),
        timeout_seconds=10,
        max_retries=0,
        rollback=None,
    )


def _completed(returncode=0, stdout="", stderr=""):
    c = mock.Mock()
    c.returncode = returncode
    c.stdout = stdout
    c.stderr = stderr
    return c


class TestLocalExecutorPrecheck(unittest.TestCase):
    def setUp(self):
        self.executor = LocalExecutor(display=DISPLAY, output_dir=OUTPUT_DIR)

    @mock.patch("vgui_runner.local_executor.shutil.which")
    def test_xdotool_missing(self, mock_which):
        mock_which.return_value = None
        err = self.executor.precheck(_make_scenario())
        self.assertIsNotNone(err)
        self.assertIn("xdotool not found", err["error"])

    @mock.patch("vgui_runner.local_executor.shutil.which")
    @mock.patch("vgui_runner.local_executor._run")
    def test_display_unreachable(self, mock_run, mock_which):
        mock_which.return_value = "/usr/bin/xdotool"
        mock_run.return_value = _completed(returncode=1, stderr="cannot open display")
        err = self.executor.precheck(_make_scenario())
        self.assertIsNotNone(err)
        self.assertIn("unreachable", err["error"])

    @mock.patch("vgui_runner.local_executor.shutil.which")
    @mock.patch("vgui_runner.local_executor._run")
    def test_no_window_for_pid(self, mock_run, mock_which):
        mock_which.return_value = "/usr/bin/xdotool"
        # getdisplaygeometry succeeds, search --pid returns empty
        mock_run.side_effect = [
            _completed(stdout="1920 1080"),  # getdisplaygeometry
            _completed(returncode=1, stderr="no window"),  # search --pid
        ]
        err = self.executor.precheck(_make_scenario())
        self.assertIsNotNone(err)
        self.assertIn("no visible window", err["error"])

    @mock.patch("vgui_runner.local_executor.shutil.which")
    @mock.patch("vgui_runner.local_executor._run")
    def test_window_found(self, mock_run, mock_which):
        mock_which.return_value = "/usr/bin/xdotool"
        mock_run.side_effect = [
            _completed(stdout="1920 1080"),  # getdisplaygeometry
            _completed(stdout=WINDOW_ID),  # search --pid
        ]
        err = self.executor.precheck(_make_scenario())
        self.assertIsNone(err)
        self.assertEqual(self.executor.window_id, WINDOW_ID)

    def test_explicit_window_id_skips_discovery(self):
        executor = LocalExecutor(display=DISPLAY, output_dir=OUTPUT_DIR, window_id=WINDOW_ID)
        with mock.patch("vgui_runner.local_executor.shutil.which") as mock_which, \
             mock.patch("vgui_runner.local_executor._run") as mock_run:
            mock_which.return_value = "/usr/bin/xdotool"
            mock_run.return_value = _completed(stdout="1920 1080")
            err = executor.precheck(_make_scenario())
        self.assertIsNone(err)
        self.assertEqual(executor.window_id, WINDOW_ID)
        # Only getdisplaygeometry called, no search --pid
        self.assertEqual(mock_run.call_count, 1)


class TestLocalExecutorExecute(unittest.TestCase):
    def setUp(self):
        self.executor = LocalExecutor(display=DISPLAY, output_dir=OUTPUT_DIR)
        self.executor.window_id = WINDOW_ID
        self.executor._pid = PID

    @mock.patch("vgui_runner.local_executor._run")
    def test_window_activate(self, mock_run):
        mock_run.return_value = _completed()
        err = self.executor.execute(_make_step(Operation.WINDOW_ACTIVATE, {"window_title": "x"}), 0)
        self.assertIsNone(err)
        argv = mock_run.call_args[0][0]
        self.assertIn("windowactivate", argv)
        self.assertIn(WINDOW_ID, argv)

    @mock.patch("vgui_runner.local_executor._run")
    def test_key(self, mock_run):
        mock_run.return_value = _completed()
        err = self.executor.execute(_make_step(Operation.KEY, {"keys": "Return"}), 0)
        self.assertIsNone(err)
        # Last call should be xdotool key Return
        last_argv = mock_run.call_args[0][0]
        self.assertEqual(last_argv[:3], ["xdotool", "key", "Return"])

    @mock.patch("vgui_runner.local_executor._run")
    def test_type(self, mock_run):
        mock_run.return_value = _completed()
        err = self.executor.execute(_make_step(Operation.TYPE, {"text": "hello"}), 0)
        self.assertIsNone(err)
        last_argv = mock_run.call_args[0][0]
        self.assertIn("type", last_argv)
        self.assertIn("hello", last_argv)

    @mock.patch("vgui_runner.local_executor._run")
    def test_click_rel(self, mock_run):
        # getwindowgeometry returns X=100 Y=200
        mock_run.return_value = _completed(stdout="X=100\nY=200\nWIDTH=800\nHEIGHT=600\n")
        err = self.executor.execute(_make_step(Operation.CLICK_REL, {"x": 50, "y": 30}), 0)
        self.assertIsNone(err)
        # mousemove should be at absolute 150, 230
        move_calls = [c[0][0] for c in mock_run.call_args_list if "mousemove" in c[0][0]]
        self.assertTrue(move_calls)
        self.assertIn("150", move_calls[0])
        self.assertIn("230", move_calls[0])

    @mock.patch("vgui_runner.local_executor._run")
    def test_drag_rel(self, mock_run):
        mock_run.return_value = _completed(stdout="X=0\nY=0\nWIDTH=800\nHEIGHT=600\n")
        err = self.executor.execute(_make_step(Operation.DRAG_REL, {"x": 100, "y": 50}), 0)
        self.assertIsNone(err)
        calls = [c[0][0] for c in mock_run.call_args_list]
        self.assertTrue(any("mousedown" in c for c in calls))
        self.assertTrue(any("mouseup" in c for c in calls))

    @mock.patch("vgui_runner.local_executor._run")
    def test_scroll_down(self, mock_run):
        mock_run.return_value = _completed(stdout="X=0\nY=0\nWIDTH=800\nHEIGHT=600\n")
        err = self.executor.execute(_make_step(Operation.SCROLL, {"direction": "down", "count": 5}), 0)
        self.assertIsNone(err)
        click_calls = [c[0][0] for c in mock_run.call_args_list if "click" in c[0][0]]
        self.assertTrue(click_calls)
        argv = click_calls[0]
        self.assertIn("5", argv)  # button 5 = down
        self.assertIn("--repeat", argv)

    @mock.patch("vgui_runner.local_executor._run")
    def test_scroll_up(self, mock_run):
        mock_run.return_value = _completed(stdout="X=0\nY=0\nWIDTH=800\nHEIGHT=600\n")
        err = self.executor.execute(_make_step(Operation.SCROLL, {"direction": "up"}), 0)
        self.assertIsNone(err)
        click_calls = [c[0][0] for c in mock_run.call_args_list if "click" in c[0][0]]
        self.assertTrue(click_calls)
        self.assertIn("4", click_calls[0])  # button 4 = up

    @mock.patch("vgui_runner.local_executor.shutil.which")
    @mock.patch("vgui_runner.local_executor._run")
    def test_screenshot(self, mock_run, mock_which):
        mock_which.return_value = "/usr/bin/import"
        mock_run.return_value = _completed()
        err = self.executor.execute(_make_step(Operation.SCREENSHOT), 0)
        self.assertIsNone(err)
        argv = mock_run.call_args[0][0]
        self.assertEqual(argv[0], "import")
        self.assertIn(WINDOW_ID, argv)

    @mock.patch("vgui_runner.local_executor._run")
    def test_vcli_load_fail_closed(self, mock_run):
        err = self.executor.execute(_make_step(Operation.VCLI_LOAD, {"command": "x"}), 0)
        self.assertIsNotNone(err)
        self.assertIn("not supported", err["error"])

    @mock.patch("vgui_runner.local_executor._run")
    def test_execute_without_precheck(self, mock_run):
        executor = LocalExecutor(display=DISPLAY, output_dir=OUTPUT_DIR)
        err = executor.execute(_make_step(Operation.KEY, {"keys": "x"}), 0)
        self.assertIsNotNone(err)
        self.assertIn("requires a successful precheck", err["error"])


class TestLocalExecutorVerify(unittest.TestCase):
    def setUp(self):
        self.executor = LocalExecutor(display=DISPLAY, output_dir=OUTPUT_DIR)
        self.executor.window_id = WINDOW_ID

    @mock.patch("vgui_runner.local_executor._run")
    def test_window_exists_true(self, mock_run):
        mock_run.return_value = _completed(stdout=f"{WINDOW_ID}\n0xother")
        step = _make_step(Operation.VERIFY, verifier={"predicate": "window_exists", "expected": True})
        err = self.executor.verify(step, 0)
        self.assertIsNone(err)

    @mock.patch("vgui_runner.local_executor._run")
    def test_window_exists_false(self, mock_run):
        mock_run.return_value = _completed(stdout="0xother\n0xanother")
        step = _make_step(Operation.VERIFY, verifier={"predicate": "window_exists", "expected": True})
        err = self.executor.verify(step, 0)
        self.assertIsNotNone(err)
        self.assertIn("expected True", err["error"])

    @mock.patch("vgui_runner.local_executor._run")
    def test_state_matches_visible(self, mock_run):
        mock_run.return_value = _completed(stdout=WINDOW_ID)
        step = _make_step(Operation.VERIFY, verifier={"predicate": "state_matches", "expected": True})
        err = self.executor.verify(step, 0)
        self.assertIsNone(err)

    @mock.patch("vgui_runner.local_executor._run")
    def test_unsupported_predicate(self, mock_run):
        step = _make_step(Operation.VERIFY, verifier={"predicate": "pixel_color", "expected": "#fff"})
        err = self.executor.verify(step, 0)
        self.assertIsNotNone(err)
        self.assertIn("unsupported verifier predicate", err["error"])


class TestLocalExecutorRecover(unittest.TestCase):
    def setUp(self):
        self.executor = LocalExecutor(display=DISPLAY, output_dir=OUTPUT_DIR)
        self.executor.window_id = WINDOW_ID

    @mock.patch("vgui_runner.local_executor._run")
    def test_recover_with_rollback(self, mock_run):
        mock_run.return_value = _completed()
        step = _make_step(Operation.KEY, {"keys": "a"})
        rollback = {"operation": "KEY", "arguments": {"keys": "Escape"}}
        err = self.executor.recover(step, 0, rollback)
        self.assertIsNone(err)

    def test_recover_without_rollback(self):
        step = _make_step(Operation.KEY, {"keys": "a"})
        err = self.executor.recover(step, 0, None)
        self.assertIsNotNone(err)
        self.assertIn("no rollback", err["error"])

    @mock.patch("vgui_runner.local_executor._run")
    def test_recover_unknown_operation(self, mock_run):
        step = _make_step(Operation.KEY, {"keys": "a"})
        rollback = {"operation": "UNKNOWN_OP", "arguments": {}}
        err = self.executor.recover(step, 0, rollback)
        self.assertIsNotNone(err)
        self.assertIn("unknown rollback operation", err["error"])


class TestLocalExecutorBaseline(unittest.TestCase):
    def setUp(self):
        self.executor = LocalExecutor(display=DISPLAY, output_dir=OUTPUT_DIR)
        self.executor.window_id = WINDOW_ID

    @mock.patch("vgui_runner.local_executor.shutil.which")
    @mock.patch("vgui_runner.local_executor._run")
    def test_baseline_screenshot(self, mock_run, mock_which):
        mock_which.return_value = "/usr/bin/import"
        mock_run.return_value = _completed()
        err = self.executor.baseline(_make_scenario())
        self.assertIsNone(err)
        self.assertTrue(self.executor._baseline_taken)

    def test_baseline_without_precheck(self):
        executor = LocalExecutor(display=DISPLAY, output_dir=OUTPUT_DIR)
        err = executor.baseline(_make_scenario())
        self.assertIsNotNone(err)
        self.assertIn("requires a successful precheck", err["error"])


if __name__ == "__main__":
    unittest.main()
