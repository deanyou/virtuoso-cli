"""Unit tests for the virastack-auto-allow skill scripts.

Pure/offline only — no Virtuoso session, no X server, no vcli binary:

- resolve_target picks the session, or refuses when the choice is ambiguous;
- find_prompt locates the planted template and returns the Allow label offset;
- a window without any prompt must not match.

Run:
    cd .claude/skills/virastack-auto-allow && python3 -m unittest discover tests
"""

import importlib.util
import json
import os
import sys
import tempfile
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
        return a

    def test_window_is_resolved_by_title_prefix(self):
        self._patch_run({
            "json": (0, json.dumps({"windows": [
                {"window_id": "0x1", "title": "virtuoso"},
                {"window_id": "0x2", "title": "ViraStack : Agent (0)"},
            ]}), ""),
        })
        self.assertEqual(aa.resolve_window("vcli", "s", "ViraStack"), "0x2")

    def test_no_matching_window_returns_none(self):
        self._patch_run({"json": (0, json.dumps({"windows": [
            {"window_id": "0x1", "title": "virtuoso"},
        ]}), "")})
        self.assertIsNone(aa.resolve_window("vcli", "s", "ViraStack"))

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
        aa.resolve_window = lambda vcli, session, prefix: "0xnew"
        self.addCleanup(setattr, aa, "resolve_window", original)

        args = self._args()
        self.assertTrue(aa.recover_target(args))
        self.assertEqual(args.session, "new-session")
        self.assertEqual(args.window_id, "0xnew")

    def test_recover_is_a_noop_when_the_target_is_unchanged(self):
        self._patch_run({
            "json": (0, json.dumps({"sessions": [{"id": "old-session"}]}), ""),
        })
        original = aa.resolve_window
        aa.resolve_window = lambda vcli, session, prefix: "0xdead"
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
        drawer.text((40, 694 + 16 - 95 + 4), command_text, fill=220)
        handle, path = tempfile.mkstemp(suffix=".png")
        os.close(handle)
        img.save(path)
        self.addCleanup(os.remove, path)
        return path

    def test_same_command_gives_the_same_signature(self):
        a = aa.command_signature(self._prompt_image("/tmp/x.sh"), (58 + aa.ALLOW_OFFSET[0], 694 + aa.ALLOW_OFFSET[1], 1.0))
        b = aa.command_signature(self._prompt_image("/tmp/x.sh"), (58 + aa.ALLOW_OFFSET[0], 694 + aa.ALLOW_OFFSET[1], 1.0))
        self.assertEqual(a, b)

    def test_different_command_gives_a_different_signature(self):
        a = aa.command_signature(self._prompt_image("/tmp/x.sh"), (58 + aa.ALLOW_OFFSET[0], 694 + aa.ALLOW_OFFSET[1], 1.0))
        b = aa.command_signature(self._prompt_image("/tmp/y.sh"), (58 + aa.ALLOW_OFFSET[0], 694 + aa.ALLOW_OFFSET[1], 1.0))
        self.assertNotEqual(a, b)


class GuidanceDeliveryTests(unittest.TestCase):
    """The guidance message must land in the composer and be submitted.

    While a permission prompt is pending the composer is that prompt's answer
    field, so the message is only typed when no prompt is pending — and the
    composer is disabled while the agent is busy, so delivery is verified with
    a screenshot instead of assumed.
    """

    W, H = 640, 604

    def _patch(self, accepts_type_at=None):
        """Mock the vcli boundary and the capture.

        `accepts_type_at` is the client row the fake composer accepts typing
        on; None means it never accepts (the agent is busy).
        """
        state = {"typed": False}
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
                if text == "Return" and state["typed"]:
                    state["typed"] = False
            return 0, "{}", ""

        def fake_capture(vcli, session, window_id, display, workdir):
            img = Image.new("L", (self.W, self.H), 30)
            if state["typed"]:
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

    def test_message_is_sent_and_submitted(self):
        self._patch(accepts_type_at=68)
        self.assertTrue(aa.try_send_guidance(self._args(), "diagnose first", "/tmp"))
        self.assertTrue(self._returns_sent())

    def test_busy_composer_keeps_the_message_queued(self):
        # The composer never accepts the text (the agent is busy): nothing may
        # be submitted, and the caller keeps the message for the next poll.
        self._patch(accepts_type_at=None)
        self.assertFalse(aa.try_send_guidance(self._args(), "diagnose first", "/tmp"))
        self.assertEqual(self._returns_sent(), [])

    def test_a_second_click_row_is_tried_when_the_first_misses(self):
        # The input line moves with the composer layout, so the delivery falls
        # back to the next candidate row.
        self._patch(accepts_type_at=74)
        self.assertTrue(aa.try_send_guidance(self._args(), "diagnose first", "/tmp"))


if __name__ == "__main__":
    unittest.main()
