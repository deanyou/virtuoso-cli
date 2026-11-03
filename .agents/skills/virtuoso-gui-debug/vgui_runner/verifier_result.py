"""vgui_runner.verifier_result - Unified verification result model.

Step 1: define the model + adapter from legacy Optional[Dict].
Step 2 (deferred): migrate Executor.verify() to return VerifierResult.

Status semantics:
- passed:    postcondition holds
- failed:    verification ran, state does not match
- skipped:   policy explicitly skipped this check
- unavailable: verification channel cannot run (not a success)
"""

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, Optional, Tuple


class VerifyStatus(str, Enum):
    PASSED = "passed"
    FAILED = "failed"
    SKIPPED = "skipped"
    UNAVAILABLE = "unavailable"


@dataclass(frozen=True)
class VerifierResult:
    """Structured verification outcome for one (step, attempt, predicate)."""
    predicate: str
    status: VerifyStatus
    expected: Any = None
    observed: Any = None
    reason_code: Optional[str] = None
    evidence_refs: Tuple[str, ...] = field(default_factory=tuple)
    step_id: Optional[str] = None
    attempt: int = 0
    route_event_seq: Optional[int] = None  # links to ROUTE_DECIDED event

    def to_trace_details(self) -> dict:
        """Serialize to JSON-safe trace details.

        Rules:
        - expected/observed are redacted via _redact.
        - evidence_refs are paths/IDs only, never file contents.
        - No command strings, tokens, or environment values.
        """
        return {
            "predicate": self.predicate,
            "status": self.status.value,
            "expected": _redact(self.expected),
            "observed": _redact(self.observed),
            "reason_code": self.reason_code,
            "evidence_refs": list(self.evidence_refs),
            "step_id": self.step_id,
            "attempt": self.attempt,
            "route_event_seq": self.route_event_seq,
        }

    @property
    def ok(self) -> bool:
        """True only when verification explicitly passed."""
        return self.status == VerifyStatus.PASSED

    @property
    def is_failure(self) -> bool:
        """True when verification ran and failed (not just unavailable)."""
        return self.status == VerifyStatus.FAILED


# Keys whose values should be redacted in expected/observed.
_SENSITIVE_KEYS = frozenset({
    "password", "token", "secret", "key", "auth", "credential",
    "session", "cookie", "api_key", "private",
})


def _redact(value: Any, depth: int = 0) -> Any:
    """Redact sensitive values before writing to trace.

    Scalars: pass through unless they look like tokens (heuristic).
    Dicts: recurse, redact values under sensitive keys.
    Lists/tuples: recurse.
    """
    if depth > 4:
        return "<max_depth>"
    if value is None or isinstance(value, (bool, int, float)):
        return value
    if isinstance(value, str):
        # Heuristic: skip very long strings (likely command output)
        if len(value) > 200:
            return value[:80] + "..."
        return value
    if isinstance(value, dict):
        out = {}
        for k, v in value.items():
            if isinstance(k, str) and k.lower() in _SENSITIVE_KEYS:
                out[k] = "<redacted>"
            else:
                out[k] = _redact(v, depth + 1)
        return out
    if isinstance(value, (list, tuple)):
        return [_redact(item, depth + 1) for item in value[:10]]
    return f"<{type(value).__name__}>"


def from_legacy_error(
    predicate: str,
    expected: Any,
    legacy_err: Optional[Dict[str, Any]],
    step_id: Optional[str] = None,
    attempt: int = 0,
) -> "VerifierResult":
    """Adapter: convert legacy Optional[Dict] error to VerifierResult.

    Legacy contract:
    - None  => verification passed
    - dict  => verification failed (contains "error" key)

    This adapter preserves backward compatibility while adding structure.
    """
    if legacy_err is None:
        return VerifierResult(
            predicate=predicate,
            status=VerifyStatus.PASSED,
            expected=expected,
            observed=expected,
            reason_code=None,
            step_id=step_id,
            attempt=attempt,
        )

    # Legacy error dict — extract observed if present
    observed = legacy_err.get("observed", legacy_err.get("error", "unknown"))
    reason = legacy_err.get("error", "verify_failed")
    return VerifierResult(
        predicate=predicate,
        status=VerifyStatus.FAILED,
        expected=expected,
        observed=observed,
        reason_code=reason[:80] if isinstance(reason, str) else "verify_failed",
        step_id=step_id,
        attempt=attempt,
    )


def map_to_error_code(result: VerifierResult) -> Optional[str]:
    """Map VerifierResult to existing Runner error codes.

    Returns None when verification should not fail the run.
    Preserves backward compatibility with ERROR_VERIFY.
    """
    from .engine import ERROR_VERIFY  # local import to avoid cycle

    if result.status == VerifyStatus.PASSED:
        return None
    if result.status == VerifyStatus.SKIPPED:
        return None  # skipped is not a failure
    if result.status == VerifyStatus.UNAVAILABLE:
        # Unavailable is NOT a pass, but it's not a verify-failed either.
        # For now, treat as verify failure to preserve old behavior.
        # Future: add ERROR_VERIFY_UNAVAILABLE.
        return ERROR_VERIFY
    # FAILED
    return ERROR_VERIFY
