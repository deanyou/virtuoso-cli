"""Tests for vgui_runner.empirical_calibration - P0.5 经验校准。"""
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from vgui_runner.empirical_calibration import (
    EmpiricalCalibration,
    empirical_to_dict,
    load_calibration,
    wilson_interval,
)

SCHEMA = """
CREATE TABLE experience_events (
    event_id TEXT PRIMARY KEY, run_id TEXT, task_id TEXT, step_id TEXT,
    attempt INTEGER, event_type TEXT, timestamp TEXT, state TEXT, outcome TEXT,
    channel TEXT, risk_class TEXT, failure_type TEXT, action TEXT,
    duration_ms INTEGER, value_source TEXT, evidence_refs TEXT, details TEXT
);
"""


def make_db(path, events):
    """events: list of (channel, failure_type, attempt, outcome)."""
    conn = sqlite3.connect(str(path))
    conn.execute(SCHEMA)
    for i, (channel, failure, attempt, outcome) in enumerate(events):
        conn.execute(
            "INSERT INTO experience_events "
            "(event_id, attempt, outcome, channel, failure_type, value_source) "
            "VALUES (?, ?, ?, ?, ?, 'VERIFIER_CONFIRMED')",
            (f"e{i}", attempt, outcome, channel, failure),
        )
    conn.commit()
    conn.close()


class TestBetaSmoothing(unittest.TestCase):
    def test_one_pass_not_one(self):
        # 冻结方法学：1 PASS / 0 FAIL → 2/3 ≈ 0.667，而不是 1.00
        with tempfile.TemporaryDirectory() as td:
            db = Path(td) / "db.sqlite3"
            make_db(db, [("skill", None, 0, "PASSED")])
            cal = load_calibration(db, n_min=1)
            est = cal.estimate("skill", None, 0)
            self.assertAlmostEqual(est.probability, 2.0 / 3.0, places=4)
            self.assertEqual(est.grouping_level, "exact")

    def test_laplace_fifty_fifty(self):
        with tempfile.TemporaryDirectory() as td:
            db = Path(td) / "db.sqlite3"
            make_db(db, [("skill", None, 0, "PASSED"), ("skill", None, 0, "FAILED")])
            cal = load_calibration(db, n_min=1)
            self.assertAlmostEqual(cal.estimate("skill", None, 0).probability, 0.5, places=6)


class TestBucketsAndBackoff(unittest.TestCase):
    def test_exact_bucket_counts(self):
        # 22 PASS / 16 FAIL → (22+1)/(38+2) = 0.575
        with tempfile.TemporaryDirectory() as td:
            db = Path(td) / "db.sqlite3"
            make_db(db, [("skill", None, 0, "PASSED")] * 22 + [("skill", None, 0, "FAILED")] * 16)
            cal = load_calibration(db, n_min=1)
            est = cal.estimate("skill", None, 0)
            self.assertEqual(est.sample_count, 38)
            self.assertAlmostEqual(est.probability, 23.0 / 40.0, places=6)

    def test_hierarchical_backoff_to_channel_failure(self):
        # exact 只有 1 条（< n_min=5）→ 回退到 channel_failure（5 条）
        with tempfile.TemporaryDirectory() as td:
            db = Path(td) / "db.sqlite3"
            events = [("skill", "window_gone", 0, "PASSED")]
            events += [("skill", "window_gone", i, "FAILED") for i in range(1, 5)]
            make_db(db, events)
            cal = load_calibration(db, n_min=5)
            est = cal.estimate("skill", "window_gone", 0)
            self.assertEqual(est.grouping_level, "channel_failure")
            self.assertEqual(est.sample_count, 5)

    def test_backoff_to_global(self):
        with tempfile.TemporaryDirectory() as td:
            db = Path(td) / "db.sqlite3"
            events = [("other", "x", 0, "PASSED")] * 6
            make_db(db, events)
            cal = load_calibration(db, n_min=5)
            est = cal.estimate("skill", None, 0)  # skill 无分组样本 → global
            self.assertEqual(est.grouping_level, "global")
            self.assertEqual(est.sample_count, 6)

    def test_n_min_gate_returns_none(self):
        with tempfile.TemporaryDirectory() as td:
            db = Path(td) / "db.sqlite3"
            make_db(db, [("skill", None, 0, "PASSED")])
            cal = load_calibration(db, n_min=5)
            self.assertIsNone(cal.estimate("skill", None, 0))

    def test_wilson_bounds(self):
        lo, hi = wilson_interval(0, 10)
        self.assertEqual(lo, 0.0)
        lo, hi = wilson_interval(10, 10)
        self.assertEqual(hi, 1.0)
        lo, hi = wilson_interval(5, 10)
        self.assertGreater(hi, lo)
        self.assertGreaterEqual(lo, 0.0)
        self.assertLessEqual(hi, 1.0)


class TestLoadDegradation(unittest.TestCase):
    def test_missing_db_returns_none(self):
        self.assertIsNone(load_calibration(Path("/nonexistent/dir/db.sqlite3")))

    def test_empty_db_no_estimate(self):
        with tempfile.TemporaryDirectory() as td:
            db = Path(td) / "db.sqlite3"
            make_db(db, [])
            cal = load_calibration(db)
            self.assertIsNotNone(cal)
            self.assertIsNone(cal.estimate("skill", None, 0))

    def test_deterministic(self):
        with tempfile.TemporaryDirectory() as td:
            db = Path(td) / "db.sqlite3"
            make_db(db, [("skill", None, 0, "PASSED")] * 9 + [("skill", None, 0, "FAILED")] * 1)
            a = load_calibration(db, n_min=5).estimate("skill", None, 0)
            b = load_calibration(db, n_min=5).estimate("skill", None, 0)
            self.assertEqual(a.probability, b.probability)
            self.assertEqual(a.sample_count, b.sample_count)

    def test_empirical_to_dict_json_safe(self):
        cal = EmpiricalCalibration(
            exact={("skill", "", 0): (1, 0)},
            channel_failure={("skill", ""): (1, 0)},
            channel={"skill": (1, 0)},
            global_stats=(1, 0),
            n_min=1,
            version="v1",
        )
        est = cal.estimate("skill", None, 0)
        self.assertIsNotNone(est)
        d = empirical_to_dict(est)
        for key in ("probability", "sample_count", "successes", "failures",
                    "ci_low", "ci_high", "grouping_level", "calibration_version"):
            self.assertIn(key, d)


if __name__ == "__main__":
    unittest.main()
