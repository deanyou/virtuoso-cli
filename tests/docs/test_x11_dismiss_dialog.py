"""Unit tests for resources/x11_dismiss_dialog.py — pure helper functions.

These tests intentionally avoid any X server / xwininfo / xprop dependency:
they exercise the pure parsing and decision helpers that the remote-side
helper uses, mirroring the Rust-side guarantees tested in
`src/transport/x11.rs` (Task 1 of the 2026-09-01 live-executor plan):

- window identity components (window id, _NET_WM_PID, display) are parsed
  strictly, and PID zero / missing is rejected;
- title is only ever a narrowing hint, never a sole identity;
- dialog-size classification matches the pinned geometric thresholds;
- a sent key is not a dismissal: the exact XID must be queried via
  XGetWindowAttributes and only X error 3 (BadWindow) proves destruction
  (port of virtuoso-bridge-lite PR #248, `test_x11_dismiss_outcome.py`).

Run:
    python3 -m unittest discover -s tests/docs -p 'test_x11_dismiss_dialog.py' -v
"""
import ctypes
import importlib.util
import io
import json
import os
import subprocess
import sys
import unittest
import unittest.mock
from contextlib import redirect_stdout

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HELPER_PATH = os.path.join(REPO_ROOT, "resources", "x11_dismiss_dialog.py")


def _load_helper():
    spec = importlib.util.spec_from_file_location("x11_dismiss_dialog", HELPER_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules["x11_dismiss_dialog"] = module
    spec.loader.exec_module(module)
    return module


x11d = _load_helper()


class ParseWindowLineTests(unittest.TestCase):
    def test_parses_id_title_and_class(self):
        line = '     0x2e01f16 "Save Changes": ("virtuoso" "Virtuoso")  580x140+1010+378 +1010+378'
        parsed = x11d._parse_window_line(line)
        self.assertEqual(parsed["id"], "0x2e01f16")
        self.assertEqual(parsed["title"], "Save Changes")
        self.assertIn("virtuoso", parsed["class"])

    def test_rejects_non_window_line(self):
        self.assertIsNone(x11d._parse_window_line("  no windows here"))
        self.assertIsNone(x11d._parse_window_line(""))

    def test_class_with_colon_and_space_is_title_not_class(self):
        # A quoted token containing a space is a title, never a WM class.
        line = '  0x1a2b "Untitled: Layout Editor - X" ("libManager" "LM") 10x10+0+0'
        parsed = x11d._parse_window_line(line)
        self.assertEqual(parsed["title"], "Untitled: Layout Editor - X")
        self.assertIn("libManager", parsed["class"])


class NetWmPidTests(unittest.TestCase):
    def _patch_check_output(self, fake):
        import subprocess as sp
        original = sp.check_output
        sp.check_output = fake
        self.addCleanup(setattr, sp, "check_output", original)

    def test_positive_pid_extracted(self):
        def fake(cmd, **kwargs):
            if cmd[:2] == ["xprop", "-id"]:
                return b"_NET_WM_PID = 45173\n"
            raise AssertionError("unexpected command %r" % cmd)

        self._patch_check_output(fake)
        self.assertEqual(x11d._read_net_wm_pid("0x1"), 45173)

    def test_zero_pid_rejected(self):
        def fake(cmd, **kwargs):
            return b"_NET_WM_PID = 0\n"

        self._patch_check_output(fake)
        self.assertIsNone(x11d._read_net_wm_pid("0x1"))

    def test_missing_pid_returns_none(self):
        import subprocess as sp

        def fake(cmd, **kwargs):
            raise sp.CalledProcessError(1, cmd)

        self._patch_check_output(fake)
        self.assertIsNone(x11d._read_net_wm_pid("0x1"))


class GeometryParsingTests(unittest.TestCase):
    def _patch_check_output(self, fake):
        import subprocess as sp
        original = sp.check_output
        sp.check_output = fake
        self.addCleanup(setattr, sp, "check_output", original)

    def test_geometry_parsed_from_xwininfo(self):
        sample = (
            "xwininfo: Window id: 0x2e\n"
            "  Absolute upper-left X:  1010\n"
            "  Absolute upper-left Y:  378\n"
            "  Width: 580\n"
            "  Height: 140\n"
            "  Map State: IsViewable\n"
        )

        def fake(cmd, **kwargs):
            return sample.encode()

        self._patch_check_output(fake)
        geo = x11d._read_window_geometry("0x2e")
        self.assertEqual(geo, {"x": 1010, "y": 378, "w": 580, "h": 140, "mapped": True})

    def test_unmapped_window(self):
        sample = (
            "xwininfo: Window id: 0x2f\n"
            "  Absolute upper-left X:  0\n"
            "  Absolute upper-left Y:  0\n"
            "  Width: 100\n"
            "  Height: 50\n"
            "  Map State: IsUnviewable\n"
        )

        def fake(cmd, **kwargs):
            return sample.encode()

        self._patch_check_output(fake)
        geo = x11d._read_window_geometry("0x2f")
        self.assertFalse(geo["mapped"])

    def test_command_failure_returns_zero_geometry_unmapped(self):
        import subprocess as sp

        def fake(cmd, **kwargs):
            raise sp.CalledProcessError(1, cmd)

        self._patch_check_output(fake)
        geo = x11d._read_window_geometry("0xdead")
        self.assertEqual(geo["w"], 0)
        self.assertFalse(geo["mapped"])


class DialogSizeClassificationTests(unittest.TestCase):
    """The geometric modal test must classify the pinned dialog/frame sizes."""

    def _patch_check_output(self, fake):
        import subprocess as sp
        original = sp.check_output
        sp.check_output = fake
        self.addCleanup(setattr, sp, "check_output", original)

    def test_semantic_dialog_transient(self):
        # WM_TRANSIENT_FOR present → modal dialog
        def fake(cmd, **kwargs):
            return b"WM_TRANSIENT_FOR(WINDOW): window id # 0x3000006\n_NET_WM_WINDOW_TYPE(ATOM) = _NET_WM_WINDOW_TYPE_NORMAL\n"

        self._patch_check_output(fake)
        self.assertEqual(x11d._semantic_dialog_classification("0x2e"), "dialog")

    def test_semantic_dialog_window_type(self):
        def fake(cmd, **kwargs):
            return b"WM_TRANSIENT_FOR:  not found.\n_NET_WM_WINDOW_TYPE(ATOM) = _NET_WM_WINDOW_TYPE_DIALOG\n"

        self._patch_check_output(fake)
        self.assertEqual(x11d._semantic_dialog_classification("0x2e"), "dialog")

    def test_semantic_not_dialog_utility(self):
        # Explicit utility window is never a modal dialog, even if dialog-sized
        def fake(cmd, **kwargs):
            return b"WM_TRANSIENT_FOR:  not found.\n_NET_WM_WINDOW_TYPE(ATOM) = _NET_WM_WINDOW_TYPE_UTILITY\n"

        self._patch_check_output(fake)
        self.assertEqual(x11d._semantic_dialog_classification("0x2e"), "not_dialog")

    def test_semantic_not_dialog_toolbar(self):
        def fake(cmd, **kwargs):
            return b"WM_TRANSIENT_FOR:  not found.\n_NET_WM_WINDOW_TYPE(ATOM) = _NET_WM_WINDOW_TYPE_TOOLBAR\n"

        self._patch_check_output(fake)
        self.assertEqual(x11d._semantic_dialog_classification("0x2e"), "not_dialog")

    def test_semantic_normal_falls_back_to_none(self):
        # NORMAL type with no transient → no signal → geometry heuristic applies
        def fake(cmd, **kwargs):
            return b"WM_TRANSIENT_FOR:  not found.\n_NET_WM_WINDOW_TYPE(ATOM) = _NET_WM_WINDOW_TYPE_NORMAL\n"

        self._patch_check_output(fake)
        self.assertIsNone(x11d._semantic_dialog_classification("0x2e"))

    def test_semantic_xprop_failure_returns_none(self):
        import subprocess as sp

        def fake(cmd, **kwargs):
            raise sp.CalledProcessError(1, "xprop")

        self._patch_check_output(fake)
        self.assertIsNone(x11d._semantic_dialog_classification("0x2e"))

    def test_typical_dialog_is_dialog_sized(self):
        # ADE "Update and Run" style: 580x140
        self.assertTrue(x11d._is_dialog_sized({"w": 580, "h": 140}))

    def test_tall_editor_pane_rejected(self):
        # h > 420 → editor pane
        self.assertFalse(x11d._is_dialog_sized({"w": 600, "h": 500}))

    def test_large_main_frame_rejected(self):
        # w > 1000 && h > 300 → main app frame
        self.assertFalse(x11d._is_dialog_sized({"w": 1200, "h": 700}))

    def test_tiny_window_rejected(self):
        # below MIN_DIALOG_DIM
        self.assertFalse(x11d._is_dialog_sized({"w": 10, "h": 10}))


class ChooseActionTests(unittest.TestCase):
    def test_known_title_maps_to_alt_o(self):
        self.assertEqual(
            x11d.choose_action("ADE Assembler Message 1749", "enter"), "alt-o"
        )

    def test_unknown_title_falls_back_to_requested(self):
        self.assertEqual(x11d.choose_action("Save Changes", "escape"), "escape")

    def test_none_title_falls_back(self):
        self.assertEqual(x11d.choose_action(None, "alt-y"), "alt-y")


class VirtuosoClassTests(unittest.TestCase):
    def test_virtuoso_class_recognized(self):
        self.assertTrue(x11d._is_virtuoso_class(["Virtuoso", "Main"]))

    def test_libmanager_class_recognized(self):
        self.assertTrue(x11d._is_virtuoso_class(["libManager"]))

    def test_other_class_rejected(self):
        self.assertFalse(x11d._is_virtuoso_class(["Firefox", "Navigator"]))

    def test_empty_class_rejected(self):
        self.assertFalse(x11d._is_virtuoso_class([]))


class ParseArgsTests(unittest.TestCase):
    """The Rust caller appends `XAUTHORITY=<path>` as a bare argument.

    Regression: the helper treated every non-flag token as the DISPLAY, so
    `XAUTHORITY=...` overwrote the display and the helper aborted with
    "cannot open display XAUTHORITY=...".
    """

    def test_xauthority_token_does_not_become_the_display(self):
        parsed = x11d.parse_args(
            [":0", "--list-windows", "XAUTHORITY=/run/user/1000/.mutter-xauth"]
        )
        self.assertEqual(parsed["display"], ":0")
        self.assertTrue(parsed["list_windows"])
        self.assertEqual(
            parsed["env"]["XAUTHORITY"], "/run/user/1000/.mutter-xauth"
        )

    def test_display_token_sets_the_display(self):
        parsed = x11d.parse_args(["DISPLAY=:1", "--list-windows"])
        self.assertEqual(parsed["display"], ":1")

    def test_plain_positional_sets_the_display(self):
        parsed = x11d.parse_args([":0.0", "--action", "escape"])
        self.assertEqual(parsed["display"], ":0.0")
        self.assertEqual(parsed["action"], "escape")

    def test_dismiss_window_and_action_flags(self):
        parsed = x11d.parse_args(
            [":0", "--dismiss-window", "0x2e01f16", "--action", "alt-o"]
        )
        self.assertEqual(parsed["dismiss_target"], "0x2e01f16")
        self.assertEqual(parsed["action"], "alt-o")
class _FakeReader:
    """Stand-in for _XWindowStateReader; answers per XID, records queries."""

    def __init__(self, states=None, error=None):
        self.states = states or {}
        self.error = error
        self.queried = []
        self.closed = False

    def window_state(self, xid):
        self.queried.append(xid)
        if self.error is not None:
            raise self.error
        return self.states.get(xid, {"exists": False, "mapped": False})

    def close(self):
        self.closed = True


class _SequenceReader:
    """Reader stub that answers with a scripted sequence of window states.

    The last entry repeats once the script is exhausted, so "always mapped"
    is simply a one-entry script.
    """

    def __init__(self, states):
        self.states = list(states)
        self.queried = []
        self.closed = False

    def window_state(self, xid):
        self.queried.append(xid)
        if len(self.states) > 1:
            return self.states.pop(0)
        return self.states[0]

    def close(self):
        self.closed = True


class _FakeClock:
    """Deterministic clock: sleep() advances it, monotonic() reads it."""

    def __init__(self):
        self.now = 0.0
        self.slept = []

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.slept.append(seconds)
        self.now += seconds


def _x_error_event(code):
    """Build a real _XErrorEvent pointer, as the X server would deliver it."""
    event = x11d._XErrorEvent()
    event.type = 2  # Error
    event.error_code = code
    return ctypes.cast(ctypes.pointer(event), ctypes.c_void_p)


class _FakeXlib:
    """ctypes stand-in for the XGetWindowAttributes call.

    Fills map_state like the server would and routes any simulated protocol
    error through the reader's own error handler, so the captured-code logic
    is exercised instead of bypassed.
    """

    def __init__(self, reader, status=1, map_state=2, error_codes=()):
        self.reader = reader
        self.status = status
        self.map_state = map_state
        self.error_codes = list(error_codes)
        self.queried = []

    def XGetWindowAttributes(self, display, window_id, attrs_ptr):
        self.queried.append(window_id)
        attrs = ctypes.cast(
            attrs_ptr, ctypes.POINTER(x11d._XWindowAttributes)
        ).contents
        attrs.map_state = self.map_state
        for code in self.error_codes:
            self.reader._handle_x_error(None, _x_error_event(code))
        return self.status


def _offline_reader(status=1, map_state=2, error_codes=()):
    """A _XWindowStateReader wired to a fake libX11 (no display, no server)."""
    reader = x11d._XWindowStateReader.__new__(x11d._XWindowStateReader)
    reader._xlib = None
    reader._display = 1
    reader._closed = False
    reader._error_codes = []
    fake = _FakeXlib(reader, status, map_state, error_codes)
    reader._xlib = fake
    return reader, fake


class WindowStateReaderTests(unittest.TestCase):
    """window_state(): only X error 3 (BadWindow) means 'destroyed'."""

    def test_viewable_window_is_still_mapped(self):
        reader, fake = _offline_reader(map_state=x11d.MAP_VIEWABLE)
        state = reader.window_state(0x10)
        self.assertEqual(fake.queried, [0x10])
        self.assertEqual(state, {"exists": True, "mapped": True})

    def test_unmapped_window_is_closed_but_still_exists(self):
        reader, _ = _offline_reader(map_state=x11d.MAP_UNMAPPED)
        self.assertEqual(
            reader.window_state(0x10), {"exists": True, "mapped": False}
        )

    def test_unviewable_child_counts_as_mapped(self):
        # IsUnviewable(1): a mapped child of an unmapped parent is not closed.
        reader, _ = _offline_reader(map_state=1)
        self.assertEqual(
            reader.window_state(0x10), {"exists": True, "mapped": True}
        )

    def test_bad_window_error_means_destroyed(self):
        reader, _ = _offline_reader(error_codes=[x11d.X_BAD_WINDOW])
        self.assertEqual(
            reader.window_state(0x10), {"exists": False, "mapped": False}
        )

    def test_non_badwindow_error_is_a_verification_failure(self):
        for code in (1, 9, 11):  # BadRequest, BadDrawable, BadAlloc
            with self.subTest(code=code):
                reader, _ = _offline_reader(error_codes=[code])
                with self.assertRaises(x11d.XInspectionFailure):
                    reader.window_state(0x10)

    def test_badwindow_plus_another_error_is_not_destruction(self):
        reader, _ = _offline_reader(error_codes=[x11d.X_BAD_WINDOW, 9])
        with self.assertRaises(x11d.XInspectionFailure):
            reader.window_state(0x10)

    def test_zero_status_without_error_is_a_verification_failure(self):
        reader, _ = _offline_reader(status=0)
        with self.assertRaises(x11d.XInspectionFailure):
            reader.window_state(0x10)

    def test_error_codes_do_not_leak_into_the_next_query(self):
        reader, _ = _offline_reader(map_state=x11d.MAP_VIEWABLE)
        reader._handle_x_error(None, _x_error_event(x11d.X_BAD_WINDOW))
        self.assertEqual(reader._error_codes, [x11d.X_BAD_WINDOW])
        self.assertEqual(
            reader.window_state(0x10), {"exists": True, "mapped": True}
        )
        self.assertEqual(reader._error_codes, [])

    def test_error_handler_records_the_code_and_keeps_the_process_alive(self):
        # The default libX11 handler prints and exits; ours must return 0.
        reader, _ = _offline_reader()
        self.assertEqual(reader._handle_x_error(None, _x_error_event(3)), 0)
        self.assertEqual(reader._error_codes, [3])


class VerifyDismissalTests(unittest.TestCase):
    """_verify_dismissal(): a sent key is only *requested* until proven closed."""

    def setUp(self):
        p = unittest.mock.patch.object(x11d.time, "sleep", lambda seconds: None)
        p.start()
        self.addCleanup(p.stop)

    def _patch_reader(self, reader=None, error=None):
        def _open(display):
            if error is not None:
                raise error
            return reader

        p = unittest.mock.patch.object(x11d, "_open_window_state_reader", _open)
        p.start()
        self.addCleanup(p.stop)

    def _patch_clock(self):
        """Drive the verification budget with a deterministic clock."""
        clock = _FakeClock()
        p = unittest.mock.patch.object(x11d.time, "sleep", clock.sleep)
        p.start()
        self.addCleanup(p.stop)
        p = unittest.mock.patch.object(x11d, "_monotonic", clock.monotonic)
        p.start()
        self.addCleanup(p.stop)
        return clock

    @staticmethod
    def _requested(**overrides):
        result = {
            "window_id": "0x10",
            "requested_window_id": "0x10",
            "resolved_window_id": "0x10",
            "child": "0x10",
            "title": "Save Changes",
            "status": "requested",
            "action_sent": True,
            "closure_confirmed": False,
            "still_mapped": None,
            "action": "enter",
            "x": 0, "y": 0, "w": 580, "h": 140,
        }
        result.update(overrides)
        return result

    def _verify(self, result, reader=None, error=None):
        self._patch_reader(reader=reader, error=error)
        return x11d._verify_dismissal(result, ":7")

    def test_verified_closed_reports_dismissed(self):
        reader = _FakeReader({0x10: {"exists": False, "mapped": False}})
        out = self._verify(self._requested(), reader=reader)
        self.assertEqual(reader.queried, [0x10])
        self.assertEqual(out["status"], "closed")
        self.assertIs(out["closure_confirmed"], True)
        self.assertIs(out["still_mapped"], False)
        self.assertEqual(out["dismissed"], "0x10")
        self.assertTrue(reader.closed)

    def test_unmapped_but_alive_window_is_verified_closed(self):
        reader = _FakeReader({0x10: {"exists": True, "mapped": False}})
        out = self._verify(self._requested(), reader=reader)
        self.assertEqual(out["status"], "closed")
        self.assertEqual(out["dismissed"], "0x10")

    def test_still_mapped_window_is_not_dismissed(self):
        reader = _FakeReader({0x10: {"exists": True, "mapped": True}})
        out = self._verify(self._requested(), reader=reader)
        self.assertEqual(out["status"], "requested")
        self.assertIs(out["closure_confirmed"], False)
        self.assertIs(out["still_mapped"], True)
        self.assertNotIn("dismissed", out)

    def test_unavailable_display_never_confirms_closure(self):
        out = self._verify(
            self._requested(), error=x11d.XInspectionFailure("cannot open display")
        )
        self.assertEqual(out["status"], "requested")
        self.assertIsNone(out["still_mapped"])
        self.assertIs(out["closure_confirmed"], False)
        self.assertNotIn("dismissed", out)
        self.assertIn("cannot open display", out["verification_error"])

    def test_state_query_failure_never_confirms_closure(self):
        reader = _FakeReader(error=x11d.XInspectionFailure("cannot read attributes"))
        out = self._verify(self._requested(), reader=reader)
        self.assertNotIn("dismissed", out)
        self.assertIs(out["closure_confirmed"], False)
        self.assertIn("cannot read attributes", out["verification_error"])

    def test_unexpected_failure_never_confirms_closure(self):
        reader = _FakeReader(error=RuntimeError("ctypes blew up"))
        out = self._verify(self._requested(), reader=reader)
        self.assertNotIn("dismissed", out)
        self.assertIn("ctypes blew up", out["verification_error"])

    def test_malformed_target_id_never_confirms_closure(self):
        reader = _FakeReader()
        out = self._verify(self._requested(child="not-a-window"), reader=reader)
        self.assertNotIn("dismissed", out)
        self.assertEqual(reader.queried, [])
        self.assertIn("verification_error", out)

    def test_legacy_dismissed_key_is_not_trusted(self):
        # An old helper (or a caller) claiming "dismissed" with no closure
        # evidence must not survive verification.
        out = self._verify(
            self._requested(status=None, dismissed="0x10"),
            reader=_FakeReader({0x10: {"exists": True, "mapped": True}}),
        )
        self.assertNotIn("dismissed", out)
        self.assertEqual(out["status"], "requested")

    def test_failed_action_is_not_verified(self):
        result = {"error": "libXtst not found", "window_id": "0x10"}
        self._patch_reader(reader=_FakeReader())
        self.assertEqual(x11d._verify_dismissal(result, ":7"), result)

    def test_missing_target_is_reported_not_assumed_closed(self):
        result = {"window_id": "", "action": "enter"}
        self._patch_reader(reader=_FakeReader())
        out = x11d._verify_dismissal(result, ":7")
        self.assertNotIn("dismissed", out)
        self.assertIn("no target window id", out["verification_error"])

    def test_teardown_failure_does_not_discard_the_verdict(self):
        class _BrokenClose(_FakeReader):
            def close(self):
                raise OSError("teardown failed")

        closed = self._verify(
            self._requested(),
            reader=_BrokenClose({0x10: {"exists": False, "mapped": False}}),
        )
        self.assertEqual(closed["status"], "closed")
        self.assertEqual(closed["dismissed"], "0x10")

        failed = self._verify(
            self._requested(),
            reader=_BrokenClose(
                {0x10: {"exists": True, "mapped": True}},
                error=OSError("cannot query"),
            ),
        )
        self.assertNotIn("dismissed", failed)
        self.assertIn("cannot query", failed["verification_error"])


class VerifyPollTests(unittest.TestCase):
    """_verify_dismissal() polls the exact XID instead of sampling it once.

    A single sample after the settle time misjudged a slow dialog (a second
    confirmation, a busy X server) as still mapped.
    """

    def setUp(self):
        clock = _FakeClock()
        p = unittest.mock.patch.object(x11d.time, "sleep", clock.sleep)
        p.start()
        self.addCleanup(p.stop)
        p = unittest.mock.patch.object(x11d, "_monotonic", clock.monotonic)
        p.start()
        self.addCleanup(p.stop)
        self.clock = clock

    @staticmethod
    def _requested(**overrides):
        result = {
            "window_id": "0x10",
            "requested_window_id": "0x10",
            "resolved_window_id": "0x10",
            "child": "0x10",
            "title": "Save Changes",
            "status": "requested",
            "action_sent": True,
            "closure_confirmed": False,
            "still_mapped": None,
            "action": "enter",
            "x": 0, "y": 0, "w": 580, "h": 140,
        }
        result.update(overrides)
        return result

    def _verify(self, result, reader):
        def _open(display):
            return reader

        p = unittest.mock.patch.object(x11d, "_open_window_state_reader", _open)
        p.start()
        self.addCleanup(p.stop)
        return x11d._verify_dismissal(result, ":7")

    def test_slow_dialog_is_confirmed_closed_by_the_poll(self):
        # Still mapped on the first two samples, gone on the third.
        reader = _SequenceReader([
            {"exists": True, "mapped": True},
            {"exists": True, "mapped": True},
            {"exists": True, "mapped": False},
        ])
        out = self._verify(self._requested(), reader)
        self.assertEqual(out["status"], "closed")
        self.assertIs(out["closure_confirmed"], True)
        self.assertIs(out["still_mapped"], False)
        self.assertEqual(out["dismissed"], "0x10")
        self.assertEqual(len(reader.queried), 3)
        # The poll waited between the samples instead of giving up after the
        # settle time.
        self.assertEqual(self.clock.slept[-1], x11d.VERIFY_POLL_INTERVAL_SECONDS)
        self.assertTrue(reader.closed)

    def test_destroyed_window_is_confirmed_on_the_first_sample(self):
        reader = _SequenceReader([{"exists": False, "mapped": False}])
        out = self._verify(self._requested(), reader)
        self.assertEqual(out["status"], "closed")
        self.assertEqual(out["dismissed"], "0x10")
        self.assertEqual(len(reader.queried), 1)

    def test_still_mapped_after_the_budget_is_not_a_dismissal(self):
        reader = _SequenceReader([{"exists": True, "mapped": True}])
        out = self._verify(self._requested(), reader)
        self.assertEqual(out["status"], "requested")
        self.assertIs(out["closure_confirmed"], False)
        self.assertIs(out["still_mapped"], True)
        self.assertNotIn("dismissed", out)
        # The poll kept re-checking until the verification budget was spent.
        self.assertGreater(len(reader.queried), 1)
        self.assertLessEqual(len(reader.queried), x11d.VERIFY_MAX_POLLS + 1)
        spent = self.clock.now - x11d.VERIFY_SETTLE_SECONDS
        self.assertLessEqual(spent, x11d.VERIFY_BUDGET_SECONDS + 1e-6)
        self.assertGreater(spent, 0.0)


class DismissExitCodeTests(unittest.TestCase):
    """main() fails closed: exit 0 only when the window was verified closed."""

    def setUp(self):
        p = unittest.mock.patch.object(x11d.time, "sleep", lambda seconds: None)
        p.start()
        self.addCleanup(p.stop)

    def _run_main(self, argv, dismiss=None, reader=None, reader_error=None,
                  dialogs=None, dismiss_error=None):
        def _patch(obj, name, value):
            p = unittest.mock.patch.object(obj, name, value)
            p.start()
            self.addCleanup(p.stop)

        # The helper's display reachability probe is stubbed out; no X server.
        _patch(x11d.subprocess, "check_output", lambda cmd, **kw: b"")

        def _dismiss(*args, **kwargs):
            if dismiss_error is not None:
                raise dismiss_error
            if callable(dismiss):
                return dismiss(*args, **kwargs)
            return dict(dismiss)

        _patch(x11d, "dismiss_window", _dismiss)
        if dialogs is not None:
            _patch(x11d, "find_dialogs", lambda display: list(dialogs))

        def _open(display):
            if reader_error is not None:
                raise reader_error
            return reader

        _patch(x11d, "_open_window_state_reader", _open)

        original_argv = sys.argv
        sys.argv = ["x11_dismiss_dialog.py"] + argv
        self.addCleanup(setattr, sys, "argv", original_argv)
        out = io.StringIO()
        with redirect_stdout(out):
            with self.assertRaises(SystemExit) as ctx:
                x11d.main()
        records = [
            json.loads(line)
            for line in out.getvalue().splitlines()
            if line.strip()
        ]
        return ctx.exception.code, records

    @staticmethod
    def _requested(**overrides):
        result = {
            "window_id": "0x10",
            "requested_window_id": "0x10",
            "resolved_window_id": "0x10",
            "child": "0x10",
            "title": "Save Changes",
            "status": "requested",
            "action_sent": True,
            "closure_confirmed": False,
            "still_mapped": None,
            "action": "enter",
            "x": 0, "y": 0, "w": 580, "h": 140,
        }
        result.update(overrides)
        return result

    def test_verified_closed_exits_zero(self):
        reader = _FakeReader({0x10: {"exists": False, "mapped": False}})
        code, records = self._run_main(
            [":7", "--dismiss-window", "0x10"],
            dismiss=self._requested(), reader=reader,
        )
        self.assertEqual(code, 0)
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]["status"], "closed")
        self.assertEqual(records[0]["dismissed"], "0x10")
        self.assertEqual(records[0]["requested_window_id"], "0x10")

    def test_still_mapped_exits_nonzero_and_is_not_dismissed(self):
        reader = _FakeReader({0x10: {"exists": True, "mapped": True}})
        code, records = self._run_main(
            [":7", "--dismiss-window", "0x10"],
            dismiss=self._requested(), reader=reader,
        )
        self.assertNotEqual(code, 0)
        self.assertNotIn("dismissed", records[0])
        self.assertIs(records[0]["still_mapped"], True)
        self.assertIs(records[0]["closure_confirmed"], False)

    def test_verification_failure_exits_nonzero_with_error_field(self):
        code, records = self._run_main(
            [":7", "--dismiss-window", "0x10"],
            dismiss=self._requested(),
            reader_error=x11d.XInspectionFailure("cannot open display :7"),
        )
        self.assertNotEqual(code, 0)
        self.assertNotIn("dismissed", records[0])
        self.assertIn("verification_error", records[0])
        self.assertEqual(records[0]["status"], "requested")

    def test_failed_action_exits_nonzero(self):
        code, records = self._run_main(
            [":7", "--dismiss-window", "0x10"],
            dismiss_error=RuntimeError("libXtst not found on remote host"),
        )
        self.assertNotEqual(code, 0)
        self.assertIn("error", records[0])
        self.assertNotIn("dismissed", records[0])

    @staticmethod
    def _per_window_dismiss(display, win_id_str, action, title="", **kwargs):
        """dismiss_window stand-in that reports each dialog it was given."""
        return DismissExitCodeTests._requested(
            window_id=win_id_str, child=win_id_str, title=title
        )

    def test_dismiss_all_exits_zero_when_every_dialog_closed(self):
        dialogs = [
            {"window_id": "0x10", "title": "a", "x": 0, "y": 0, "w": 10, "h": 10},
            {"window_id": "0x11", "title": "b", "x": 0, "y": 0, "w": 10, "h": 10},
        ]
        reader = _FakeReader({
            0x10: {"exists": False, "mapped": False},
            0x11: {"exists": True, "mapped": False},
        })
        code, records = self._run_main(
            [":7", "--dismiss"], dialogs=dialogs, reader=reader,
            dismiss=self._per_window_dismiss,
        )
        self.assertEqual(code, 0)
        outcomes = [r for r in records if "status" in r]
        self.assertEqual([r["status"] for r in outcomes], ["closed", "closed"])
        self.assertEqual([r["dismissed"] for r in outcomes], ["0x10", "0x11"])
        self.assertEqual(sorted(reader.queried), [0x10, 0x11])

    def test_dismiss_all_exits_nonzero_when_one_dialog_stays_mapped(self):
        dialogs = [
            {"window_id": "0x10", "title": "a", "x": 0, "y": 0, "w": 10, "h": 10},
            {"window_id": "0x11", "title": "b", "x": 0, "y": 0, "w": 10, "h": 10},
        ]
        reader = _FakeReader({
            0x10: {"exists": True, "mapped": True},
            0x11: {"exists": False, "mapped": False},
        })
        code, records = self._run_main(
            [":7", "--dismiss"], dialogs=dialogs, reader=reader,
            dismiss=self._per_window_dismiss,
        )
        self.assertNotEqual(code, 0)
        outcomes = [r for r in records if "status" in r]
        self.assertEqual([r["status"] for r in outcomes], ["requested", "closed"])
        self.assertNotIn("dismissed", outcomes[0])
        self.assertEqual(outcomes[1]["dismissed"], "0x11")


class XErrorHandlerInstallationTests(unittest.TestCase):
    """The X error handler must be installed BEFORE any X call.

    `dismiss_window` runs `XRaiseWindow` / `XSetInputFocus` before the state
    reader exists. A handler installed only by the reader left that phase
    unprotected: a BadWindow hit libX11's default handler, which kills the
    process before a single record is printed.
    """

    def setUp(self):
        p = unittest.mock.patch.object(x11d.time, "sleep", lambda seconds: None)
        p.start()
        self.addCleanup(p.stop)

    def _run_main(self, argv, dismiss):
        order = []

        def _install():
            order.append("install")
            return True

        def _dismiss(*args, **kwargs):
            order.append("dismiss")
            return dismiss(*args, **kwargs)

        p = unittest.mock.patch.object(x11d, "install_x_error_handler", _install)
        p.start()
        self.addCleanup(p.stop)

        p = unittest.mock.patch.object(
            x11d.subprocess, "check_output", lambda cmd, **kw: b""
        )
        p.start()
        self.addCleanup(p.stop)

        p = unittest.mock.patch.object(x11d, "dismiss_window", _dismiss)
        p.start()
        self.addCleanup(p.stop)

        def _open(display):
            return _FakeReader({0x10: {"exists": False, "mapped": False}})

        p = unittest.mock.patch.object(x11d, "_open_window_state_reader", _open)
        p.start()
        self.addCleanup(p.stop)

        original_argv = sys.argv
        sys.argv = ["x11_dismiss_dialog.py"] + argv
        self.addCleanup(setattr, sys, "argv", original_argv)
        out = io.StringIO()
        with redirect_stdout(out):
            with self.assertRaises(SystemExit):
                x11d.main()
        return order

    def test_handler_is_installed_before_dismiss_window_runs(self):
        requested = {
            "window_id": "0x10",
            "requested_window_id": "0x10",
            "resolved_window_id": "0x10",
            "child": "0x10",
            "title": "Save Changes",
            "status": "requested",
            "action_sent": True,
            "closure_confirmed": False,
            "still_mapped": None,
            "action": "enter",
            "x": 0, "y": 0, "w": 580, "h": 140,
        }
        order = self._run_main([":7", "--dismiss-window", "0x10"], lambda *a, **kw: dict(requested))
        self.assertIn("install", order, order)
        self.assertIn("dismiss", order, order)
        self.assertLess(
            order.index("install"),
            order.index("dismiss"),
            "the X error handler must be installed before dismiss_window runs: %r" % (order,),
        )

    def test_handler_is_installed_before_the_dismiss_all_loop(self):
        requested = {
            "window_id": "0x10",
            "requested_window_id": "0x10",
            "resolved_window_id": "0x10",
            "child": "0x10",
            "title": "Save Changes",
            "status": "requested",
            "action_sent": True,
            "closure_confirmed": False,
            "still_mapped": None,
            "action": "enter",
            "x": 0, "y": 0, "w": 580, "h": 140,
        }
        p = unittest.mock.patch.object(
            x11d, "find_dialogs",
            lambda display: [
                {"window_id": "0x10", "title": "a", "x": 0, "y": 0, "w": 10, "h": 10}
            ],
        )
        p.start()
        self.addCleanup(p.stop)
        order = self._run_main([":7", "--dismiss"], lambda *a, **kw: dict(requested))
        self.assertLess(
            order.index("install"),
            order.index("dismiss"),
            "the X error handler must be installed before dismiss_window runs: %r" % (order,),
        )


if __name__ == "__main__":
    unittest.main()
