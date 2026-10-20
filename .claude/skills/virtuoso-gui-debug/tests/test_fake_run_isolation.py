"""Fake (offline) runs must not write into the skill package's tracked DB.

Regression: a fake GUI run wrote the Experience writeback into
`.claude/skills/virtuoso-gui-debug/data/skill_db.sqlite3`, which is tracked
in git — every offline replay dirtied the working tree.
"""
import hashlib
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, '.')

from vgui_runner.model import Scenario
from vgui_runner.engine import Runner, FakeExecutor

SKILL_DB = Path(__file__).resolve().parent.parent / "data" / "skill_db.sqlite3"

VALID = {
    "version": "1.0", "task_id": "iso-1", "session_id": "s1", "pid": 12345,
    "display": ":0", "cellview": {"lib": "L", "cell": "C", "view": "layout"},
    "steps": [{
        "id": "step1", "operation": "SCREENSHOT", "arguments": {"path": "/tmp/x.png"},
        "verifier": {"predicate": "window_exists", "expected": True},
        "timeout_seconds": 30, "max_retries": 0,
    }],
}


def _digest(path: Path) -> str:
    if not path.exists():
        return "<missing>"
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_fake_run_does_not_touch_the_tracked_skill_db():
    before = _digest(SKILL_DB)

    tmpdir = tempfile.mkdtemp()
    outdir = Path(tmpdir) / "run"
    Runner(FakeExecutor({})).run(Scenario.from_dict(VALID), outdir)

    after = _digest(SKILL_DB)
    assert before == after, (
        f"fake run modified the tracked skill DB: {before} -> {after}"
    )

    # The experience must still be captured — just not in the repo.
    assert (outdir / "experience.db").exists(), (
        "fake run should write its experience DB into the run output directory"
    )
