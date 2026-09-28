"""vgui_runner.decision_contract - VCLI Decision Architecture 冻结契约 (P0).

VCLI depends on the Decision Contract, not on Jev / Laya / LLM.
本模块只定义输入/输出契约与 Provider SPI，不包含任何决策逻辑。

冻结边界（见 vcli_decision_architecture_frozen）：
- Rule/Policy 独占 risk_class 与安全放行：safe_to_continue 是 policy_result，
  由 Policy 层综合生成，Judge / Provider 不直接输出。
- Verifier 独占 PASS / FAIL / UNKNOWN 最终事实。
- Provider 只产生结构化判断；不执行 GUI action、不修改 Skill、不写终态 PASS。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional, Protocol, Tuple, runtime_checkable


class ConfidenceType(str, Enum):
    """置信度来源类型。只有 CALIBRATED / EMPIRICAL 才能解释为概率。

    - RULE:       确定性规则，不解释为概率，可信边界由规则版本与前置条件决定
    - EMPIRICAL:  历史 Verifier 结果统计出的经验概率（P0.5）
    - CALIBRATED: 独立校准过的 learned Judge 概率（未来）
    - HEURISTIC:  启发式/自报值，不自动解释为概率（待校准）
    - UNKNOWN:    缺失/异常，升级 slow path / human
    """

    RULE = "rule"
    EMPIRICAL = "empirical"
    CALIBRATED = "calibrated"
    HEURISTIC = "heuristic"
    UNKNOWN = "unknown"


class ProviderKind(str, Enum):
    RULE = "rule"
    LOCAL_JUDGE = "local_judge"
    JEV = "jev"
    LLM = "llm"


class RecoveryChoice(str, Enum):
    """recovery_route 统一动作空间（learned Judge 的完整候选）。

    RuleProvider 当前只产生其中规则可表达的子集（retry/fallback/abort/human）。
    """

    RETRY = "retry"
    WAIT = "wait"
    REACQUIRE = "reacquire"
    RECONNECT = "reconnect"
    FALLBACK = "fallback"
    ABORT = "abort"
    HUMAN = "human"


# 本契约支持的决策问题类型。
QUESTION_KINDS = frozenset({
    "route_channel", "recovery_route", "escalation", "action_success",
})


@dataclass(frozen=True)
class GuiDecisionState:
    """统一输入契约：决策所需的事实，不含操作性上下文。

    route_channel / x11_status / ssh_status / popup_state 是环境事实；
    操作性上下文（capability、policy 等）由 DecisionQuestion.payload 携带。
    """

    run_id: str
    tool: str = "vcli-gui"
    window_identity: Optional[str] = None
    last_action: Optional[str] = None
    last_result: Optional[str] = None
    failure_type: Optional[str] = None
    retry_count: int = 0
    route_channel: Optional[str] = None
    x11_status: Optional[str] = None
    ssh_status: Optional[str] = None
    popup_state: Optional[str] = None
    recent_events: List[Dict[str, Any]] = field(default_factory=list)
    retrieved_case_summary: Optional[str] = None


@dataclass(frozen=True)
class DecisionQuestion:
    """单个决策问题。kind 决定 Provider 计算哪个 primitive。

    kind 必须是 QUESTION_KINDS 之一；payload 携带该问题所需的操作性上下文。
    """

    kind: str
    payload: Dict[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        if self.kind not in QUESTION_KINDS:
            raise ValueError(f"unknown question kind: {self.kind!r}")


@dataclass(frozen=True)
class EmpiricalSupport:
    """经验校准证据：概率必须带样本量与来源，否则不可用于 gating。"""

    probability: float
    sample_count: int
    successes: int
    failures: int
    ci_low: float
    ci_high: float
    grouping_level: str  # exact | channel_failure | channel | global
    calibration_version: str


@dataclass
class GuiDecision:
    """统一结构化输出（可变累加结果：Provider 逐问题填充字段）。

    - safe_to_continue 是 policy_result：由 Policy 层综合 risk_class（规则）、
      retry budget、deterministic preconditions 与可选 P(success) 生成，
      Judge / Provider 不直接输出（保持 None）。
    - confidence / confidence_type 反映给出概率答案的 primary 问题。
    - support 携带经验校准证据；缺失时 confidence 不可解释为概率。
    """

    provider: ProviderKind
    confidence: Optional[float] = None
    confidence_type: ConfidenceType = ConfidenceType.UNKNOWN
    recovery_route: Optional[RecoveryChoice] = None
    recovery_probabilities: Dict[str, float] = field(default_factory=dict)
    need_escalation: Optional[float] = None
    action_succeeded: Optional[float] = None
    safe_to_continue: Optional[float] = None  # policy_result，非 Judge 输出
    model_version: str = "rule"
    calibration_version: Optional[str] = None
    support: Optional[EmpiricalSupport] = None
    reason_code: Optional[str] = None
    extra: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class DecisionCapabilities:
    """Provider 能力声明，供 Decision Router 按能力选择，不做硬编码判断。"""

    choice: bool = False
    score: bool = False
    boolean_probability: bool = False
    calibrated_probability: bool = False
    batch_decision: bool = False
    local_execution: bool = False
    long_context: bool = False
    rag_reasoning: bool = False
    max_context: int = 0
    expected_latency_class: str = "unknown"


@dataclass(frozen=True)
class ProviderHealth:
    healthy: bool
    detail: str = ""


@runtime_checkable
class DecisionProvider(Protocol):
    """DecisionProvider SPI：capabilities / health / decide。

    Provider 只产生结构化判断；不执行 GUI action、不修改 Skill、
    不写终态 PASS。未来 Laya / Jev / LLM 均实现同一契约。
    """

    def capabilities(self) -> DecisionCapabilities:
        ...

    def health(self) -> ProviderHealth:
        ...

    def decide(
        self,
        state: GuiDecisionState,
        questions: List[DecisionQuestion],
    ) -> GuiDecision:
        ...
