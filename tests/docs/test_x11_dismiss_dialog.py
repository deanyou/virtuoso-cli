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
