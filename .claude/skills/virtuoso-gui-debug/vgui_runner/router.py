"""vgui_runner.router - Pure function action routing decision layer.

MVP: returns RouteDecision and writes to trace. Does NOT auto-switch executors.

Routing priority:
  P0: skill          (VCLI/SKILL direct semantic operation)
  P1: vcli_x11       (vcli window action-x11 over SSH)
  P2: local_x11      (local xdotool)
  P3: vision         (manual/visual confirmation - opt-in only)
  REJECTED:          (cannot satisfy constraints)
"""

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional

from .model import Operation


class Channel(str, Enum):
    SKILL = "skill"
    VCLI_X11 = "vcli_x11"
    LOCAL_X11 = "local_x11"
    VISION = "vision"
    REJECTED = "rejected"


class RiskClass(str, Enum):
    READ_ONLY = "read_only"
    IDEMPOTENT_WRITE = "idempotent_write"
    NON_IDEMPOTENT_WRITE = "non_idempotent_write"
    DESTRUCTIVE = "destructive"


# Operations that map cleanly to VCLI/SKILL semantic calls.
# VCLI_CALL is intentionally NOT auto-routed to skill — it requires explicit
# function name + args and may execute arbitrary SKILL. We mark it as
# non_idempotent_write and require explicit policy override.
_SKILL_OPERATIONS = {
    Operation.VCLI_LOAD: RiskClass.IDEMPOTENT_WRITE,
    Operation.CIW_INPUT: RiskClass.NON_IDEMPOTENT_WRITE,
    Operation.SCREENSHOT: RiskClass.READ_ONLY,
}

# Operations that work through vcli X11 (window action-x11 over SSH).
_VCLI_X11_OPERATIONS = {
    Operation.WINDOW_WAIT,
    Operation.WINDOW_ACTIVATE,
    Operation.WINDOW_DISCOVER,
    Operation.KEY,
    Operation.TYPE,
    Operation.CLICK_REL,
    Operation.CLICK_ABS,
    Operation.DOUBLE_CLICK,
    Operation.DRAG_REL,
    Operation.SCROLL,
    Operation.MINIMIZE,
    Operation.MAXIMIZE,
    Operation.CLOSE,
    Operation.DISMISS_DIALOG,
}

# Operations that only make sense on local X11 (no remote DISPLAY).
_LOCAL_X11_ONLY = {
    # Currently none — vcli_x11 covers all GUI ops. Local is fallback.
}

# Operations that are inherently read-only.
_READ_ONLY_OPERATIONS = {
    Operation.SCREENSHOT,
    Operation.WINDOW_DISCOVER,
    Operation.WINDOW_WAIT,
    Operation.WINDOW_ACTIVATE,
    Operation.VERIFY,
}

# Operations that are destructive / irreversible.
_DESTRUCTIVE_OPERATIONS = {
    Operation.CLOSE,
    Operation.DISMISS_DIALOG,
}


@dataclass(frozen=True)
class ActionRequest:
    """The action to route."""
    operation: Operation
    step_id: str
    arguments: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class CapabilitySnapshot:
    """Current environment capabilities at routing time."""
    vcli_available: bool = False
    session_alive: bool = False
    pid_valid: bool = False
    display_available: bool = False
    window_identity_known: bool = False
    remote_x11_allowed: bool = False
    local_x11_available: bool = False
    vision_enabled: bool = False
    ssh_budget_remaining: int = 0  # 0 = exhausted
    daemon_healthy: bool = False


@dataclass(frozen=True)
class RoutePolicy:
    """User/system routing policy overrides."""
    allow_vision: bool = False
    allow_vcli_call: bool = False  # explicit opt-in for arbitrary SKILL calls
    require_window_identity: bool = True
    max_ssh_connections: int = 4
    allow_auto_fallback: bool = True
    allowed_channels: Optional[List[Channel]] = None  # None = all allowed


@dataclass(frozen=True)
class RouteDecision:
    """The routing decision output."""
    channel: Channel
    reason: str
    risk_class: RiskClass
    confidence: float  # 0.0 - 1.0
    required_capabilities: List[str] = field(default_factory=list)
    fallbacks: List[Channel] = field(default_factory=list)
    rejected: bool = False

    def to_trace_details(self) -> dict:
        """Serialize to stable, JSON-safe trace details.

        Rules:
        - No command arguments, tokens, or environment variables.
        - Channel/risk are enum values (strings).
        - Reason is a short code-like string, not free-form prose.
        """
        return {
            "channel": self.channel.value,
            "risk_class": self.risk_class.value,
            "confidence": round(self.confidence, 2),
            "reason_code": _reason_to_code(self.reason),
            "required_capabilities": list(self.required_capabilities),
            "fallbacks": [fb.value for fb in self.fallbacks],
            "rejected": self.rejected,
        }


def _reason_to_code(reason: str) -> str:
    """Map free-form reason to a stable short code for trace indexing."""
    r = reason.lower()
    if "vcli_call" in r or "allow_vcli_call" in r:
        return "vcli_call_blocked"
    if "ssh budget" in r or "budget exhausted" in r:
        return "ssh_budget_exhausted"
    if "window_identity" in r or "identity" in r:
        return "no_window_identity"
    if "no executor available" in r:
        return "no_executor"
    if "no route" in r:
        return "no_route"
    if "direct vcli" in r or "skill" in r:
        return "skill_available"
    if "vcli x11" in r or "action-x11" in r:
        return "vcli_x11_available"
    if "local xdotool" in r or "local x11" in r:
        return "local_x11_available"
    if "visual" in r or "vision" in r:
        return "vision_fallback"
    return "other"


def emit_route_decision(trace, step_id: str, attempt: int, decision: RouteDecision) -> None:
    """Emit a stable ROUTE_DECIDED event to trace.

    Called exactly once per (step_id, attempt).
    Rejected routes also emit (with rejected=true).
    Does NOT invoke any executor or fallback.
    """
    trace.emit(
        state="ROUTE_DECIDED",
        step_id=step_id,
        attempt=attempt,
        outcome="rejected" if decision.rejected else "selected",
        details=decision.to_trace_details(),
    )


def _classify_risk(operation: Operation) -> RiskClass:
    if operation in _READ_ONLY_OPERATIONS:
        return RiskClass.READ_ONLY
    if operation in _DESTRUCTIVE_OPERATIONS:
        return RiskClass.DESTRUCTIVE
    if operation == Operation.VCLI_CALL:
        return RiskClass.NON_IDEMPOTENT_WRITE
    if operation in _SKILL_OPERATIONS:
        return _SKILL_OPERATIONS[operation]
    # Default for GUI operations
    return RiskClass.NON_IDEMPOTENT_WRITE


def _capabilities_missing(
    channel: Channel,
    caps: CapabilitySnapshot,
    policy: RoutePolicy,
) -> List[str]:
    """Return list of missing required capabilities for a channel."""
    missing = []
    if channel == Channel.SKILL:
        if not caps.vcli_available:
            missing.append("vcli_available")
        if not caps.session_alive:
            missing.append("session_alive")
        if not caps.daemon_healthy:
            missing.append("daemon_healthy")
    elif channel == Channel.VCLI_X11:
        if not caps.vcli_available:
            missing.append("vcli_available")
        if not caps.session_alive:
            missing.append("session_alive")
        if not caps.pid_valid:
            missing.append("pid_valid")
        if not caps.display_available:
            missing.append("display_available")
        if not caps.remote_x11_allowed:
            missing.append("remote_x11_allowed")
        if policy.require_window_identity and not caps.window_identity_known:
            missing.append("window_identity_known")
    elif channel == Channel.LOCAL_X11:
        if not caps.local_x11_available:
            missing.append("local_x11_available")
        if not caps.display_available:
            missing.append("display_available")
        # Local xdotool cannot do server-side identity revalidation.
        # If policy requires identity, we must have already resolved it via
        # vcli before falling back to local.
        if policy.require_window_identity and not caps.window_identity_known:
            missing.append("window_identity_known")
    elif channel == Channel.VISION:
        if not caps.display_available:
            missing.append("display_available")
    return missing


def _channel_allowed(channel: Channel, policy: RoutePolicy) -> bool:
    if policy.allowed_channels is None:
        return True
    return channel in policy.allowed_channels


def route(
    request: ActionRequest,
    caps: CapabilitySnapshot,
    policy: RoutePolicy,
) -> RouteDecision:
    """
    Pure function: decide which channel should execute this action.

    MVP behavior: returns the decision but does NOT auto-execute via
    a different executor. The caller logs the decision to trace and
    continues with the explicitly selected executor.
    """
    risk = _classify_risk(request.operation)
    fallbacks: List[Channel] = []

    # P0: SKILL / VCLI direct
    if request.operation in _SKILL_OPERATIONS or request.operation == Operation.VCLI_CALL:
        # VCLI_CALL requires explicit policy opt-in
        if request.operation == Operation.VCLI_CALL and not policy.allow_vcli_call:
            return RouteDecision(
                channel=Channel.REJECTED,
                reason="VCLI_CALL requires explicit policy.allow_vcli_call=True (arbitrary SKILL execution)",
                risk_class=risk,
                confidence=0.0,
                required_capabilities=[],
                fallbacks=[],
                rejected=True,
            )

        if _channel_allowed(Channel.SKILL, policy):
            missing = _capabilities_missing(Channel.SKILL, caps, policy)
            if not missing:
                # Check SSH budget for VCLI path
                if caps.ssh_budget_remaining <= 0:
                    fallbacks.append(Channel.VCLI_X11)
                    fallbacks.append(Channel.LOCAL_X11)
                    return RouteDecision(
                        channel=Channel.REJECTED,
                        reason=f"SSH budget exhausted (remaining={caps.ssh_budget_remaining}); cannot reach VCLI/SKILL",
                        risk_class=risk,
                        confidence=0.0,
                        required_capabilities=["ssh_budget"],
                        fallbacks=fallbacks,
                        rejected=True,
                    )
                return RouteDecision(
                    channel=Channel.SKILL,
                    reason=f"Operation {request.operation.value} maps to direct VCLI/SKILL semantic call",
                    risk_class=risk,
                    confidence=0.95,
                    required_capabilities=missing,
                    fallbacks=[Channel.VCLI_X11, Channel.LOCAL_X11],
                )
            fallbacks.append(Channel.VCLI_X11)

    # P1: vcli X11
    if request.operation in _VCLI_X11_OPERATIONS or Channel.VCLI_X11 not in fallbacks:
        if _channel_allowed(Channel.VCLI_X11, policy):
            missing = _capabilities_missing(Channel.VCLI_X11, caps, policy)
            if not missing:
                if caps.ssh_budget_remaining <= 0:
                    fallbacks.append(Channel.LOCAL_X11)
                    return RouteDecision(
                        channel=Channel.REJECTED,
                        reason=f"SSH budget exhausted (remaining={caps.ssh_budget_remaining}); vcli X11 unavailable",
                        risk_class=risk,
                        confidence=0.0,
                        required_capabilities=["ssh_budget"],
                        fallbacks=fallbacks,
                        rejected=True,
                    )
                return RouteDecision(
                    channel=Channel.VCLI_X11,
                    reason=f"Operation {request.operation.value} via vcli window action-x11 over SSH",
                    risk_class=risk,
                    confidence=0.85,
                    required_capabilities=missing,
                    fallbacks=[Channel.LOCAL_X11, Channel.VISION] if policy.allow_vision else [Channel.LOCAL_X11],
                )
            fallbacks.append(Channel.LOCAL_X11)

    # P2: local X11
    if _channel_allowed(Channel.LOCAL_X11, policy):
        missing = _capabilities_missing(Channel.LOCAL_X11, caps, policy)
        if not missing:
            return RouteDecision(
                channel=Channel.LOCAL_X11,
                reason=f"Operation {request.operation.value} via local xdotool",
                risk_class=risk,
                confidence=0.70,
                required_capabilities=missing,
                fallbacks=[Channel.VISION] if policy.allow_vision else [],
            )
        fallbacks.append(Channel.VISION)

    # P3: vision (opt-in only)
    if policy.allow_vision and _channel_allowed(Channel.VISION, policy):
        missing = _capabilities_missing(Channel.VISION, caps, policy)
        if not missing:
            return RouteDecision(
                channel=Channel.VISION,
                reason=f"Fallback to manual/visual for {request.operation.value}",
                risk_class=risk,
                confidence=0.40,
                required_capabilities=missing,
                fallbacks=[],
            )

    # REJECTED
    reason_parts = []
    if not caps.vcli_available and not caps.local_x11_available:
        reason_parts.append("no executor available")
    if policy.require_window_identity and not caps.window_identity_known:
        reason_parts.append("window identity not verified")
    if caps.ssh_budget_remaining <= 0 and not caps.local_x11_available:
        reason_parts.append("SSH budget exhausted and no local X11")

    return RouteDecision(
        channel=Channel.REJECTED,
        reason="; ".join(reason_parts) if reason_parts else "no route satisfies policy",
        risk_class=risk,
        confidence=0.0,
        required_capabilities=[],
        fallbacks=fallbacks,
        rejected=True,
    )
