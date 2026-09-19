"""vgui_runner.engine - State machine, fake executor, and runner."""

import json
import time
from abc import ABC
from abc import abstractmethod
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any, Dict, Optional

from .model import Scenario, Step
from .trace import Trace
from .verifier_result import VerifierResult, VerifyStatus, from_legacy_error
from .recovery import RecoveryPolicy, RecoveryRequest, RecoveryAction, ErrorCategory, decide_recovery
from .router import RiskClass, Channel, ActionRequest, CapabilitySnapshot, RoutePolicy, route, emit_route_decision


def _op_risk_class(operation) -> RiskClass:
    """Map operation to risk class for recovery decisions."""
    from .model import Operation
    read_only_ops = {Operation.SCREENSHOT, Operation.WINDOW_WAIT,
                     Operation.WINDOW_DISCOVER, Operation.WINDOW_ACTIVATE,
                     Operation.MINIMIZE, Operation.MAXIMIZE}
    destructive_ops = {Operation.CLOSE, Operation.DISMISS_DIALOG}
    idempotent_ops = {Operation.KEY, Operation.TYPE, Operation.CLICK_REL,
                      Operation.CLICK_ABS, Operation.DOUBLE_CLICK,
                      Operation.DRAG_REL, Operation.SCROLL}
    if operation in read_only_ops:
        return RiskClass.READ_ONLY
    if operation in destructive_ops:
        return RiskClass.DESTRUCTIVE
    if operation in idempotent_ops:
        return RiskClass.IDEMPOTENT_WRITE
    return RiskClass.NON_IDEMPOTENT_WRITE


class RunState(str, Enum):
    PRECHECK = "PRECHECK"
    BASELINE = "BASELINE"
    EXECUTE = "EXECUTE"
    VERIFY = "VERIFY"
    RECOVER = "RECOVER"
    PASSED = "PASSED"
    FAILED = "FAILED"


class StepPhase(str, Enum):
    PRECHECK = "precheck"
    BASELINE = "baseline"
    EXECUTE = "execute"
    VERIFY = "verify"
    RECOVER = "recover"


class StepOutcome(str, Enum):
    SUCCESS = "SUCCESS"
    FAILURE = "FAILURE"


ERROR_PRECHECK = "PRECHECK_ERROR"
ERROR_BASELINE = "BASELINE_ERROR"
ERROR_EXECUTE = "EXECUTE_ERROR"
ERROR_RECOVER = "RECOVER_ERROR"
ERROR_VERIFY = "VERIFY_ERROR"
ERROR_VERIFY_UNAVAILABLE = "VERIFY_UNAVAILABLE"
ERROR_ROLLBACK = "ROLLBACK_ERROR"


def _json_value(value: Any) -> Any:
    """Convert the model's deeply frozen JSON values back to JSON containers."""
    if isinstance(value, tuple):
        return [_json_value(item) for item in value]
    if hasattr(value, "items"):
        return {key: _json_value(item) for key, item in value.items()}
    return value


@dataclass
class RunSummary:
    passed: bool
    failed_step_id: Optional[str]
    error_code: Optional[str]
    phase: Optional[str]


class Executor(ABC):
    """Protocol for step executors."""

    @abstractmethod
    def precheck(self, scenario: Scenario) -> Optional[Dict[str, Any]]:
        pass

    @abstractmethod
    def baseline(self, scenario: Scenario) -> Optional[Dict[str, Any]]:
        pass

    @abstractmethod
    def execute(self, step: Step, attempt: int) -> Optional[Dict[str, Any]]:
        pass

    @abstractmethod
    def verify(self, step: Step, attempt: int) -> Optional[Dict[str, Any]]:
        pass

    @abstractmethod
    def recover(self, step: Step, attempt: int, rollback: Optional[Dict[str, Any]]) -> Optional[Dict[str, Any]]:
        pass


class FakeExecutor(Executor):
    """Fake executor with outcomes per step, phase, and attempt.

    No subprocess, os.environ, shell, vcli, X11, or xdotool access.
    Outcomes dict format: {(step_id, phase_str, attempt): StepOutcome}
    """

    def __init__(self, outcomes: Optional[Dict[tuple, StepOutcome]] = None):
        self._outcomes: Dict[tuple, StepOutcome] = outcomes or {}

    def outcome(self, step_id: str, phase: StepPhase, attempt: int) -> StepOutcome:
        key = (step_id, phase.value, attempt)
        return self._outcomes.get(key, StepOutcome.SUCCESS)

    def precheck(self, scenario: Scenario) -> Optional[Dict[str, Any]]:
        if self.outcome("_precheck", StepPhase.PRECHECK, 0) == StepOutcome.FAILURE:
            return {"error": "precheck failed"}
        return None

    def baseline(self, scenario: Scenario) -> Optional[Dict[str, Any]]:
        if self.outcome("_baseline", StepPhase.BASELINE, 0) == StepOutcome.FAILURE:
            return {"error": "baseline failed"}
        return None

    def execute(self, step: Step, attempt: int) -> Optional[Dict[str, Any]]:
        if self.outcome(step.id, StepPhase.EXECUTE, attempt) == StepOutcome.FAILURE:
            return {"error": f"execute failed for step {step.id}"}
        return None

    def verify(self, step: Step, attempt: int) -> VerifierResult:
        """Return VerifierResult (new contract)."""
        predicate = dict(step.verifier).get("predicate", "unknown")
        expected = dict(step.verifier).get("expected")
        if self.outcome(step.id, StepPhase.VERIFY, attempt) == StepOutcome.FAILURE:
            return VerifierResult(
                predicate=predicate,
                status=VerifyStatus.FAILED,
                expected=expected,
                observed="<not measured>",
                reason_code="fake_verify_failed",
                step_id=step.id,
                attempt=attempt,
            )
        return VerifierResult(
            predicate=predicate,
            status=VerifyStatus.PASSED,
            expected=expected,
            observed=expected,
            step_id=step.id,
            attempt=attempt,
        )

    def recover(self, step: Step, attempt: int, rollback: Optional[Dict[str, Any]]) -> Optional[Dict[str, Any]]:
        if self.outcome(step.id, StepPhase.RECOVER, attempt) == StepOutcome.FAILURE:
            return {"error": f"recover failed for step {step.id}"}
        return None


class Runner:
    """State machine runner that executes scenarios step-by-step."""

    def __init__(self, executor: Executor, recovery_policy: Optional[RecoveryPolicy] = None,
                 caps: Optional[CapabilitySnapshot] = None, route_policy: Optional[RoutePolicy] = None):
        self._executor = executor
        self._recovery_policy = recovery_policy or RecoveryPolicy()
        self._caps = caps or CapabilitySnapshot(
            vcli_available=True, session_alive=True, pid_valid=True,
            display_available=True, window_identity_known=True,
            remote_x11_allowed=True, local_x11_available=True,
            vision_enabled=False, ssh_budget_remaining=10, daemon_healthy=True,
        )
        self._route_policy = route_policy or RoutePolicy()

    def run(self, scenario: Scenario, output_dir: Path) -> RunSummary:
        output_dir.mkdir(parents=True, exist_ok=False)

        task_path = output_dir / "task.json"
        tmp_task = output_dir / ".task.json.tmp"
        task_data = {
            "version": scenario.version,
            "task_id": scenario.task_id,
            "session_id": scenario.session_id,
            "pid": scenario.pid,
            "display": scenario.display,
            "cellview": {
                "lib": scenario.cellview.lib,
                "cell": scenario.cellview.cell,
                "view": scenario.cellview.view,
            },
            "steps": [
                {
                    "id": s.id,
                    "operation": s.operation.value,
                    "arguments": _json_value(s.arguments),
                    "verifier": _json_value(s.verifier),
                    "timeout_seconds": s.timeout_seconds,
                    "max_retries": s.max_retries,
                    "rollback": _json_value(s.rollback) if s.rollback else None,
                }
                for s in scenario.steps
            ],
        }
        with open(tmp_task, "w", encoding="utf-8") as f:
            json.dump(task_data, f, separators=(",", ":"))
        tmp_task.replace(task_path)

        trace_path = output_dir / "agent-actions.jsonl"
        trace = Trace(trace_path)

        state = RunState.PRECHECK
        failed_step_id: Optional[str] = None
        error_code: Optional[str] = None
        phase: Optional[str] = None

        # PRECHECK phase
        trace.emit(RunState.PRECHECK.value)
        start = time.monotonic()
        err = self._executor.precheck(scenario)
        duration_ms = int((time.monotonic() - start) * 1000)
        if err:
            trace.emit(RunState.PRECHECK.value, outcome="FAILURE", duration_ms=duration_ms, details=err)
            trace.emit(RunState.FAILED.value, step_id=None)
            trace.close()
            self._write_summary(output_dir, scenario.task_id, False, None, ERROR_PRECHECK, "PRECHECK")
            return RunSummary(passed=False, failed_step_id=None, error_code=ERROR_PRECHECK, phase="PRECHECK")

        trace.emit(RunState.PRECHECK.value, outcome="SUCCESS", duration_ms=duration_ms)
        state = RunState.BASELINE

        # BASELINE phase
        trace.emit(RunState.BASELINE.value)
        start = time.monotonic()
        err = self._executor.baseline(scenario)
        duration_ms = int((time.monotonic() - start) * 1000)
        if err:
            trace.emit(RunState.BASELINE.value, outcome="FAILURE", duration_ms=duration_ms, details=err)
            trace.emit(RunState.FAILED.value, step_id=None)
            trace.close()
            self._write_summary(output_dir, scenario.task_id, False, None, ERROR_BASELINE, "BASELINE")
            return RunSummary(passed=False, failed_step_id=None, error_code=ERROR_BASELINE, phase="BASELINE")

        trace.emit(RunState.BASELINE.value, outcome="SUCCESS", duration_ms=duration_ms)
        state = RunState.EXECUTE

        # Step execution loop — RecoveryPolicy driven
        step_index = 0
        run_deadline = time.monotonic() + self._recovery_policy.total_deadline_ms / 1000.0

        while state == RunState.EXECUTE:
            if step_index >= len(scenario.steps):
                state = RunState.PASSED
                break

            step = scenario.steps[step_index]
            max_retries = step.max_retries
            step_succeeded = False
            step_failed = False

            # Single finite for-attempt loop per step
            for attempt in range(max_retries + 1):
                # P1 Router: decide channel before execute
                req = ActionRequest(operation=step.operation, step_id=step.id, arguments=dict(step.arguments))
                decision = route(req, self._caps, self._route_policy)
                emit_route_decision(trace, step.id, attempt, decision)
                if decision.rejected:
                    trace.emit(RunState.EXECUTE.value, step_id=step.id, attempt=attempt, outcome="REJECTED",
                               details={"reason": decision.reason, "channel": decision.channel.value})
                    state = RunState.FAILED
                    failed_step_id = step.id
                    error_code = "ROUTE_REJECTED"
                    phase = "ROUTE"
                    step_failed = True
                    break

                # Execute phase
                trace.emit(RunState.EXECUTE.value, step_id=step.id, attempt=attempt)
                start = time.monotonic()
                err = self._executor.execute(step, attempt)
                duration_ms = int((time.monotonic() - start) * 1000)

                if err:
                    trace.emit(RunState.EXECUTE.value, step_id=step.id, attempt=attempt, outcome="FAILURE", duration_ms=duration_ms, details=err)
                    # P1: use operation risk class, not hardcoded non_idempotent
                    op_risk = _op_risk_class(step.operation)
                    elapsed_ms = int((run_deadline - time.monotonic()) * 1000 * -1)
                    req = RecoveryRequest(
                        step_id=step.id, attempt=attempt,
                        risk_class=op_risk,
                        error_category=ErrorCategory.UNKNOWN,
                        error_message=str(err)[:200],
                        elapsed_ms=elapsed_ms,
                        has_rollback=step.rollback is not None,
                    )
                    decision = decide_recovery(req, self._recovery_policy)
                    trace.emit("RECOVERY_DECIDED", step_id=step.id, attempt=attempt,
                               details=decision.to_trace_details())

                    if decision.action in (RecoveryAction.ROLLBACK, RecoveryAction.RETRY):
                        if step.rollback:
                            start = time.monotonic()
                            rec_err = self._executor.recover(step, attempt, dict(step.rollback))
                            rb_dur = int((time.monotonic() - start) * 1000)
                            if rec_err:
                                trace.emit(RunState.RECOVER.value, step_id=step.id, attempt=attempt,
                                           outcome="FAILURE", duration_ms=rb_dur, details=rec_err)
                                trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                           details={"result": "rollback_failed_terminal"})
                                state = RunState.FAILED
                                failed_step_id = step.id
                                error_code = "MANUAL_INTERVENTION_REQUIRED"
                                phase = "RECOVER"
                                step_failed = True
                                break
                        trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                   details={"result": "ok"})
                        continue
                    elif decision.action == RecoveryAction.FALLBACK:
                        trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                   details={"result": "fallback"})
                        continue
                    elif decision.action == RecoveryAction.MANUAL:
                        trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                   details={"result": "manual_required"})
                        state = RunState.FAILED
                        failed_step_id = step.id
                        error_code = "MANUAL_INTERVENTION_REQUIRED"
                        phase = "RECOVER"
                        step_failed = True
                        break
                    else:  # ABORT
                        state = RunState.FAILED
                        failed_step_id = step.id
                        error_code = ERROR_EXECUTE
                        phase = "EXECUTE"
                        step_failed = True
                        break
                else:
                    trace.emit(RunState.EXECUTE.value, step_id=step.id, attempt=attempt, outcome="SUCCESS", duration_ms=duration_ms)

                    # Verify phase (same attempt) — VerifierResult contract
                    trace.emit(RunState.VERIFY.value, step_id=step.id, attempt=attempt)
                    start = time.monotonic()
                    vresult_raw = self._executor.verify(step, attempt)
                    duration_ms = int((time.monotonic() - start) * 1000)

                    # Adapt legacy dict return to VerifierResult if needed
                    if isinstance(vresult_raw, dict) or vresult_raw is None:
                        predicate = dict(step.verifier).get("predicate", "unknown")
                        expected = dict(step.verifier).get("expected")
                        vresult = from_legacy_error(predicate, expected, vresult_raw,
                                                    step_id=step.id, attempt=attempt)
                    else:
                        vresult = vresult_raw

                    # Emit structured verify result to trace
                    trace.emit(RunState.VERIFY.value, step_id=step.id, attempt=attempt,
                               outcome=vresult.status.value.upper(),
                               duration_ms=duration_ms,
                               details=vresult.to_trace_details())

                    if vresult.status == VerifyStatus.PASSED:
                        step_succeeded = True
                        step_index += 1
                        break
                    elif vresult.status == VerifyStatus.SKIPPED:
                        # Policy decides: skipped does not fail the run by default.
                        # Future: consult skip_policy. For now, treat as success.
                        step_succeeded = True
                        step_index += 1
                        break
                    elif vresult.status == VerifyStatus.UNAVAILABLE:
                        # P1 fix: use operation risk class, not hardcoded read_only
                        op_risk = _op_risk_class(step.operation)
                        elapsed_ms = int((run_deadline - time.monotonic()) * 1000 * -1)
                        req = RecoveryRequest(
                            step_id=step.id, attempt=attempt,
                            risk_class=op_risk,
                            error_category=ErrorCategory.VERIFY_UNAVAILABLE,
                            elapsed_ms=elapsed_ms,
                            has_rollback=step.rollback is not None,
                        )
                        decision = decide_recovery(req, self._recovery_policy)
                        trace.emit("RECOVERY_DECIDED", step_id=step.id, attempt=attempt,
                                   details=decision.to_trace_details())

                        if decision.action == RecoveryAction.MANUAL:
                            trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                       details={"result": "manual_required"})
                            state = RunState.FAILED
                            failed_step_id = step.id
                            error_code = "MANUAL_INTERVENTION_REQUIRED"
                            phase = "RECOVER"
                            step_failed = True
                            break
                        elif decision.action in (RecoveryAction.ROLLBACK, RecoveryAction.RETRY):
                            if step.rollback:
                                start = time.monotonic()
                                rec_err = self._executor.recover(step, attempt, dict(step.rollback))
                                rb_dur = int((time.monotonic() - start) * 1000)
                                if rec_err:
                                    trace.emit(RunState.RECOVER.value, step_id=step.id, attempt=attempt,
                                               outcome="FAILURE", duration_ms=rb_dur, details=rec_err)
                                    trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                               details={"result": "rollback_failed_terminal"})
                                    state = RunState.FAILED
                                    failed_step_id = step.id
                                    error_code = "MANUAL_INTERVENTION_REQUIRED"
                                    phase = "RECOVER"
                                    step_failed = True
                                    break
                            trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                       details={"result": "ok"})
                        else:
                            state = RunState.FAILED
                            failed_step_id = step.id
                            error_code = ERROR_VERIFY_UNAVAILABLE
                            phase = "VERIFY"
                            step_failed = True
                            break
                    else:  # FAILED
                        op_risk = _op_risk_class(step.operation)
                        elapsed_ms = int((run_deadline - time.monotonic()) * 1000 * -1)
                        req = RecoveryRequest(
                            step_id=step.id, attempt=attempt,
                            risk_class=op_risk,
                            error_category=ErrorCategory.VERIFY_FAILED,
                            elapsed_ms=elapsed_ms,
                            has_rollback=step.rollback is not None,
                        )
                        decision = decide_recovery(req, self._recovery_policy)
                        trace.emit("RECOVERY_DECIDED", step_id=step.id, attempt=attempt,
                                   details=decision.to_trace_details())

                        if decision.action == RecoveryAction.MANUAL:
                            trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                       details={"result": "manual_required"})
                            state = RunState.FAILED
                            failed_step_id = step.id
                            error_code = "MANUAL_INTERVENTION_REQUIRED"
                            phase = "RECOVER"
                            step_failed = True
                            break
                        elif decision.action in (RecoveryAction.ROLLBACK, RecoveryAction.RETRY):
                            if step.rollback:
                                start = time.monotonic()
                                rec_err = self._executor.recover(step, attempt, dict(step.rollback))
                                rb_dur = int((time.monotonic() - start) * 1000)
                                if rec_err:
                                    trace.emit(RunState.RECOVER.value, step_id=step.id, attempt=attempt,
                                               outcome="FAILURE", duration_ms=rb_dur, details=rec_err)
                                    trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                               details={"result": "rollback_failed_terminal"})
                                    state = RunState.FAILED
                                    failed_step_id = step.id
                                    error_code = "MANUAL_INTERVENTION_REQUIRED"
                                    phase = "RECOVER"
                                    step_failed = True
                                    break
                            trace.emit("RECOVERY_APPLIED", step_id=step.id, attempt=attempt,
                                       details={"result": "ok"})
                            if attempt >= max_retries:
                                state = RunState.FAILED
                                failed_step_id = step.id
                                error_code = ERROR_VERIFY
                                phase = "VERIFY"
                                step_failed = True
                                break
                        else:
                            state = RunState.FAILED
                            failed_step_id = step.id
                            error_code = ERROR_VERIFY
                            phase = "VERIFY"
                            step_failed = True
                            break

            # If we exhausted all attempts without success and didn't fail explicitly
            if not step_succeeded and not step_failed:
                state = RunState.FAILED
                failed_step_id = step.id
                error_code = ERROR_EXECUTE
                phase = "EXECUTE"

            if step_failed:
                break

        # Emit terminal event before closing trace
        if state == RunState.PASSED:
            trace.emit(RunState.PASSED.value, outcome="SUCCESS")
        elif state == RunState.FAILED:
            trace.emit(RunState.FAILED.value, step_id=failed_step_id)

        trace.close()

        summary = RunSummary(
            passed=(state == RunState.PASSED),
            failed_step_id=failed_step_id,
            error_code=error_code,
            phase=phase,
        )

        self._write_summary(output_dir, scenario.task_id, summary.passed, summary.failed_step_id, summary.error_code, summary.phase)
        return summary

    def _write_summary(self, output_dir: Path, task_id: str, passed: bool, failed_step: Optional[str], error_code: Optional[str], phase: Optional[str]) -> None:
        summary_path = output_dir / "summary.json"
        tmp_summary = output_dir / ".summary.json.tmp"
        with open(tmp_summary, "w", encoding="utf-8") as f:
            json.dump({
                "status": "passed" if passed else "failed",
                "task_id": task_id,
                "failed_step": failed_step,
                "error_code": error_code,
                "phase": phase,
            }, f, separators=(",", ":"))
        tmp_summary.replace(summary_path)
