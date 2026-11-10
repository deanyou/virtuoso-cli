"""Tests for vgui_runner.decision_contract - 冻结契约 (P0)."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from vgui_runner.decision_contract import (
    ConfidenceType,
    DecisionProvider,
    DecisionQuestion,
    GuiDecision,
    GuiDecisionState,
    ProviderKind,
    QUESTION_KINDS,
    RecoveryChoice,
)


class TestConfidenceSemantics(unittest.TestCase):
    def test_confidence_types_frozen(self):
        self.assertEqual(
            {c.value for c in ConfidenceType},
            {"rule", "empirical", "calibrated", "heuristic", "unknown"},
        )

    def test_probability_bearing_types(self):
        # 语义冻结：只有 EMPIRICAL / CALIBRATED 能解释为概率
        self.assertIn(ConfidenceType.EMPIRICAL, ConfidenceType)
        self.assertIn(ConfidenceType.CALIBRATED, ConfidenceType)


class TestRecoveryChoice(unittest.TestCase):
    def test_frozen_action_space(self):
        self.assertEqual(
            {c.value for c in RecoveryChoice},
            {"retry", "wait", "reacquire", "reconnect", "fallback", "abort", "human"},
        )


class TestQuestionValidation(unittest.TestCase):
    def test_unknown_kind_rejected(self):
        with self.assertRaises(ValueError):
            DecisionQuestion(kind="make_coffee")

    def test_known_kinds_accept(self):
        for kind in ("route_channel", "recovery_route", "escalation", "action_success"):
            self.assertIn(kind, QUESTION_KINDS)
            DecisionQuestion(kind=kind)


class TestGuiDecisionBoundaries(unittest.TestCase):
    def test_safe_to_continue_is_policy_result(self):
        # 冻结边界：safe_to_continue 是 policy_result，Judge 不直接输出（默认 None）
        d = GuiDecision(provider=ProviderKind.RULE)
        self.assertIsNone(d.safe_to_continue)

    def test_default_confidence_unknown(self):
        d = GuiDecision(provider=ProviderKind.RULE)
        self.assertEqual(d.confidence_type, ConfidenceType.UNKNOWN)
        self.assertIsNone(d.confidence)


class TestProviderProtocol(unittest.TestCase):
    def test_protocol_runtime_checkable(self):
        class Dummy:
            def capabilities(self):
                return None

            def health(self):
                return None

            def decide(self, state, questions):
                return None

        self.assertIsInstance(Dummy(), DecisionProvider)

    def test_state_contract_defaults(self):
        s = GuiDecisionState(run_id="r1")
        self.assertEqual(s.tool, "vcli-gui")
        self.assertEqual(s.retry_count, 0)
        self.assertIsNone(s.route_channel)


if __name__ == "__main__":
    unittest.main()
