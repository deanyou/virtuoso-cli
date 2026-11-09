"""Regression coverage for the skill mirror's policy and path adaptation."""
import importlib.util
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts' / 'sync_skills.py'
SPEC = importlib.util.spec_from_file_location('sync_skills', SCRIPT)
import sys
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class SyncContractTests(unittest.TestCase):
    def test_repo_skill_path_is_resolvable_and_not_codex(self):
        target, _ = MODULE.transform_text('sim-plot/SKILL.md',
            'python3 .claude/skills/sim-plot/scripts/plot_sim.py')
        self.assertEqual(target, 'python3 .agents/skills/sim-plot/scripts/plot_sim.py')
        self.assertTrue((SCRIPT.parents[1] / target.split(' ', 1)[1]).is_file())

    def test_unclassified_skill_is_pending_and_never_copied(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / '.claude/skills/new-skill/SKILL.md'
            source.parent.mkdir(parents=True)
            source.write_text('new skill')
            (root / '.agents/skills').mkdir(parents=True)
            # Exclude existing link rule to isolate this fixture.
            saved = MODULE.SKILL_RULES
            try:
                MODULE.SKILL_RULES = {}
                result = MODULE.build_plan(root)
                self.assertEqual({p[0] for p in result.pending}, {'new-skill'})
                self.assertEqual(MODULE.main(['--root', tmp, '--check', '--strict-pending']), 2)
                MODULE.apply_plan(root, result, prune=True)
                self.assertFalse((root / '.agents/skills/new-skill').exists())
            finally:
                MODULE.SKILL_RULES = saved


if __name__ == '__main__':
    unittest.main()
