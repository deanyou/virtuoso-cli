#!/usr/bin/env python3
"""gui_runner.py - CLI for Virtuoso GUI Runner (offline-only foundation).

Every invocation outcome (including validation, usage, and runtime errors)
prints exactly one compact machine-readable JSON object to stdout.
Optionally, human-readable diagnostics may also be written to stderr.
Exit codes:
  0 - PASSED (scenario completed successfully)
  1 - FAILED (scenario step failed after all retries)
  2 - validation/usage/runtime error
"""
from __future__ import annotations

import argparse
import json
import sys
import traceback
from pathlib import Path

# Resolve imports relative to script directory
_SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(_SCRIPT_DIR))

from vgui_runner.model import Scenario, ScenarioValidationError  # noqa: E402
from vgui_runner.engine import FakeExecutor, Runner, StepOutcome  # noqa: E402
from vgui_runner.command_runner import CommandError, LocalRunner, SshRunner  # noqa: E402
from vgui_runner.live_executor import LiveExecutor  # noqa: E402
from vgui_runner.local_executor import LocalExecutor  # noqa: E402


def emit_json(payload: dict) -> None:
    """Emit one compact JSON object to stdout."""
    sys.stdout.write(json.dumps(payload, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def parse_fake_outcomes(path: Path) -> dict:
    """Parse fake outcomes JSON file into FakeExecutor outcomes dict."""
    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)

    if not isinstance(data, list):
        raise ValueError("fake-outcomes must be a JSON array")

    outcomes = {}
    for item in data:
        if not isinstance(item, dict):
            raise ValueError("each outcome must be an object")
        step_id = item.get("step_id")
        phase = item.get("phase")
        attempt = item.get("attempt")
        outcome = item.get("outcome")

        if not isinstance(step_id, str):
            raise ValueError("step_id must be a string")
        if phase not in ("precheck", "baseline", "execute", "verify", "recover"):
            raise ValueError(f"phase must be one of precheck/baseline/execute/verify/recover, got {phase}")
        if not isinstance(attempt, int) or isinstance(attempt, bool) or attempt < 0:
            raise ValueError("attempt must be a nonnegative integer")
        if outcome not in ("SUCCESS", "FAILURE"):
            raise ValueError(f"outcome must be SUCCESS or FAILURE, got {outcome}")

        key = (step_id, phase, attempt)
        outcomes[key] = StepOutcome(outcome)

    return outcomes


def cmd_validate(scenario_path: Path) -> int:
    """Validate a scenario file. Returns 0 on success, 2 on error."""
    try:
        scenario = Scenario.load(scenario_path)
        emit_json({
            "status": "valid",
            "task_id": scenario.task_id,
            "session_id": scenario.session_id,
            "pid": scenario.pid,
            "display": scenario.display,
            "cellview": str(scenario.cellview),
            "steps": len(scenario.steps),
        })
        return 0
    except ScenarioValidationError as e:
        emit_json({"status": "invalid", "error": str(e), "path": e.path})
        return 2
    except Exception as e:
        emit_json({"status": "error", "error": f"unexpected error: {type(e).__name__}: {e}"})
        return 2


def build_executor(executor_name, session_id, vcli_path, ssh_host, output_dir, fake_outcomes_path, window_id=None):
    """Construct the executor named by --executor. Returns (executor, error)."""
    if executor_name == "fake":
        if fake_outcomes_path is not None:
            try:
                outcomes = parse_fake_outcomes(fake_outcomes_path)
            except Exception as e:  # noqa: BLE001
                return None, {"status": "error", "error": f"invalid fake-outcomes: {e}"}
            return FakeExecutor(outcomes), None
        return FakeExecutor(), None
    if executor_name == "live":
        if not session_id:
            return None, {
                "status": "error",
                "error": "--executor live requires --session",
            }
        if not vcli_path:
            return None, {
                "status": "error",
                "error": "--executor live requires --vcli PATH",
            }
        if not output_dir:
            return None, {
                "status": "error",
                "error": "--executor live requires --output DIR",
            }
        try:
            if ssh_host:
                runner = SshRunner(vcli_path=vcli_path, ssh_host=ssh_host)
            else:
                runner = LocalRunner(vcli_path=vcli_path)
            executor = LiveExecutor(
                command_runner=runner,
                vcli_path=vcli_path,
                ssh_host=ssh_host,
                session_id=session_id,
                output_dir=output_dir,
            )
            return executor, None
        except (CommandError, ValueError) as e:
            return None, {"status": "error", "error": str(e)}
    if executor_name == "local":
        if not output_dir:
            return None, {
                "status": "error",
                "error": "--executor local requires --output DIR",
            }
        try:
            executor = LocalExecutor(
                display=":0",  # overridden by scenario.display at precheck
                output_dir=output_dir,
                window_id=window_id,
            )
            return executor, None
        except ValueError as e:
            return None, {"status": "error", "error": str(e)}
    return None, {
        "status": "error",
        "error": f"executor '{executor_name}' not supported; use 'fake', 'live', or 'local'",
    }


def cmd_run(scenario_path: Path, output_dir: Path, executor_name: str,
            session_id=None, vcli_path=None, ssh_host=None,
            fake_outcomes_path: Path = None, window_id: str = None) -> int:
    """Run a scenario. Returns 0 on pass, 1 on fail, 2 on error."""
    executor, err = build_executor(
        executor_name, session_id, vcli_path, ssh_host, output_dir, fake_outcomes_path,
        window_id=window_id,
    )
    if executor is None:
        emit_json(err)
        return 2

    try:
        scenario = Scenario.load(scenario_path)
    except ScenarioValidationError as e:
        emit_json({"status": "invalid", "error": str(e), "path": e.path})
        return 2
    except Exception as e:
        emit_json({"status": "error", "error": f"unexpected error: {type(e).__name__}: {e}"})
        return 2

    if executor_name == "live" and scenario.session_id != session_id:
        emit_json({
            "status": "error",
            "error": (
                f"scenario session '{scenario.session_id}' does not match "
                f"CLI session '{session_id}'"
            ),
        })
        return 2

    # LocalExecutor binds DISPLAY from the scenario at precheck time.
    if isinstance(executor, LocalExecutor):
        executor._display = scenario.display
        executor._env["DISPLAY"] = scenario.display

    runner = Runner(executor)

    try:
        summary = runner.run(scenario, output_dir)
    except Exception as e:
        emit_json({"status": "error", "error": f"runtime error: {type(e).__name__}: {e}"})
        return 2
    finally:
        if isinstance(executor, LiveExecutor):
            executor.close()

    emit_json({
        "status": "passed" if summary.passed else "failed",
        "task_id": scenario.task_id,
        "failed_step": summary.failed_step_id,
        "error_code": summary.error_code,
    })

    return 0 if summary.passed else 1


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="gui_runner.py",
        description="Virtuoso GUI Runner (offline-only foundation)",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    val = sub.add_parser("validate", help="Validate a scenario JSON file")
    val.add_argument("scenario", type=Path, help="Path to scenario JSON file")

    run = sub.add_parser("run", help="Run a scenario with fake, live, or local executor")
    run.add_argument("scenario", type=Path, help="Path to scenario JSON file")
    run.add_argument("--output", "-o", type=Path, required=True, help="Output directory")
    run.add_argument("--executor", default="fake", help="Executor name ('fake', 'live', or 'local')")
    run.add_argument("--session", default=None, dest="session_id",
                     help="Virtuoso session id (required with --executor live)")
    run.add_argument("--vcli", default=None, dest="vcli_path",
                     help="Path to the vcli binary (required with --executor live)")
    run.add_argument("--ssh-host", default=None, dest="ssh_host",
                     help="SSH host running vcli (optional with --executor live)")
    run.add_argument("--window-id", default=None, dest="window_id",
                     help="X11 window id (optional with --executor local; overrides PID discovery)")
    run.add_argument("--fake-outcomes", type=Path, default=None,
                     dest="fake_outcomes",
                     help="Path to JSON file with fake outcomes (only with --executor fake)")

    args = parser.parse_args()

    try:
        if args.command == "validate":
            return cmd_validate(args.scenario)
        elif args.command == "run":
            if args.fake_outcomes is not None and args.executor != "fake":
                emit_json({
                    "status": "error",
                    "error": "--fake-outcomes only valid with --executor fake",
                })
                return 2
            return cmd_run(
                args.scenario, args.output, args.executor,
                session_id=args.session_id,
                vcli_path=args.vcli_path,
                ssh_host=args.ssh_host,
                fake_outcomes_path=args.fake_outcomes,
                window_id=args.window_id,
            )
        else:
            parser.print_help()
            emit_json({"status": "error", "error": "unknown command"})
            return 2
    except SystemExit:
        raise
    except Exception as e:
        emit_json({"status": "error", "error": f"unexpected error: {type(e).__name__}: {e}"})
        return 2


if __name__ == "__main__":
    sys.exit(main())