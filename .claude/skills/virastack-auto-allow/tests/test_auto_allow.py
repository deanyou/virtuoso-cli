"""Unit tests for the virastack-auto-allow skill scripts.

Pure/offline only — no Virtuoso session, no X server, no vcli binary:

- resolve_target picks the session, or refuses when the choice is ambiguous;
- find_prompt locates the planted template and returns the Allow label offset;
- a window without any prompt must not match.

Run:
    cd .claude/skills/virastack-auto-allow && python3 -m unittest discover tests
"""

import contextlib
import importlib.util
import io
import json
import os
import sys
import tempfile
import time
import unittest

import numpy as np
from PIL import Image, ImageDraw

SKILL_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(SKILL_DIR)))


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


aa = _load("virastack_auto_allow", os.path.join(SKILL_DIR, "scripts", "auto_allow.py"))
rt = _load("virastack_auto_allow_resolve", os.path.join(SKILL_DIR, "scripts", "resolve_target.py"))


class FindPromptTests(unittest.TestCase):
    def setUp(self):
        self.template = aa.load_template(aa.DEFAULT_TEMPLATE)
        self.img = Image.new("L", (780, 968), 30)
        self.img.paste(Image.open(aa.DEFAULT_TEMPLATE).convert("L"), (58, 694))
        handle, self.path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        self.img.save(self.path)
        self.addCleanup(os.remove, self.path)

    def test_planted_prompt_is_found_with_the_allow_offset(self):
        found = aa.find_prompt(self.path, self.template)
        self.assertIsNotNone(found)
        x, y, score = found
        self.assertAlmostEqual(x, 58 + aa.ALLOW_OFFSET[0], delta=1)
        self.assertAlmostEqual(y, 694 + aa.ALLOW_OFFSET[1], delta=1)
        self.assertGreater(score, aa.MATCH_THRESHOLD)

    def test_window_without_prompt_does_not_match(self):
        blank = Image.new("L", (780, 968), 30)
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        blank.save(path)
        self.addCleanup(os.remove, path)
        self.assertIsNone(aa.find_prompt(path, self.template))

    def test_template_is_loaded_once_not_per_poll(self):
        # The loop must not depend on the template file still existing between
        # polls — a moved template used to crash the running loop.
        template = aa.load_template(aa.DEFAULT_TEMPLATE)
        self.assertIsNotNone(aa.find_prompt(self.path, template))

    def test_a_truncated_capture_does_not_kill_the_loop(self):
        # A capture can leave a truncated file behind. The loop has to survive
        # it: an unreadable screenshot means "no prompt this time".
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        with open(path, "wb") as f:
            f.write(b"\x89PNG\r\n\x1a\n")
        self.addCleanup(os.remove, path)
        self.assertIsNone(aa.find_prompt(path, self.template))
        self.assertIsNone(aa.crop_region(path, (0, 0, 700, 260)))
        self.assertIsNone(aa.command_band(path, (10, 710, 0.99)))


class IdleLimitTests(unittest.TestCase):
    def test_zero_idle_polls_never_stops_the_loop(self):
        # A cellview run (apRunDeviceGeneration) keeps the agent busy for
        # longer than any idle budget; the loop must not quit unnoticed.
        self.assertFalse(aa.idle_limit_reached(0, 1))
        self.assertFalse(aa.idle_limit_reached(0, 100000))

    def test_positive_idle_polls_stops_after_the_limit(self):
        self.assertFalse(aa.idle_limit_reached(3, 2))
        self.assertTrue(aa.idle_limit_reached(3, 3))
        self.assertTrue(aa.idle_limit_reached(3, 4))


class ResolveSessionTests(unittest.TestCase):
    def _patch_vcli_json(self, sessions):
        original = rt.vcli_json
        rt.vcli_json = lambda vcli, session, args: {"sessions": sessions}
        self.addCleanup(setattr, rt, "vcli_json", original)

    def test_single_session_is_selected(self):
        self._patch_vcli_json([{"id": "host-user-1"}])
        self.assertEqual(rt.resolve_session("vcli", ""), "host-user-1")

    def test_explicit_session_wins(self):
        self._patch_vcli_json([{"id": "other"}])
        self.assertEqual(rt.resolve_session("vcli", "explicit"), "explicit")

    def test_ambiguous_sessions_are_refused(self):
        self._patch_vcli_json([{"id": "a"}, {"id": "b"}])
        with self.assertRaises(RuntimeError):
            rt.resolve_session("vcli", "")

    def test_no_session_is_an_error(self):
        self._patch_vcli_json([])
        with self.assertRaises(RuntimeError):
            rt.resolve_session("vcli", "")


class RecoverTargetTests(unittest.TestCase):
    """The loop must find the target again after a Virtuoso/daemon restart."""

    def _patch_run(self, responses):
        original = aa.run
        aa.run = lambda cmd: responses.get(cmd[-1], (1, "", "boom"))
        self.addCleanup(setattr, aa, "run", original)

    def _args(self):
        class A(object):
            pass
        a = A()
        a.vcli = "vcli"
        a.session = "old-session"
        a.window_id = "0xdead"
        a.title_prefix = "ViraStack"
        a.display = ""
        a.display_explicit = False
        return a

    def test_window_is_resolved_by_title_prefix(self):
        self._patch_run({
            "json": (0, json.dumps({"windows": [
                {"window_id": "0x1", "title": "virtuoso"},
                {"window_id": "0x2", "title": "ViraStack : Agent (0)"},
            ]}), ""),
        })
        self.assertEqual(aa.resolve_window("vcli", "s", "ViraStack"), ("0x2", None))

    def test_the_display_is_captured_with_the_window(self):
        # list-windows-x11 reports the display it queried: the action calls
        # have to name it, and ":0" is wrong on a host whose Virtuoso runs
        # on another display.
        self._patch_run({
            "json": (0, json.dumps({"display": ":1", "windows": [
                {"window_id": "0x2", "title": "ViraStack : Agent (0)", "display": ":1"},
            ]}), ""),
        })
        self.assertEqual(aa.resolve_window("vcli", "s", "ViraStack"), ("0x2", ":1"))

    def test_no_matching_window_returns_none(self):
        self._patch_run({"json": (0, json.dumps({"windows": [
            {"window_id": "0x1", "title": "virtuoso"},
        ]}), "")})
        self.assertEqual(aa.resolve_window("vcli", "s", "ViraStack")[0], None)

    def test_dead_session_is_replaced_by_the_live_one(self):
        self._patch_run({
            "json": (0, json.dumps({"sessions": [{"id": "new-session"}]}), ""),
        })
        self.assertEqual(aa.resolve_session("vcli", "old-session"), "new-session")

    def test_live_session_is_kept(self):
        self._patch_run({
            "json": (0, json.dumps({"sessions": [{"id": "old-session"}]}), ""),
        })
        self.assertEqual(aa.resolve_session("vcli", "old-session"), "old-session")

    def test_recover_updates_both_ids(self):
        self._patch_run({
            "json": (0, json.dumps({"sessions": [{"id": "new-session"}]}), ""),
        })
        original = aa.resolve_window
        aa.resolve_window = lambda vcli, session, prefix: ("0xnew", ":1")
        self.addCleanup(setattr, aa, "resolve_window", original)

        args = self._args()
        self.assertTrue(aa.recover_target(args))
        self.assertEqual(args.session, "new-session")
        self.assertEqual(args.window_id, "0xnew")
        # The display is re-resolved too: Virtuoso can come back on
        # another display.
        self.assertEqual(args.display, ":1")

    def test_an_explicit_display_wins_over_the_detected_one(self):
        self._patch_run({
            "json": (0, json.dumps({"sessions": [{"id": "new-session"}]}), ""),
        })
        original = aa.resolve_window
        aa.resolve_window = lambda vcli, session, prefix: ("0xnew", ":1")
        self.addCleanup(setattr, aa, "resolve_window", original)

        args = self._args()
        args.display = ":7"
        args.display_explicit = True
        self.assertTrue(aa.recover_target(args))
        self.assertEqual(args.display, ":7")

    def test_recover_is_a_noop_when_the_target_is_unchanged(self):
        self._patch_run({
            "json": (0, json.dumps({"sessions": [{"id": "old-session"}]}), ""),
        })
        original = aa.resolve_window
        aa.resolve_window = lambda vcli, session, prefix: ("0xdead", None)
        self.addCleanup(setattr, aa, "resolve_window", original)

        args = self._args()
        self.assertFalse(aa.recover_target(args))


class CommandSignatureTests(unittest.TestCase):
    """The repeat counter keys on the command text, not on the whole prompt."""

    def _prompt_image(self, command_text):
        img = Image.new("L", (780, 968), 30)
        img.paste(Image.open(aa.DEFAULT_TEMPLATE).convert("L"), (58, 694))
        drawer = ImageDraw.Draw(img)
        # The command line sits ~95px above the Allow label (694+16).
        drawer.text((35, 694 + 16 - 95), command_text, fill=220)
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        img.save(path)
        self.addCleanup(os.remove, path)
        return path

    def _band(self, command_text):
        found = aa.find_prompt(self._prompt_image(command_text), aa.load_template(aa.DEFAULT_TEMPLATE))
        self.assertIsNotNone(found)
        return aa.command_band(self._prompt_image(command_text), found)

    def test_same_command_gives_the_same_signature(self):
        a = self._band("/tmp/mkdir.sh")
        b = self._band("/tmp/mkdir.sh")
        self.assertTrue(aa.same_command(a, b))

    def test_different_command_gives_a_different_signature(self):
        a = self._band("/tmp/mkdir.sh")
        b = self._band("/tmp/spectre_run.sh")
        self.assertFalse(aa.same_command(a, b))

    def test_a_re_rendered_command_is_still_the_same_command(self):
        # The same command drawn one pixel lower must not look like a new
        # command — a byte-exact hash would, and the repeat counter would
        # never reach the threshold.
        img = Image.new("L", (780, 968), 30)
        img.paste(Image.open(aa.DEFAULT_TEMPLATE).convert("L"), (58, 694))
        drawer = ImageDraw.Draw(img)
        drawer.text((35, 694 + 16 - 95), "/tmp/mkdir.sh", fill=220)
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        img.save(path)
        self.addCleanup(os.remove, path)
        template = aa.load_template(aa.DEFAULT_TEMPLATE)
        found = aa.find_prompt(path, template)
        band = aa.command_band(path, found)
        # Move the band down by one pixel without wrapping the rows around.
        shifted = np.zeros_like(band)
        shifted[1:] = band[:-1]
        self.assertTrue(aa.same_command(band, shifted))

    def _card_image(self, command_text):
        """A permission card: the command in a code block, a status line, the
        option rows. The status line is the last text line above the options
        and is the same for every prompt."""
        allow_y = 694 + 16
        img = Image.new("L", (780, 968), 90)
        drawer = ImageDraw.Draw(img)
        # the code block, ~70px above the Allow label
        drawer.rectangle((35, allow_y - 80, 600, allow_y - 50), fill=45)
        drawer.text((40, allow_y - 74), command_text, fill=216)
        # the status line, ~35px above the Allow label
        drawer.text((35, allow_y - 42), "Running script...", fill=255)
        img.paste(Image.open(aa.DEFAULT_TEMPLATE).convert("L"), (58, 694))
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        img.save(path)
        self.addCleanup(os.remove, path)
        return path

    def _long_command_card(self, command_text):
        """A card whose command fills the region that is searched for it.

        The code block then covers more of the region than the card around
        it, and the most common grey of the region is the colour of the code
        block: the command must not be taken for the background.
        """
        allow_y = 694 + 16
        img = Image.new("L", (780, 968), 75)
        drawer = ImageDraw.Draw(img)
        # the code block, from the top of the region down to the status line
        drawer.rectangle((35, allow_y - 180, 740, allow_y - 70), fill=45)
        drawer.text((40, allow_y - 174), command_text, fill=216)
        drawer.text((40, allow_y - 154), "a second line of the command", fill=216)
        drawer.text((40, allow_y - 134), "and a third line", fill=216)
        # the status line, on the background of the card
        drawer.text((35, allow_y - 60), "Running script...", fill=255)
        img.paste(Image.open(aa.DEFAULT_TEMPLATE).convert("L"), (58, 694))
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        img.save(path)
        self.addCleanup(os.remove, path)
        return path

    def _band_of(self, image):
        template = aa.load_template(aa.DEFAULT_TEMPLATE)
        found = aa.find_prompt(image, template)
        self.assertIsNotNone(found)
        return aa.command_band(image, found)

    def test_the_band_is_the_command_and_not_the_status_line(self):
        # The status line above the option rows is the same for every prompt:
        # fingerprinting it would make every request look like a repeat of
        # the same command.
        a = self._band_of(self._card_image("/tmp/mkdir.sh"))
        b = self._band_of(self._card_image("/tmp/spectre_run.sh"))
        self.assertFalse(aa.same_command(a, b))

    def test_a_long_command_is_not_taken_for_the_background(self):
        # The command fills the region it is searched for, so the colour of
        # the region is the colour of the code block. The band must still be
        # the command and not the status line above the option rows.
        a = self._band_of(self._long_command_card("/tmp/mkdir.sh"))
        b = self._band_of(self._long_command_card("/tmp/spectre_run.sh"))
        self.assertFalse(aa.same_command(a, b))

    def test_a_card_without_a_status_line_still_yields_the_command(self):
        # Not every card renders a status line; the command is then the last
        # text line above the option rows.
        allow_y = 694 + 16
        img = Image.new("L", (780, 968), 75)
        drawer = ImageDraw.Draw(img)
        drawer.rectangle((35, allow_y - 80, 600, allow_y - 50), fill=45)
        drawer.text((40, allow_y - 74), "/tmp/mkdir.sh", fill=216)
        img.paste(Image.open(aa.DEFAULT_TEMPLATE).convert("L"), (58, 694))
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        img.save(path)
        self.addCleanup(os.remove, path)
        band = self._band_of(path)
        self.assertIsNotNone(band)
        self.assertTrue(aa.same_command(band, self._band_of(self._card_image("/tmp/mkdir.sh"))))


class GuardRegionTests(unittest.TestCase):
    """The post-click guard must look at the prompt, not at a fixed block.

    The permission prompt is rendered wherever the client puts it.  A region
    hard-coded at the left of the window compares unrelated pixels when the
    prompt renders further right: a false "unchanged" abort, or a false pass
    when other content moved instead.
    """

    def _region(self, prompt_x, prompt_y, size=(1600, 900)):
        template = aa.load_template(aa.DEFAULT_TEMPLATE)
        found = (prompt_x + aa.ALLOW_OFFSET[0], prompt_y + aa.ALLOW_OFFSET[1], 0.95)
        return aa.guard_region(found, template, size), template

    def test_the_region_covers_the_detected_prompt(self):
        region, template = self._region(1200, 700)
        left, top, width, height = region
        th, tw = template.shape[0], template.shape[1]
        self.assertLessEqual(left, 1200)
        self.assertGreaterEqual(left + width, 1200 + tw)
        self.assertLessEqual(top, 700)
        self.assertGreaterEqual(top + height, 700 + th)

    def test_the_region_follows_the_prompt_to_the_right(self):
        # The guard must not stay at the left edge of a wide window: that
        # would compare pixels the prompt never touched.
        region, _ = self._region(1200, 700)
        self.assertGreater(region[0], 700)

    def test_the_region_is_clamped_to_the_screenshot(self):
        region, _ = self._region(1595, 895, size=(1600, 900))
        left, top, width, height = region
        self.assertGreaterEqual(left, 0)
        self.assertGreaterEqual(top, 0)
        self.assertLessEqual(left + width, 1600)
        self.assertLessEqual(top + height, 900)
        self.assertGreater(width, 0)
        self.assertGreater(height, 0)

    def test_the_region_of_a_prompt_at_the_left_edge_is_usable(self):
        region, _ = self._region(0, 0, size=(1600, 900))
        left, top, width, height = region
        self.assertEqual(left, 0)
        self.assertEqual(top, 0)
        self.assertGreater(width, 0)
        self.assertGreater(height, 0)


class DisplayFlagTests(unittest.TestCase):
    """--display defaults to auto: vcli detects the DISPLAY itself."""

    def test_the_display_flag_is_omitted_when_it_is_empty(self):
        self.assertEqual(aa.display_args(""), [])
        self.assertEqual(aa.display_args(":1"), ["--display", ":1"])

    def test_the_capture_omits_an_empty_display(self):
        calls = []

        def fake_run(cmd):
            calls.append(cmd)
            return 0, json.dumps({"artifact": {"local_path": __file__}}), ""

        original = aa.run
        aa.run = fake_run
        self.addCleanup(setattr, aa, "run", original)

        aa.capture("vcli", "s", "0x1", "", "/tmp/work")
        self.assertNotIn("--display", calls[0])

    def test_the_capture_passes_an_explicit_display(self):
        calls = []

        def fake_run(cmd):
            calls.append(cmd)
            return 0, json.dumps({"artifact": {"local_path": __file__}}), ""

        original = aa.run
        aa.run = fake_run
        self.addCleanup(setattr, aa, "run", original)

        aa.capture("vcli", "s", "0x1", ":3", "/tmp/work")
        self.assertIn("--display", calls[0])
        self.assertEqual(calls[0][calls[0].index("--display") + 1], ":3")


class GuardSamplingTests(unittest.TestCase):
    """The post-click guard has to catch the change while it is there.

    The client re-renders the permission card in place: the card is consumed
    and the next request appears at the very same place a second later. A
    guard that waits several seconds before its first sample sees the card
    back exactly as it was and mistakes a working click for a miss — it
    aborted the loop while the agent was simply asking again.
    """

    def _args(self, interval=3):
        class A(object):
            pass
        a = A()
        a.vcli = "vcli"
        a.session = "s"
        a.window_id = "0x1"
        a.display = ":0"
        a.interval = interval
        return a

    def test_a_short_lived_change_is_still_caught(self):
        state = {"t0": None}

        def fake_capture(vcli, session, window_id, display, workdir):
            # The card is back exactly as it was two seconds after the click.
            if state["t0"] is None:
                state["t0"] = time.time()
            changed = (time.time() - state["t0"]) < 2.0
            img = Image.new("L", (40, 40), 200 if changed else 100)
            handle, path = tempfile.mkstemp(suffix=".png")
            os.close(handle)
            img.save(path)
            self.addCleanup(os.remove, path)
            return path

        original = aa.capture
        aa.capture = fake_capture
        self.addCleanup(setattr, aa, "capture", original)

        before = np.full((40, 40), 100, dtype=np.uint8)
        self.assertTrue(
            aa.wait_until_progress(self._args(), "/tmp", before, (0, 0, 40, 40), polls=5))

    def test_a_prompt_that_never_changes_is_still_a_miss(self):
        # The guard keeps its teeth: when nothing changes at all, the click
        # did not land and the caller has to stop.
        def fake_capture(vcli, session, window_id, display, workdir):
            img = Image.new("L", (40, 40), 100)
            handle, path = tempfile.mkstemp(suffix=".png")
            os.close(handle)
            img.save(path)
            self.addCleanup(os.remove, path)
            return path

        original = aa.capture
        aa.capture = fake_capture
        self.addCleanup(setattr, aa, "capture", original)

        before = np.full((40, 40), 100, dtype=np.uint8)
        self.assertFalse(
            aa.wait_until_progress(self._args(interval=0.2), "/tmp", before,
                                  (0, 0, 40, 40), polls=2))


class ClickFailureCountTests(unittest.TestCase):
    """A click that leaves the prompt unchanged is counted, not fatal.

    The client re-renders the permission card in place, so the change can be
    over before the first sample catches it. One such miss is no proof that
    the click went nowhere — only a run of them means the loop is clicking
    at a place that does nothing.
    """

    def _run_loop(self, misses, max_clicks=6):
        """Drive main() with a fixed pattern of misses; return (code, log)."""
        img = Image.new("L", (780, 968), 30)
        img.paste(Image.open(aa.DEFAULT_TEMPLATE).convert("L"), (58, 694))
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        img.save(path)
        self.addCleanup(os.remove, path)

        state = {"misses": list(misses)}

        def fake_capture(vcli, session, window_id, display, workdir):
            return path

        originals = {
            "capture": aa.capture,
            "click": aa.click,
            "wait_until_progress": aa.wait_until_progress,
            "try_send_guidance": aa.try_send_guidance,
        }
        aa.capture = fake_capture
        aa.click = lambda *a, **k: True

        def fake_progress(*a, **k):
            # Each entry of the pattern says what the guard reports for that
            # click: True = the prompt area stayed unchanged (a miss).
            if state["misses"]:
                return not state["misses"].pop(0)
            return True

        aa.wait_until_progress = fake_progress
        aa.try_send_guidance = lambda *a, **k: (True, None)
        self.addCleanup(lambda: [setattr(aa, k, v) for k, v in originals.items()])

        argv = ["auto_allow.py", "--vcli", "vcli", "--session", "s",
                "--window-id", "0x1", "--display", ":0", "--interval", "0",
                "--idle-polls", "0", "--timeout", "0",
                "--max-clicks", str(max_clicks)]
        old_argv = sys.argv
        sys.argv = argv
        buf = io.StringIO()
        try:
            with contextlib.redirect_stdout(buf):
                code = aa.main()
        finally:
            sys.argv = old_argv
        return code, buf.getvalue()

    def test_a_single_miss_does_not_abort_the_loop(self):
        code, log = self._run_loop([True])
        self.assertEqual(code, 0)
        self.assertIn("the click may have missed", log)
        self.assertNotIn("aborting", log)

    def test_three_misses_in_a_row_abort_the_loop(self):
        code, log = self._run_loop([True, True, True])
        self.assertEqual(code, 1)
        self.assertIn("aborting", log)

    def test_a_miss_after_a_success_does_not_abort_the_loop(self):
        # The counter is reset by a click that did land, so only consecutive
        # misses count.
        code, log = self._run_loop([True, False, True, False], max_clicks=4)
        self.assertEqual(code, 0)
        self.assertNotIn("aborting", log)


class GuideMessageTests(unittest.TestCase):
    def test_the_repeat_count_is_substituted(self):
        self.assertEqual(aa.format_guide_message("seen %d times", 3), "seen 3 times")

    def test_a_literal_percent_does_not_crash_the_loop(self):
        # %-formatting would raise on "100% of the time" and take the
        # unattended loop down with it.
        self.assertEqual(
            aa.format_guide_message("100% of the time, %d repeats", 3),
            "100% of the time, 3 repeats",
        )


class LoopGuidanceTests(unittest.TestCase):
    """The loop counts repeated commands and queues the guidance."""

    def _prompt_image(self, command_text=None):
        img = Image.new("L", (780, 968), 30)
        if command_text is not None:
            img.paste(Image.open(aa.DEFAULT_TEMPLATE).convert("L"), (58, 694))
            drawer = ImageDraw.Draw(img)
            drawer.text((35, 694 + 16 - 95), command_text, fill=220)
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        img.save(path)
        self.addCleanup(os.remove, path)
        return path

    def _run_loop(self, commands, guide_threshold=2, max_clicks=6):
        """Drive main() over a list of commands (None = no prompt pending)."""
        frames = [self._prompt_image(c) for c in commands]
        state = {"i": 0}
        guidance = []

        def fake_capture(vcli, session, window_id, display, workdir):
            path = frames[min(state["i"], len(frames) - 1)]
            state["i"] += 1
            return path

        originals = {
            "capture": aa.capture,
            "click": aa.click,
            "wait_until_progress": aa.wait_until_progress,
            "try_send_guidance": aa.try_send_guidance,
        }
        aa.capture = fake_capture
        aa.click = lambda *a, **k: True
        aa.wait_until_progress = lambda *a, **k: True

        def fake_guidance(args, message, workdir, ours_snapshot=None):
            guidance.append((message, ours_snapshot is not None))
            return True, None

        aa.try_send_guidance = fake_guidance
        self.addCleanup(lambda: [setattr(aa, k, v) for k, v in originals.items()])

        argv = ["auto_allow.py", "--vcli", "vcli", "--session", "s",
                "--window-id", "0x1", "--display", ":0", "--interval", "0",
                "--idle-polls", "1", "--timeout", "0",
                "--max-clicks", str(max_clicks),
                "--guide-threshold", str(guide_threshold)]
        old_argv = sys.argv
        sys.argv = argv
        try:
            aa.main()
        finally:
            sys.argv = old_argv
        return guidance

    def test_the_guidance_is_queued_after_the_threshold(self):
        guidance = self._run_loop(["/tmp/mkdir.sh", "/tmp/mkdir.sh",
                                   "/tmp/mkdir.sh", None])
        self.assertEqual(len(guidance), 1)
        self.assertIn("2 times", guidance[0][0])

    def test_a_different_command_resets_the_counter(self):
        guidance = self._run_loop(["/tmp/mkdir.sh", "/tmp/spectre_run.sh",
                                   "/tmp/mkdir.sh", None])
        self.assertEqual(guidance, [])

    def test_the_guidance_is_only_sent_when_no_prompt_is_pending(self):
        # The last frame still shows a prompt: the message stays queued.
        guidance = self._run_loop(["/tmp/x.sh", "/tmp/x.sh", "/tmp/x.sh"])
        self.assertEqual(guidance, [])


class GuidanceDeliveryTests(unittest.TestCase):
    """The guidance message must land in the composer and be submitted.

    While a permission prompt is pending the composer is that prompt's answer
    field, so the message is only typed when no prompt is pending — and the
    composer is disabled while the agent is busy, so delivery is verified with
    a screenshot instead of assumed.
    """

    W, H = 640, 604

    def _patch(self, accepts_type_at=None, placeholder=False, leftover=False,
               caret=False, single_char=False):
        """Mock the vcli boundary and the capture.

        `accepts_type_at` is the client row the fake composer accepts typing
        on; None means it never accepts (the agent is busy). `placeholder`
        makes the composer show the answer placeholder of a pending prompt.
        `leftover` makes the composer hold the text of a previous, unconfirmed
        attempt of the loop itself. `caret` makes the empty composer show the
        blinking caret and the descenders of the disclaimer above it.
        `single_char` makes the composer hold one narrow character the user
        typed.
        """
        state = {"typed": False, "leftover": leftover}
        self.state = state
        calls = []
        self.calls = calls

        def fake_run(cmd):
            calls.append(cmd)
            if "geometry" in cmd:
                return 0, json.dumps({"geometry": {"w": self.W, "h": self.H}}), ""
            op = cmd[cmd.index("--operation") + 1] if "--operation" in cmd else ""
            if op == "click-rel":
                y = int(cmd[cmd.index("--y") + 1])
                state["accepts"] = (accepts_type_at is not None and y == self.H - accepts_type_at)
            elif op == "type":
                if state.get("accepts"):
                    state["typed"] = True
            elif op == "key":
                text = cmd[cmd.index("--text") + 1]
                if text == "BackSpace":
                    # The loop clears the composer before typing.
                    state["typed"] = False
                    state["leftover"] = False
                elif text == "Return" and state["typed"]:
                    state["typed"] = False
            return 0, "{}", ""

        def fake_capture(vcli, session, window_id, display, workdir):
            img = Image.new("L", (self.W, self.H), 30)
            if caret:
                # An empty composer is not free of ink: the blinking caret is a
                # line of its own and the descenders of the disclaimer above
                # bleed into the crop. Measured on the real frames: 24 pixels.
                for y in range(self.H - 68, self.H - 55):
                    img.putpixel((37, y), 220)
                for x in (277, 326, 395):
                    img.putpixel((x, self.H - 96), 220)
                for x in range(322, 326):
                    img.putpixel((x, self.H - 95), 220)
            if placeholder:
                # The answer placeholder of a pending prompt: a long dim line
                # across the composer.
                for x in range(35, 300):
                    for y in range(self.H - 70, self.H - 62):
                        img.putpixel((x, y), 140)
            if single_char:
                # One narrow character the user typed: three columns of ink,
                # less than the ink of an empty composer with its caret.
                for x in range(38, 41):
                    for y in range(self.H - 66, self.H - 63):
                        img.putpixel((x, y), 220)
            if state.get("foreign"):
                # Text the user typed over the message of the loop.
                for x in range(40, 200):
                    for y in range(self.H - 70, self.H - 60):
                        img.putpixel((x, y), 220)
            elif state["typed"] or state["leftover"]:
                for x in range(20, 320):
                    for y in range(self.H - 84, self.H - 60):
                        img.putpixel((x, y), 220)
            handle, path = tempfile.mkstemp(suffix=".png")
            os.close(handle)
            img.save(path)
            self.addCleanup(os.remove, path)
            return path

        original_run, original_capture = aa.run, aa.capture
        aa.run, aa.capture = fake_run, fake_capture
        self.addCleanup(setattr, aa, "run", original_run)
        self.addCleanup(setattr, aa, "capture", original_capture)

    def _args(self):
        class A(object):
            pass
        a = A()
        a.vcli = "vcli"
        a.session = "s"
        a.window_id = "0x1"
        a.display = ":0"
        return a

    def _returns_sent(self):
        return [c for c in self.calls
                if "--operation" in c and c[c.index("--operation") + 1] == "key"
                and c[c.index("--text") + 1] == "Return"]

    def _types_sent(self):
        return [c for c in self.calls
                if "--operation" in c and c[c.index("--operation") + 1] == "type"]

    def _clears_sent(self):
        return [c for c in self.calls
                if "--operation" in c and c[c.index("--operation") + 1] == "key"
                and c[c.index("--text") + 1] == "BackSpace"]

    def _composer_crop(self, kind):
        """The composer crop as the mock renders it."""
        crop = np.full((56, 400), 30, dtype=np.uint8)
        if kind == "ours":
            for x in range(20, 320):
                for y in range(self.H - 84, self.H - 60):
                    crop[y - (self.H - 96), x] = 220
        elif kind == "foreign":
            for x in range(40, 200):
                for y in range(self.H - 70, self.H - 60):
                    crop[y - (self.H - 96), x] = 220
        return crop

    def test_message_is_sent_and_submitted(self):
        self._patch(accepts_type_at=68)
        delivered, snapshot = aa.try_send_guidance(self._args(), "diagnose first", "/tmp")
        self.assertTrue(delivered)
        self.assertIsNone(snapshot)
        self.assertTrue(self._returns_sent())

    def test_busy_composer_keeps_the_message_queued(self):
        # The composer never accepts the text (the agent is busy): nothing may
        # be submitted, and the caller keeps the message for the next poll.
        self._patch(accepts_type_at=None)
        delivered, snapshot = aa.try_send_guidance(self._args(), "diagnose first", "/tmp")
        self.assertFalse(delivered)
        self.assertIsNone(snapshot)
        self.assertEqual(self._returns_sent(), [])

    def test_a_second_click_row_is_tried_when_the_first_misses(self):
        # The input line moves with the composer layout, so the delivery falls
        # back to the next candidate row.
        self._patch(accepts_type_at=74)
        delivered, _ = aa.try_send_guidance(self._args(), "diagnose first", "/tmp")
        self.assertTrue(delivered)

    def test_a_pending_prompt_is_never_answered(self):
        # A prompt that this skill does not match (the plan approval) still
        # shows the answer placeholder in the composer: typing there would
        # answer the prompt instead of talking to the agent.
        self._patch(accepts_type_at=68, placeholder=True)
        delivered, snapshot = aa.try_send_guidance(self._args(), "diagnose first", "/tmp")
        self.assertFalse(delivered)
        self.assertIsNone(snapshot)
        self.assertEqual(self._types_sent(), [])
        self.assertEqual(self._returns_sent(), [])

    def test_text_the_user_typed_is_not_wiped(self):
        # A composer that already holds text is left alone.
        self._patch(accepts_type_at=68, placeholder=True)
        delivered, _ = aa.try_send_guidance(self._args(), "diagnose first", "/tmp")
        self.assertFalse(delivered)
        self.assertEqual(self._clears_sent(), [])

    def test_the_leftover_text_of_the_loop_is_retried(self):
        # A previous attempt left the message in the composer: the loop owns
        # that text, so it must clear it and try again instead of treating it
        # as somebody else's.
        self._patch(accepts_type_at=68, leftover=True)
        delivered, snapshot = aa.try_send_guidance(
            self._args(), "diagnose first", "/tmp",
            ours_snapshot=self._composer_crop("ours"))
        self.assertTrue(delivered)
        self.assertIsNone(snapshot)
        self.assertTrue(self._returns_sent())

    def test_text_the_user_typed_over_the_message_is_not_wiped(self):
        # The loop left its message in the composer, but the user has typed
        # over it: the composer no longer shows the text of the loop, and
        # clearing it would destroy the text of the user.
        self._patch(accepts_type_at=68)
        self.state["foreign"] = True
        delivered, snapshot = aa.try_send_guidance(
            self._args(), "diagnose first", "/tmp",
            ours_snapshot=self._composer_crop("ours"))
        self.assertFalse(delivered)
        self.assertIsNone(snapshot)
        self.assertEqual(self._clears_sent(), [])
        self.assertEqual(self._types_sent(), [])


    def test_an_empty_composer_with_a_caret_is_not_taken_for_text(self):
        # An empty composer is not free of ink: the blinking caret is a line of
        # its own and the descenders of the disclaimer above it bleed into the
        # crop. That ink must not stop the guidance — otherwise the message
        # would never be delivered in that layout.
        self._patch(accepts_type_at=68, caret=True)
        delivered, snapshot = aa.try_send_guidance(self._args(), "diagnose first", "/tmp")
        self.assertTrue(delivered)
        self.assertIsNone(snapshot)
        self.assertTrue(self._returns_sent())

    def test_a_single_character_the_user_typed_is_not_wiped(self):
        # One narrow character carries less ink than an empty composer with its
        # caret, so the ink is judged by its shape: a glyph is wider than the
        # caret, and the user's character must survive the loop.
        self._patch(accepts_type_at=68, single_char=True)
        delivered, snapshot = aa.try_send_guidance(self._args(), "diagnose first", "/tmp")
        self.assertFalse(delivered)
        self.assertIsNone(snapshot)
        self.assertEqual(self._clears_sent(), [])
        self.assertEqual(self._types_sent(), [])


class ComposerInkShapeTests(unittest.TestCase):
    """An empty composer is told apart from one that holds text by shape."""

    def _crop(self):
        return np.full((56, 400), 30, dtype=np.uint8)

    def test_a_caret_and_the_descenders_are_not_text(self):
        crop = self._crop()
        for y in range(28, 42):
            crop[y, 37] = 220
        for x in (277, 326, 395):
            crop[0, x] = 220
        for x in range(322, 326):
            crop[1, x] = 220
        self.assertFalse(aa.composer_holds_text(crop))

    def test_a_narrow_glyph_is_text(self):
        crop = self._crop()
        for x in range(38, 41):
            for y in range(30, 33):
                crop[y, x] = 220
        self.assertTrue(aa.composer_holds_text(crop))

    def test_a_line_of_text_is_text(self):
        crop = self._crop()
        crop[28:34, 40:200] = 220
        self.assertTrue(aa.composer_holds_text(crop))

    def test_an_empty_crop_holds_no_text(self):
        self.assertFalse(aa.composer_holds_text(self._crop()))

    def test_a_missing_crop_holds_no_text(self):
        self.assertFalse(aa.composer_holds_text(None))


if __name__ == "__main__":
    unittest.main()
