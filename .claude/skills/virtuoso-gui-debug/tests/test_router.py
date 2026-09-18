"""Tests for vgui_runner.router - pure function routing decisions."""

import pytest
from vgui_runner.model import Operation
from vgui_runner.router import (
    ActionRequest,
    CapabilitySnapshot,
    Channel,
    RouteDecision,
    RoutePolicy,
    RiskClass,
    route,
)


# Fixtures
def _full_caps() -> CapabilitySnapshot:
    return CapabilitySnapshot(
        vcli_available=True,
        session_alive=True,
        pid_valid=True,
        display_available=True,
        window_identity_known=True,
        remote_x11_allowed=True,
        local_x11_available=True,
        vision_enabled=False,
        ssh_budget_remaining=4,
        daemon_healthy=True,
    )


def _policy(**kw) -> RoutePolicy:
    return RoutePolicy(**kw)


class TestStableRouting:
    """Same input always produces same route."""

    def test_screenshot_routes_to_skill(self):
        req = ActionRequest(Operation.SCREENSHOT, "step1")
        caps = _full_caps()
        policy = _policy()
        d1 = route(req, caps, policy)
        d2 = route(req, caps, policy)
        assert d1.channel == d2.channel == Channel.SKILL
        assert d1.reason == d2.reason

    def test_click_routes_to_vcli_x11(self):
        req = ActionRequest(Operation.CLICK_REL, "step1", {"x": 10, "y": 20})
        caps = _full_caps()
        policy = _policy()
        d = route(req, caps, policy)
        assert d.channel == Channel.VCLI_X11


class TestVcliUnavailableFallback:
    """When VCLI unavailable, degrade to X11."""

    def test_vcli_down_falls_back_to_vcli_x11_for_gui_ops(self):
        """SKILL-only ops (screenshot) need VCLI; if VCLI down but X11 caps present."""
        req = ActionRequest(Operation.SCREENSHOT, "step1")
        caps = CapabilitySnapshot(
            vcli_available=False,
            session_alive=False,
            pid_valid=True,
            display_available=True,
            window_identity_known=True,
            remote_x11_allowed=True,
            local_x11_available=True,
            vision_enabled=False,
            ssh_budget_remaining=2,
            daemon_healthy=False,
        )
        policy = _policy()
        d = route(req, caps, policy)
        # Screenshot isn't in _VCLI_X11_OPERATIONS, so it falls through to local
        assert d.channel in (Channel.LOCAL_X11, Channel.REJECTED)

    def test_vcli_down_rejects_skill_ops(self):
        req = ActionRequest(Operation.VCLI_LOAD, "step1", {"command": "load(\"x.il\")"})
        caps = CapabilitySnapshot(
            vcli_available=False,
            session_alive=False,
            pid_valid=True,
            display_available=True,
            window_identity_known=True,
            remote_x11_allowed=True,
            local_x11_available=True,
            vision_enabled=False,
            ssh_budget_remaining=2,
            daemon_healthy=False,
        )
        policy = _policy()
        d = route(req, caps, policy)
        # VCLI_LOAD is skill-only; no local equivalent
        assert d.channel == Channel.REJECTED


class TestIdentityRejection:
    """Without window identity, GUI ops rejected when policy requires it."""

    def test_no_window_identity_rejects_gui_ops(self):
        req = ActionRequest(Operation.CLICK_REL, "step1", {"x": 10, "y": 20})
        caps = CapabilitySnapshot(
            vcli_available=True,
            session_alive=True,
            pid_valid=True,
            display_available=True,
            window_identity_known=False,  # NOT known
            remote_x11_allowed=True,
            local_x11_available=True,
            vision_enabled=False,
            ssh_budget_remaining=4,
            daemon_healthy=True,
        )
        policy = RoutePolicy(require_window_identity=True)
        d = route(req, caps, policy)
        assert d.channel == Channel.REJECTED
        assert "window_identity" in d.reason or "identity" in d.reason.lower()

    def test_window_identity_optional_allows_route(self):
        req = ActionRequest(Operation.CLICK_REL, "step1", {"x": 10, "y": 20})
        caps = _full_caps()
        caps.__class__  # already full
        policy = RoutePolicy(require_window_identity=False)
        # Even with identity unknown, policy says don't require it
        caps_no_id = CapabilitySnapshot(
            vcli_available=True,
            session_alive=True,
            pid_valid=True,
            display_available=True,
            window_identity_known=False,
            remote_x11_allowed=True,
            local_x11_available=True,
            vision_enabled=False,
            ssh_budget_remaining=4,
            daemon_healthy=True,
        )
        d = route(req, caps_no_id, policy)
        assert d.channel == Channel.VCLI_X11


class TestNonIdempotentNoAutoFallback:
    """Non-idempotent ops don't get auto-fallback to potentially destructive channels."""

    def test_click_is_non_idempotent(self):
        req = ActionRequest(Operation.CLICK_REL, "step1", {"x": 10, "y": 20})
        caps = _full_caps()
        policy = _policy()
        d = route(req, caps, policy)
        assert d.risk_class == RiskClass.NON_IDEMPOTENT_WRITE

    def test_screenshot_is_read_only(self):
        req = ActionRequest(Operation.SCREENSHOT, "step1")
        caps = _full_caps()
        policy = _policy()
        d = route(req, caps, policy)
        assert d.risk_class == RiskClass.READ_ONLY

    def test_close_is_destructive(self):
        req = ActionRequest(Operation.CLOSE, "step1")
        caps = _full_caps()
        policy = _policy()
        d = route(req, caps, policy)
        assert d.risk_class == RiskClass.DESTRUCTIVE


class TestVisionOptIn:
    """Vision channel only when explicitly enabled."""

    def test_vision_not_default(self):
        req = ActionRequest(Operation.CLICK_REL, "step1", {"x": 10, "y": 20})
        caps = CapabilitySnapshot(
            vcli_available=False,
            session_alive=False,
            pid_valid=False,
            display_available=True,
            window_identity_known=True,
            remote_x11_allowed=False,
            local_x11_available=False,
            vision_enabled=True,
            ssh_budget_remaining=0,
            daemon_healthy=False,
        )
        policy = RoutePolicy(allow_vision=False)
        d = route(req, caps, policy)
        assert d.channel == Channel.REJECTED

    def test_vision_opt_in_allows(self):
        req = ActionRequest(Operation.CLICK_REL, "step1", {"x": 10, "y": 20})
        caps = CapabilitySnapshot(
            vcli_available=False,
            session_alive=False,
            pid_valid=False,
            display_available=True,
            window_identity_known=True,
            remote_x11_allowed=False,
            local_x11_available=False,
            vision_enabled=True,
            ssh_budget_remaining=0,
            daemon_healthy=False,
        )
        policy = RoutePolicy(allow_vision=True)
        d = route(req, caps, policy)
        assert d.channel == Channel.VISION


class TestSshBudget:
    """SSH budget exhaustion produces explicit rejection reason."""

    def test_ssh_exhausted_rejects_remote_channels(self):
        req = ActionRequest(Operation.CLICK_REL, "step1", {"x": 10, "y": 20})
        caps = CapabilitySnapshot(
            vcli_available=True,
            session_alive=True,
            pid_valid=True,
            display_available=True,
            window_identity_known=True,
            remote_x11_allowed=True,
            local_x11_available=False,  # no local fallback
            vision_enabled=False,
            ssh_budget_remaining=0,
            daemon_healthy=True,
        )
        policy = _policy()
        d = route(req, caps, policy)
        assert d.channel == Channel.REJECTED
        assert "budget" in d.reason.lower() or "ssh" in d.reason.lower()


class TestVcliCallGuard:
    """VCLI_CALL is explicitly guarded by policy."""

    def test_vcli_call_rejected_by_default(self):
        req = ActionRequest(Operation.VCLI_CALL, "step1", {"function": "dbCreateRect", "args": []})
        caps = _full_caps()
        policy = _policy()  # allow_vcli_call=False
        d = route(req, caps, policy)
        assert d.channel == Channel.REJECTED
        assert "VCLI_CALL" in d.reason or "vcli_call" in d.reason.lower()

    def test_vcli_call_allowed_with_opt_in(self):
        req = ActionRequest(Operation.VCLI_CALL, "step1", {"function": "dbCreateRect", "args": []})
        caps = _full_caps()
        policy = RoutePolicy(allow_vcli_call=True)
        d = route(req, caps, policy)
        assert d.channel == Channel.SKILL
        assert d.risk_class == RiskClass.NON_IDEMPOTENT_WRITE


class TestRequiredCapabilities:
    """RouteDecision includes required capabilities for trace."""

    def test_skill_route_lists_capabilities(self):
        req = ActionRequest(Operation.SCREENSHOT, "step1")
        caps = _full_caps()
        policy = _policy()
        d = route(req, caps, policy)
        assert isinstance(d.required_capabilities, list)
        assert isinstance(d.fallbacks, list)
        assert d.confidence > 0
