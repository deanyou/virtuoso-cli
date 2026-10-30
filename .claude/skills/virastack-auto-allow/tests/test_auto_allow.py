"""Unit tests for the virastack-auto-allow skill scripts.

Pure/offline only — no Virtuoso session, no X server, no vcli binary:

- resolve_target picks the session, or refuses when the choice is ambiguous;
- find_prompt locates the planted template and returns the Allow label offset;
- a window without any prompt must not match.

Run:
    cd .claude/skills/virastack-auto-allow && python3 -m unittest discover tests
"""

import importlib.util
import os
import sys
import tempfile
import unittest

import numpy as np
from PIL import Image

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


if __name__ == "__main__":
    unittest.main()
