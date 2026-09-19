"""vgui_runner.recovery - Pure recovery decision policy.

MVP: decision layer only. Does NOT call executors or modify state.
Runner consults this policy before any retry/fallback/rollback.

Decision outputs:
- retry:           safe to retry same channel
- rollback:        undo partial state, then retry
- fallback:        switch to next channel
- abort:           stop, mark failed
- manual:          requires human intervention
"""

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional

from .router import RiskClass, Channel


class RecoveryAction(str, Enum):
    RETRY = "retry"
    ROLLBACK = "rollback"
    FALLBACK = "fallback"
    ABORT = "abort"
    MANUAL = "manual"


class ErrorCategory(str, Enum):
    TIMEOUT = "timeout"
    CONNECTION_LOST = "connection_lost"
    WINDOW_GONE = "window_gone"
    VERIFY_FAILED = "verify_failed"
    VERIFY_UNAVAILABLE = "verify_unavailable"
    ROLLBACK_FAILED = "rollback_failed"
    UNKNOWN = "unknown"


@dataclass(frozen=True)
class RecoveryPolicy:
    """Global recovery policy constraints."""
    max_attempts: int = 2
    total_deadline_ms: int = 30000  # covers execute+verify+rollback+backoff
    backoff_ms: int = 500
    fallback_channels: tuple = ("vcli_x11", "local_x11")
    verify_before_retry: bool = True
    rollback_deadline_ms: int = 5000


@dataclass(frozen=True)
class RecoveryRequest:
    """What happened and what we know."""
    step_id: str
    attempt: int
    risk_class: RiskClass
    error_category: ErrorCategory
    error_message: str = ""
    result_uncertain: bool = False  # network lost, response unknown
    elapsed_ms: int = 0
    has_rollback: bool = False
    current_channel: Channel = Channel.SKILL


@dataclass(frozen=True)
class RecoveryDecision:
    """What Runner should do next."""
    action: RecoveryAction
    reason_code: str
    risk_class: RiskClass
    remaining_deadline_ms: int
    verification_required: bool
    next_channel: Optional[Channel] = None
    attempts_used: int = 0

    def to_trace_details(self) -> dict:
        return {
            "action": self.action.value,
            "cause": self.reason_code,
            "risk_class": self.risk_class.value,
            "remaining_deadline_ms": self.remaining_deadline_ms,
            "verification_required": self.verification_required,
            "next_channel": self.next_channel.value if self.next_channel else None,
            "attempts_used": self.attempts_used,
        }


# Which error categories are safe to retry, by risk class.
_RETRY_BY_RISK = {
    RiskClass.READ_ONLY: {ErrorCategory.TIMEOUT, ErrorCategory.CONNECTION_LOST,
                          ErrorCategory.WINDOW_GONE, ErrorCategory.VERIFY_FAILED,
                          ErrorCategory.VERIFY_UNAVAILABLE, ErrorCategory.UNKNOWN},
    RiskClass.IDEMPOTENT_WRITE: {ErrorCategory.TIMEOUT, ErrorCategory.CONNECTION_LOST,
                                  ErrorCategory.VERIFY_FAILED, ErrorCategory.VERIFY_UNAVAILABLE,
                                  ErrorCategory.UNKNOWN},
    RiskClass.NON_IDEMPOTENT_WRITE: set(),  # never auto-replay
    RiskClass.DESTRUCTIVE: set(),           # always manual
}


def decide_recovery(
    req: RecoveryRequest,
    policy: RecoveryPolicy,
) -> RecoveryDecision:
    """Pure function: decide what to do after a failure.

    Does NOT call executors. Runner applies the decision.
    """
    remaining = policy.total_deadline_ms - req.elapsed_ms

    # Hard deadline exceeded
    if remaining <= 0:
        return RecoveryDecision(
            action=RecoveryAction.ABORT,
            reason_code="deadline_exceeded",
            risk_class=req.risk_class,
            remaining_deadline_ms=0,
            verification_required=False,
            attempts_used=req.attempt + 1,
        )

    # Destructive ops always need human
    if req.risk_class == RiskClass.DESTRUCTIVE:
        return RecoveryDecision(
            action=RecoveryAction.MANUAL,
            reason_code="destructive_requires_human",
            risk_class=req.risk_class,
            remaining_deadline_ms=remaining,
            verification_required=False,
            attempts_used=req.attempt + 1,
        )

    # Non-idempotent: never auto-replay
    if req.risk_class == RiskClass.NON_IDEMPOTENT_WRITE:
        if req.result_uncertain:
            # Result unknown — must verify before deciding
            return RecoveryDecision(
                action=RecoveryAction.MANUAL,
                reason_code="non_idempotent_uncertain",
                risk_class=req.risk_class,
                remaining_deadline_ms=remaining,
                verification_required=True,
                attempts_used=req.attempt + 1,
            )
        return RecoveryDecision(
            action=RecoveryAction.ABORT,
            reason_code="non_idempotent_no_retry",
            risk_class=req.risk_class,
            remaining_deadline_ms=remaining,
            verification_required=False,
            attempts_used=req.attempt + 1,
        )

    # Rollback failure is terminal
    if req.error_category == ErrorCategory.ROLLBACK_FAILED:
        return RecoveryDecision(
            action=RecoveryAction.MANUAL,
            reason_code="rollback_failed",
            risk_class=req.risk_class,
            remaining_deadline_ms=remaining,
            verification_required=False,
            attempts_used=req.attempt + 1,
        )

    # Check retry budget
    if req.attempt + 1 >= policy.max_attempts:
        # Try fallback channel before aborting
        if policy.fallback_channels and req.current_channel != Channel.REJECTED:
            next_ch = Channel(policy.fallback_channels[0])
            return RecoveryDecision(
                action=RecoveryAction.FALLBACK,
                reason_code="retry_budget_exhausted_fallback",
                risk_class=req.risk_class,
                remaining_deadline_ms=remaining,
                verification_required=True,
                next_channel=next_ch,
                attempts_used=req.attempt + 1,
            )
        return RecoveryDecision(
            action=RecoveryAction.ABORT,
            reason_code="retry_budget_exhausted",
            risk_class=req.risk_class,
            remaining_deadline_ms=remaining,
            verification_required=False,
            attempts_used=req.attempt + 1,
        )

    # Check if this error category is retryable for this risk class
    retryable_errors = _RETRY_BY_RISK.get(req.risk_class, set())
    if req.error_category not in retryable_errors:
        return RecoveryDecision(
            action=RecoveryAction.ABORT,
            reason_code=f"error_not_retryable_{req.error_category.value}",
            risk_class=req.risk_class,
            remaining_deadline_ms=remaining,
            verification_required=False,
            attempts_used=req.attempt + 1,
        )

    # Result uncertain: must verify first (especially for writes)
    if req.result_uncertain and policy.verify_before_retry:
        return RecoveryDecision(
            action=RecoveryAction.ROLLBACK if req.has_rollback else RecoveryAction.RETRY,
            reason_code="uncertain_verify_first",
            risk_class=req.risk_class,
            remaining_deadline_ms=remaining,
            verification_required=True,
            attempts_used=req.attempt + 1,
        )

    # Safe to retry
    return RecoveryDecision(
        action=RecoveryAction.ROLLBACK if req.has_rollback else RecoveryAction.RETRY,
        reason_code=f"retry_{req.error_category.value}",
        risk_class=req.risk_class,
        remaining_deadline_ms=remaining,
        verification_required=policy.verify_before_retry and req.risk_class != RiskClass.READ_ONLY,
        attempts_used=req.attempt + 1,
    )
