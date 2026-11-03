"""Tests for vgui_runner.rule_provider - P0 包装等价性 + P0.5 校准接入。"""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from vgui_runner.decision_contract import (
    ConfidenceType,
    DecisionQuestion,
    GuiDecisionState,
    ProviderKind,
    RecoveryChoice,
)
from vgui_runner.empirical_calibration import EmpiricalCalibration
from vgui_runner.model import Operation
from vgui_runner.recovery import RecoveryAction
from vgui_runner.router import (
    ActionRequest,
    CapabilitySnapshot,
    RoutePolicy,
    route,
)
from vgui_runner.rule_provider import RuleDecisionProvider


def full_caps():
    return CapabilitySnapshot(
        vcli_available=True, session_alive=True, pid_valid=True,
        display_available=True, window_identity_known=True,
        remote_x11_allowed=True, local_x11_available=True,
        vision_enabled=False, ssh_budget_remaining=4, daemon_healthy=True,
    )


def state(channel="skill"):
    return GuiDecisionState(run_id="r", route_channel=channel, retry_count=0)


class TestCapabilities(unittest.TestCase):
    def test_rule_capabilities(self):
        caps = RuleDecisionProvider().capabilities()
        self.assertTrue(caps.choice)
        self.assertTrue(caps.local_execution)
        self.assertFalse(caps.calibrated_probability)

    def test_calibrated_flag_with_calibration(self):
        cal = EmpiricalCalibration(n_min=1)
        self.assertTrue(RuleDecisionProvider(calibration=cal).capabilities().calibrated_probability)

    def test_health_healthy(self):
        self.assertTrue(RuleDecisionProvider().health().healthy)


class TestRouteEquivalence(unittest.TestCase):
    def test_provider_matches_direct_route(self):
        p = RuleDecisionProvider()
        direct = route(ActionRequest(Operation.SCREENSHOT, "s1"), full_caps(), RoutePolicy())
        q = DecisionQuestion(kind="route_channel", payload={
            "operation": "SCREENSHOT", "step_id": "s1",
            "caps": full_caps(), "policy": RoutePolicy(),
        })
        d = p.decide(state(), [q])
        self.assertEqual(d.extra["route_channel"], direct.channel.value)
        self.assertEqual(d.reason_code, direct.reason)
        # P0 无校准时：置信度保持规则硬编码值，标记 HEURISTIC（待校准）
        self.assertEqual(d.confidence, direct.confidence)
        self.assertEqual(d.confidence_type, ConfidenceType.HEURISTIC)

    def test_rejected_route(self):
        p = RuleDecisionProvider()
        q = DecisionQuestion(kind="route_channel", payload={
            "operation": "VCLI_CALL", "step_id": "s2",
            "arguments": {"function": "x", "args": []},
            "caps": full_caps(), "policy": RoutePolicy(allow_vcli_call=False),
        })
        d = p.decide(state(), [q])
        self.assertTrue(d.extra["route_rejected"])
        self.assertEqual(d.extra["route_channel"], "rejected")

    def test_route_with_empirical_confidence(self):
        cal = EmpiricalCalibration(
            exact={("skill", "", 0): (22, 16)},
            channel_failure={("skill", ""): (22, 16)},
            channel={"skill": (22, 16)},
            global_stats=(22, 16),
            n_min=5, version="v1",
        )
        p = RuleDecisionProvider(calibration=cal)
        q = DecisionQuestion(kind="route_channel", payload={
            "operation": "SCREENSHOT", "step_id": "s1", "caps": full_caps(),
        })
        d = p.decide(state(), [q])
        # P0.5：经验概率替换硬编码 0.95，标记 EMPIRICAL
        self.assertEqual(d.confidence_type, ConfidenceType.EMPIRICAL)
        self.assertAlmostEqual(d.confidence, 23.0 / 40.0, places=4)
        self.assertEqual(d.calibration_version, "v1")
        self.assertIsNotNone(d.support)


class TestRecoveryMapping(unittest.TestCase):
    def _decide_recovery(self, action):
        def fake(req, policy):
            dec = type("_D", (), {})()
            dec.action = action
            dec.reason_code = "fake"
            dec.verification_required = False
            dec.next_channel = None
            return dec
        p = RuleDecisionProvider(decide_recovery_fn=fake)
        q = DecisionQuestion(kind="recovery_route", payload={
            "step_id": "s1", "risk_class": "read_only", "error_category": "timeout",
            "current_channel": "skill",
        })
        return p.decide(state(), [q])

    def test_action_mapping(self):
        cases = {
            RecoveryAction.RETRY: RecoveryChoice.RETRY,
            RecoveryAction.ROLLBACK: RecoveryChoice.RETRY,
            RecoveryAction.FALLBACK: RecoveryChoice.FALLBACK,
            RecoveryAction.ABORT: RecoveryChoice.ABORT,
            RecoveryAction.MANUAL: RecoveryChoice.HUMAN,
        }
        for action, expected in cases.items():
            with self.subTest(action=action):
                self.assertEqual(self._decide_recovery(action).recovery_route, expected)

    def test_recovery_no_calibration_is_rule(self):
        d = self._decide_recovery(RecoveryAction.RETRY)
        self.assertEqual(d.confidence_type, ConfidenceType.RULE)
        self.assertIsNone(d.confidence)


class TestEscalation(unittest.TestCase):
    def _escalation(self, action):
        def fake(req, policy):
            dec = type("_D", (), {})()
            dec.action = action
            dec.reason_code = "fake"
            dec.verification_required = False
            dec.next_channel = None
            return dec
        p = RuleDecisionProvider(decide_recovery_fn=fake)
        q = DecisionQuestion(kind="escalation", payload={
            "step_id": "s1", "risk_class": "destructive", "error_category": "timeout",
            "current_channel": "skill",
        })
        return p.decide(state(), [q])

    def test_manual_escalates(self):
        self.assertEqual(self._escalation(RecoveryAction.MANUAL).need_escalation, 1.0)

    def test_retry_no_escalation(self):
        self.assertEqual(self._escalation(RecoveryAction.RETRY).need_escalation, 0.0)

    def test_escalation_basis_recorded(self):
        d = self._escalation(RecoveryAction.MANUAL)
        self.assertEqual(d.extra["escalation_basis"], "human")


class TestEmpiricalIntegration(unittest.TestCase):
    def _calibration(self):
        return EmpiricalCalibration(
            exact={("skill", "", 0): (22, 16)},
            channel_failure={("skill", ""): (22, 16)},
            channel={"skill": (22, 16)},
            global_stats=(22, 16),
            n_min=5, version="test-v1",
        )

    def test_action_success_uses_empirical(self):
        p = RuleDecisionProvider(calibration=self._calibration())
        q = DecisionQuestion(kind="action_success", payload={"failure_type": None})
        d = p.decide(state(), [q])
        self.assertAlmostEqual(d.action_succeeded, 23.0 / 40.0, places=4)
        self.assertEqual(d.confidence_type, ConfidenceType.EMPIRICAL)
        self.assertEqual(d.calibration_version, "test-v1")
        self.assertIsNotNone(d.support)

    def test_action_success_without_calibration_unknown(self):
        p = RuleDecisionProvider()
        q = DecisionQuestion(kind="action_success", payload={"failure_type": None})
        d = p.decide(state(), [q])
        self.assertIsNone(d.action_succeeded)
        self.assertEqual(d.confidence_type, ConfidenceType.UNKNOWN)

    def test_recovery_confidence_empirical(self):
        p = RuleDecisionProvider(calibration=self._calibration())
        q = DecisionQuestion(kind="recovery_route", payload={
            "step_id": "s1", "risk_class": "read_only", "error_category": "timeout",
            "current_channel": "skill", "failure_type": None, "attempt": 0,
        })
        d = p.decide(state(), [q])
        self.assertEqual(d.confidence_type, ConfidenceType.EMPIRICAL)
        self.assertIsNotNone(d.recovery_route)
        self.assertEqual(d.provider, ProviderKind.RULE)

    def test_unsupported_question_ignored(self):
        p = RuleDecisionProvider()
        q = DecisionQuestion(kind="route_channel", payload={
            "operation": "SCREENSHOT", "step_id": "s1", "caps": full_caps(),
        })
        d = p.decide(state(), [q])
        self.assertEqual(d.extra["route_channel"], "skill")


if __name__ == "__main__":
    unittest.main()
