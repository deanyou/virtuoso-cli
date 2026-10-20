"""Unit tests for resources/x11_dismiss_dialog.py — pure helper functions.

These tests intentionally avoid any X server / xwininfo / xprop dependency:
they exercise the pure parsing and decision helpers that the remote-side
helper uses, mirroring the Rust-side guarantees tested in
`src/transport/x11.rs` (Task 1 of the 2026-09-01 live-executor plan):

- window identity components (window id, _NET_WM_PID, display) are parsed
  strictly, and PID zero / missing is rejected;
- title is only ever a narrowing hint, never a sole identity;
- dialog-size classification matches the pinned geometric thresholds.

Run:
    python3 -m unittest discover -s tests/docs -p 'test_x11_dismiss_dialog.py' -v
"""
import importlib.util
import os
import sys
import unittest

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


class HelperDisplayArgvContractTests(unittest.TestCase):
    """End-to-end contract between the Rust builder and the real helper.

    The vendored helper takes its display from a POSITIONAL argv argument
    (`resources/x11_dismiss_dialog.py` parses argv and never reads
    `os.environ["DISPLAY"]`), and it only consults its own `find_x11_env()`
    probe when no positional display was given — in which case it also
    OVERWRITES `os.environ["XAUTHORITY"]` with its own detected cookie.

    These tests drive the real `main()` so a change to the helper's parsing
    contract is caught here, not silently in production (review B1/B2/B3).
    """

    DETECTED_ENV = {"DISPLAY": ":0", "XAUTHORITY": "/auto/detected/cookie"}

    def setUp(self):
        # main() mutates os.environ["DISPLAY"] / ["XAUTHORITY"]; snapshot the
        # values NOW and restore them afterwards so the tests stay
        # order-independent and do not leak into the rest of the process.
        for key in ("DISPLAY", "XAUTHORITY"):
            self.addCleanup(self._restore_env, key, os.environ.get(key))

    @staticmethod
    def _restore_env(key, saved):
        if saved is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = saved

    def _run_helper(self, argv, env_display, env_xauthority, detected_env=None):
        """Run the REAL helper main() with the X11 side effects stubbed.

        Returns (displays_used, find_x11_env_calls) — `displays_used` is what
        the helper actually passed down to its window enumeration.
        """
        import subprocess as sp

        os.environ["DISPLAY"] = env_display
        if env_xauthority is None:
            os.environ.pop("XAUTHORITY", None)
        else:
            os.environ["XAUTHORITY"] = env_xauthority

        original_check_output = sp.check_output
        sp.check_output = lambda cmd, **kwargs: b""
        self.addCleanup(setattr, sp, "check_output", original_check_output)

        probe_calls = []

        def fake_find_x11_env(*args, **kwargs):
            probe_calls.append(True)
            return dict(detected_env or self.DETECTED_ENV)

        original_find_x11_env = x11d.find_x11_env
        x11d.find_x11_env = fake_find_x11_env
        self.addCleanup(setattr, x11d, "find_x11_env", original_find_x11_env)

        # The helper reaches the display through two different entry points:
        # find_dialogs() for --dismiss, discover_windows() for --list-windows.
        displays_used = []
        original_find_dialogs = x11d.find_dialogs
        original_discover_windows = x11d.discover_windows

        def spy_find_dialogs(display):
            displays_used.append(display)
            return original_find_dialogs(display)

        def spy_discover_windows(display):
            displays_used.append(display)
            return original_discover_windows(display)

        x11d.find_dialogs = spy_find_dialogs
        x11d.discover_windows = spy_discover_windows
        self.addCleanup(setattr, x11d, "find_dialogs", original_find_dialogs)
        self.addCleanup(setattr, x11d, "discover_windows", original_discover_windows)

        old_argv = sys.argv
        sys.argv = [HELPER_PATH] + list(argv)
        try:
            with self.assertRaises(SystemExit):
                x11d.main()
        finally:
            sys.argv = old_argv
        return displays_used, probe_calls

    def test_positional_display_is_the_display_the_helper_uses(self):
        """The requested display must be the one the helper actually uses."""
        displays_used, probe_calls = self._run_helper(
            argv=[":99", "--dismiss", "--action", "enter"],
            env_display=":99",
            env_xauthority="/tmp/xauth-cookie",
        )
        self.assertEqual(
            displays_used,
            [":99"],
            "helper must act on the requested display, not on one it picked itself",
        )
        self.assertEqual(os.environ["DISPLAY"], ":99")
        self.assertEqual(
            probe_calls,
            [],
            "helper must not fall back to its own DISPLAY probe when a display was given",
        )

    def test_injected_xauthority_is_not_overwritten(self):
        """A cookie passed in via the env prefix must survive the helper run."""
        self._run_helper(
            argv=[":99", "--dismiss", "--action", "enter"],
            env_display=":99",
            env_xauthority="/tmp/xauth-cookie",
        )
        self.assertEqual(
            os.environ["XAUTHORITY"],
            "/tmp/xauth-cookie",
            "helper must not replace the XAUTHORITY it was given",
        )

    def test_without_positional_display_the_helper_uses_its_own_probe(self):
        """Characterizes the B1 failure mode the Rust side must avoid.

        Without the positional display the helper consults find_x11_env() and
        replaces both DISPLAY and XAUTHORITY — which is why the Rust builder
        must pass the display positionally even though it also exports
        DISPLAY through the env prefix.
        """
        displays_used, probe_calls = self._run_helper(
            argv=["--dismiss", "--action", "enter"],
            env_display=":99",
            env_xauthority="/tmp/xauth-cookie",
        )
        self.assertEqual(
            probe_calls,
            [True],
            "without a positional display the helper falls back to find_x11_env()",
        )
        self.assertEqual(displays_used, [":0"])
        self.assertNotEqual(
            displays_used[0], ":99", "the requested display must not be silently ignored"
        )
        self.assertEqual(
            os.environ["XAUTHORITY"],
            self.DETECTED_ENV["XAUTHORITY"],
            "the helper's own cookie replaces the injected one on the fallback path",
        )

    def test_display_with_host_prefix_is_parsed_as_one_positional_arg(self):
        """The Rust builder shell-escapes the display; the helper sees one arg."""
        displays_used, probe_calls = self._run_helper(
            argv=["host:10.0", "--list-windows"],
            env_display="host:10.0",
            env_xauthority=None,
        )
        self.assertEqual(displays_used, ["host:10.0"])
        self.assertEqual(probe_calls, [])


if __name__ == "__main__":
    unittest.main()
