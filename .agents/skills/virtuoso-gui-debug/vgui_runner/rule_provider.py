"""vgui_runner.rule_provider - RuleDecisionProvider。

P0：把现有 route() / decide_recovery() 包装为 DecisionProvider 契约，
    行为与直接调用完全一致（可注入函数以便等价性测试）。
P0.5：当经验校准可用时，用 EMPIRICAL 概率替换硬编码 HEURISTIC confidence；
     决策动作本身仍由确定性规则产生 —— 经验证据只作为置信度来源，
     不污染输入事实、不覆盖安全规则 / retry budget / Verifier 终态。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Dict, List, Optional

from .decision_contract import (
    ConfidenceType,
    DecisionCapabilities,
    DecisionQuestion,
    GuiDecision,
    GuiDecisionState,
    ProviderHealth,
    ProviderKind,
    RecoveryChoice,
)
from .empirical_calibration import EmpiricalCalibration
from .model import Operation
from .recovery import (
    ErrorCategory,
    RecoveryAction,
    RecoveryPolicy,
    RecoveryRequest,
    decide_recovery,
)
from .router import (
    ActionRequest,
    CapabilitySnapshot,
    Channel,
    RiskClass,
    RoutePolicy,
    route,
)

# RecoveryAction → 契约动作空间映射。ROLLBACK 先回滚再重试，仍属 retry 族。
_RECOVERY_MAP: Dict[RecoveryAction, RecoveryChoice] = {
    RecoveryAction.RETRY: RecoveryChoice.RETRY,
    RecoveryAction.ROLLBACK: RecoveryChoice.RETRY,
    RecoveryAction.FALLBACK: RecoveryChoice.FALLBACK,
    RecoveryAction.ABORT: RecoveryChoice.ABORT,
    RecoveryAction.MANUAL: RecoveryChoice.HUMAN,
}


@dataclass
class RuleDecisionProvider:
    """确定性规则 Provider：现有纯函数之上的薄壳。

    route_fn / decide_recovery_fn 可注入（测试用），默认使用仓库当前实现。
    calibration 为 None 时按 P0 语义工作（HEURISTIC / RULE 置信度）。
    """

    route_fn: Callable[..., Any] = route
    decide_recovery_fn: Callable[..., Any] = decide_recovery
    calibration: Optional[EmpiricalCalibration] = None
    default_caps: Optional[CapabilitySnapshot] = None
    default_route_policy: Optional[RoutePolicy] = None
    default_recovery_policy: Optional[RecoveryPolicy] = None

    # ── SPI ──────────────────────────────────────────────────────────────

    def capabilities(self) -> DecisionCapabilities:
        return DecisionCapabilities(
            choice=True,
            boolean_probability=True,
            calibrated_probability=self.calibration is not None,
            batch_decision=True,
            local_execution=True,
            max_context=0,
            expected_latency_class="instant",
        )

    def health(self) -> ProviderHealth:
        return ProviderHealth(healthy=True, detail="deterministic rule provider")

    def decide(
        self,
        state: GuiDecisionState,
        questions: List[DecisionQuestion],
    ) -> GuiDecision:
        """按问题类型依次回答，合并为一份 GuiDecision。

        多问题场景下，confidence / confidence_type 反映首个给出概率答案的
        primary 问题（recovery_route / action_success）；escalation 是规则旗标。
        """
        result = GuiDecision(provider=ProviderKind.RULE, model_version="rule")
        recovery_decision = None
        for q in questions:
            if q.kind == "route_channel":
                self._answer_route(state, q, result)
            elif q.kind == "recovery_route":
                recovery_decision = self._answer_recovery(state, q, result)
            elif q.kind == "escalation":
                self._answer_escalation(state, q, result, recovery_decision)
            elif q.kind == "action_success":
                self._answer_action_success(state, q, result)
        if self.calibration is not None:
            result.calibration_version = self.calibration.version
        return result

    # ── 各问题实现 ───────────────────────────────────────────────────────

    def _answer_route(self, state: GuiDecisionState, q: DecisionQuestion, result: GuiDecision) -> None:
        p = q.payload
        req = ActionRequest(Operation(p["operation"]), p["step_id"], dict(p.get("arguments") or {}))
        dec = self.route_fn(req, self._as_caps(p.get("caps")), self._as_route_policy(p.get("policy")))
        result.extra["route_channel"] = dec.channel.value
        result.extra["route_rejected"] = dec.rejected
        result.reason_code = dec.reason
        if dec.rejected:
            return
        est = self._estimate(state, p.get("attempt"))
        if est is not None:
            self._apply_estimate(result, est)
        else:
            result.confidence = dec.confidence
            result.confidence_type = ConfidenceType.HEURISTIC

    def _answer_recovery(self, state: GuiDecisionState, q: DecisionQuestion, result: GuiDecision):
        p = q.payload
        req = RecoveryRequest(
            step_id=p["step_id"],
            attempt=p.get("attempt", state.retry_count),
            risk_class=RiskClass(p["risk_class"]),
            error_category=ErrorCategory(p["error_category"]),
            error_message=p.get("error_message", ""),
            result_uncertain=bool(p.get("result_uncertain", False)),
            elapsed_ms=int(p.get("elapsed_ms", 0)),
            has_rollback=bool(p.get("has_rollback", False)),
            current_channel=Channel(p.get("current_channel", "skill")),
        )
        dec = self.decide_recovery_fn(req, self._as_recovery_policy(p.get("policy")))
        result.recovery_route = _RECOVERY_MAP[dec.action]
        result.reason_code = dec.reason_code
        result.extra["recovery_verification_required"] = dec.verification_required
        result.extra["recovery_next_channel"] = dec.next_channel.value if dec.next_channel else None
        est = self._estimate(state, req.attempt, failure=p.get("failure_type"))
        if est is not None:
            self._apply_estimate(result, est)
        else:
            result.confidence = None
            result.confidence_type = ConfidenceType.RULE
        return dec

    def _answer_escalation(self, state: GuiDecisionState, q: DecisionQuestion, result: GuiDecision,
                           recovery_decision: Any) -> None:
        p = q.payload
        action = p.get("recovery_action")
        if action is None and recovery_decision is not None:
            action = _RECOVERY_MAP[recovery_decision.action]
        if action is None:
            # 无 recovery 依据时按 payload 直接算一次。
            rec = self._answer_recovery(state, q, result)
            action = _RECOVERY_MAP[rec.action]
        escalate = action in (RecoveryChoice.HUMAN, RecoveryChoice.ABORT)
        result.need_escalation = 1.0 if escalate else 0.0
        result.extra["escalation_basis"] = action.value if hasattr(action, "value") else str(action)

    def _answer_action_success(self, state: GuiDecisionState, q: DecisionQuestion, result: GuiDecision) -> None:
        p = q.payload
        est = self._estimate(state, p.get("attempt"), failure=p.get("failure_type"))
        if est is None:
            result.action_succeeded = None
            result.confidence_type = ConfidenceType.UNKNOWN
            return
        result.action_succeeded = est.probability
        self._apply_estimate(result, est)

    # ── 经验校准 ─────────────────────────────────────────────────────────

    def _estimate(self, state: GuiDecisionState, attempt: Optional[int], failure: Optional[str] = None):
        """查经验校准；不可用时返回 None（调用方保持规则置信度语义）。"""
        if self.calibration is None:
            return None
        try:
            return self.calibration.estimate(
                state.route_channel,
                failure,
                attempt if attempt is not None else state.retry_count,
            )
        except Exception:
            return None

    def _apply_estimate(self, result: GuiDecision, est) -> None:
        result.confidence = round(est.probability, 4)
        result.confidence_type = ConfidenceType.EMPIRICAL
        result.support = est
        result.calibration_version = est.calibration_version

    # ── 上下文装配（兼容 dict 或现成对象） ───────────────────────────────

    def _as_caps(self, value: Any) -> CapabilitySnapshot:
        if value is None:
            return self.default_caps or CapabilitySnapshot()
        if isinstance(value, CapabilitySnapshot):
            return value
        return CapabilitySnapshot(**value)

    def _as_route_policy(self, value: Any) -> RoutePolicy:
        if value is None:
            return self.default_route_policy or RoutePolicy()
        if isinstance(value, RoutePolicy):
            return value
        return RoutePolicy(**value)

    def _as_recovery_policy(self, value: Any) -> RecoveryPolicy:
        if value is None:
            return self.default_recovery_policy or RecoveryPolicy()
        if isinstance(value, RecoveryPolicy):
            return value
        return RecoveryPolicy(**value)
