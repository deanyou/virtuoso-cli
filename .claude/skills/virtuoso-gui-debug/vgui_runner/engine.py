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

    def verify(self, step: Step, attempt: int) -> Optional[Dict[str, Any]]:
        if self.outcome(step.id, StepPhase.VERIFY, attempt) == StepOutcome.FAILURE:
            return {"error": f"verify failed for step {step.id}"}
        return None

    def recover(self, step: Step, attempt: int, rollback: Optional[Dict[str, Any]]) -> Optional[Dict[str, Any]]:
        if self.outcome(step.id, StepPhase.RECOVER, attempt) == StepOutcome.FAILURE:
            return {"error": f"recover failed for step {step.id}"}
        return None


class Runner:
    """State machine runner that executes scenarios step-by-step."""

    def __init__(self, executor: Executor):
        self._executor = executor

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

        # Step execution loop
        step_index = 0
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
                # Execute phase
                trace.emit(RunState.EXECUTE.value, step_id=step.id, attempt=attempt)
                start = time.monotonic()
                err = self._executor.execute(step, attempt)
                duration_ms = int((time.monotonic() - start) * 1000)

                if err:
                    trace.emit(RunState.EXECUTE.value, step_id=step.id, attempt=attempt, outcome="FAILURE", duration_ms=duration_ms, details=err)
                    if attempt < max_retries:
                        # More attempts available: recover and try again
                        trace.emit(RunState.RECOVER.value, step_id=step.id, attempt=attempt)
                        start = time.monotonic()
                        rec_err = self._executor.recover(step, attempt, dict(step.rollback) if step.rollback else None)
                        duration_ms = int((time.monotonic() - start) * 1000)
                        if rec_err:
                            trace.emit(RunState.RECOVER.value, step_id=step.id, attempt=attempt, outcome="FAILURE", duration_ms=duration_ms, details=rec_err)
                            state = RunState.FAILED
                            failed_step_id = step.id
                            error_code = ERROR_RECOVER
                            phase = "RECOVER"
                            step_failed = True
                            break
                        else:
                            trace.emit(RunState.RECOVER.value, step_id=step.id, attempt=attempt, outcome="SUCCESS", duration_ms=duration_ms)
                            # continue to next attempt
                    else:
                        # No more attempts
                        state = RunState.FAILED
                        failed_step_id = step.id
                        error_code = ERROR_EXECUTE
                        phase = "EXECUTE"
                        step_failed = True
                        break
                else:
                    trace.emit(RunState.EXECUTE.value, step_id=step.id, attempt=attempt, outcome="SUCCESS", duration_ms=duration_ms)

                    # Verify phase (same attempt)
                    trace.emit(RunState.VERIFY.value, step_id=step.id, attempt=attempt)
                    start = time.monotonic()
                    err = self._executor.verify(step, attempt)
                    duration_ms = int((time.monotonic() - start) * 1000)

                    if err:
                        trace.emit(RunState.VERIFY.value, step_id=step.id, attempt=attempt, outcome="FAILURE", duration_ms=duration_ms, details=err)
                        if step.rollback and attempt < max_retries:
                            trace.emit(RunState.RECOVER.value, step_id=step.id, attempt=attempt, outcome="ROLLED_BACK")
                            start = time.monotonic()
                            rec_err = self._executor.recover(step, attempt, dict(step.rollback) if step.rollback else None)
                            duration_ms = int((time.monotonic() - start) * 1000)
                            if rec_err:
                                trace.emit(RunState.RECOVER.value, step_id=step.id, attempt=attempt, outcome="FAILURE", duration_ms=duration_ms, details=rec_err)
                                state = RunState.FAILED
                                failed_step_id = step.id
                                error_code = ERROR_ROLLBACK
                                phase = "RECOVER"
                                step_failed = True
                                break
                            else:
                                trace.emit(RunState.RECOVER.value, step_id=step.id, attempt=attempt, outcome="SUCCESS", duration_ms=duration_ms)
                                # continue to next attempt
                        else:
                            state = RunState.FAILED
                            failed_step_id = step.id
                            error_code = ERROR_VERIFY
                            phase = "VERIFY"
                            step_failed = True
                            break
                    else:
                        trace.emit(RunState.VERIFY.value, step_id=step.id, attempt=attempt, outcome="SUCCESS", duration_ms=duration_ms)
                        # Both execute and verify succeeded for this attempt
                        step_succeeded = True
                        step_index += 1
                        break

            # If we exhausted all attempts without success and didn't fail explicitly
            if not step_succeeded and not step_failed:
                # This happens when for loop completes but no explicit success/failure break
                # Only possible if max_retries=0 and first execute succeeds but verify fails without rollback
                # Or if all attempts exhausted with recover success (which shouldn't reach here if logic is correct)
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
