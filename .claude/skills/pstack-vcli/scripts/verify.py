#!/usr/bin/env python3
"""Run fixed vcli verification gates and retain their process evidence.

Python 3.9+, standard library only. A plan is never a verification result.
The smoke scope uses the built binary and isolated vcli runtime directories.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time


RUST_COMMANDS = (
    ("cargo_test", ("cargo", "test")),
    ("cargo_clippy", ("cargo", "clippy", "--", "-D", "warnings")),
    ("cargo_fmt", ("cargo", "fmt", "--check")),
)
SMOKE_LIMITATIONS = [
    "config check is excluded: Config::build_report reads ~/.vcli/.env "
    "without a runtime-path override (src/config.rs).",
    "Live Virtuoso, SSH, Spectre, and GUI operations are not covered.",
]


def utc_now():
    return datetime.now(timezone.utc).isoformat()


def validate_repo(repo):
    for relative in ("Cargo.toml", "src/main.rs", "AGENTS.md"):
        path = repo / relative
        if not path.is_file() or repo not in path.resolve().parents:
            raise ValueError("not a vcli repository: missing or external " + relative)


def make_steps(repo, scope):
    steps = []
    if scope in ("rust", "all"):
        steps.extend({"id": name, "command": list(command), "kind": "rust"}
                     for name, command in RUST_COMMANDS)
    if scope in ("skills", "all"):
        steps.append({"id": "skill_tests", "kind": "skills",
                      "command": ["bash", ".github/scripts/run-skill-tests.sh"]})
    if scope in ("smoke", "all"):
        binary = repo / "target" / "debug" / ("vcli.exe" if os.name == "nt" else "vcli")
        steps.extend([
            {"id": "cargo_build_vcli", "kind": "smoke",
             "command": ["cargo", "build", "--bin", "vcli"]},
            {"id": "vcli_version", "kind": "smoke",
             "command": [str(binary), "--version"], "contract": "version"},
            {"id": "vcli_schema", "kind": "smoke",
             "command": [str(binary), "schema", "skill", "exec", "--format", "json"],
             "contract": "schema"},
            {"id": "vcli_sessions", "kind": "smoke",
             "command": [str(binary), "session", "list", "--format", "json"],
             "contract": "sessions"},
        ])
    for step in steps:
        step["status"] = "planned"
        step["cwd"] = str(repo) if step["kind"] != "skills" else "<temporary-skills-snapshot>"
    return steps


def isolated_smoke_env(repo, runtime):
    env = {key: value for key, value in os.environ.items()
           if not key.upper().startswith(("VB_", "VCLI_"))}
    for name, directory in (
        ("VB_HOME", "home"), ("VB_CACHE_DIR", "cache"),
        ("VB_LOG_DIR", "logs"), ("VB_OUTPUT_DIR", "artifacts"),
        ("VB_TMP_DIR", "tmp"), ("VB_STATE_DIR", "state"),
        ("VB_CONFIG_DIR", "config"),
    ):
        path = runtime / directory
        path.mkdir(parents=True, exist_ok=True)
        env[name] = str(path)
    targets = runtime / "targets.yaml"
    targets.write_text("targets: {}\n", encoding="utf-8")
    env["VB_TARGETS_FILE"] = str(targets)
    # An explicit profile bypasses ~/.vcli/profile and virtualenv bindings.
    env["VB_PROFILE"] = "pstack-smoke"
    # Override a caller's target directory so the next commands use this build.
    env["CARGO_TARGET_DIR"] = str(repo / "target")
    env.pop("CARGO_BUILD_TARGET", None)
    return env


def snapshot_skills(repo, destination):
    source = repo / ".claude" / "skills"
    script = repo / ".github" / "scripts" / "run-skill-tests.sh"
    if not source.is_dir() or not script.is_file():
        raise ValueError("skill test sources are missing")
    if source.is_symlink() or repo not in source.resolve().parents:
        raise ValueError("skill sources must be contained in the repository")
    # A retained symlink could redirect writes back into the user's checkout.
    # Refuse such a snapshot rather than claiming it is isolated.
    for root, directories, files in os.walk(source, followlinks=False):
        for name in directories + files:
            if (Path(root) / name).is_symlink():
                raise ValueError("skill snapshot refuses symlinks: " + str(Path(root) / name))
    if script.is_symlink() or repo not in script.resolve().parents:
        raise ValueError("skill test runner must be contained in the repository")
    (destination / ".github" / "scripts").mkdir(parents=True)
    shutil.copy2(script, destination / ".github" / "scripts" / script.name)
    shutil.copytree(source, destination / ".claude" / "skills",
                    ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))


def validate_contract(contract, stdout):
    if contract == "version":
        fields = stdout.strip().split()
        if len(fields) != 2 or fields[0] != "vcli" or not fields[1][0:1].isdigit():
            raise ValueError("unexpected vcli version output")
        return
    payload = json.loads(stdout)
    if not isinstance(payload, dict):
        raise ValueError("expected a JSON object")
    if contract == "schema":
        command = payload.get("skill exec")
        arguments = command.get("args") if isinstance(command, dict) else None
        code = arguments.get("code") if isinstance(arguments, dict) else None
        if not isinstance(code, dict) or code.get("required") is not True or code.get("type") != "string":
            raise ValueError("schema does not describe a required string code argument")
    elif contract == "sessions":
        if (payload.get("status") != "success" or payload.get("sync_status") != "ok"
                or type(payload.get("count")) is not int or payload["count"] != 0
                or payload.get("sessions") != []):
            raise ValueError("isolated session list must contain zero sessions and successful sync")


def terminate_process(process):
    if os.name == "posix":
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    elif os.name == "nt":
        # taskkill is a standard Windows command. No shell interpolation.
        try:
            subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           timeout=5, check=False)
        except (OSError, subprocess.TimeoutExpired):
            process.kill()
    else:
        process.kill()
    if process.poll() is None:
        process.kill()
    process.wait()


def execute_step(step, cwd, env, timeout, logs):
    step["cwd"] = str(cwd)
    stdout_path = logs / (step["id"] + ".stdout.log")
    stderr_path = logs / (step["id"] + ".stderr.log")
    step.update({"started_at": utc_now(), "stdout_log": str(stdout_path),
                 "stderr_log": str(stderr_path), "returncode": None})
    started = time.monotonic()
    with stdout_path.open("wb") as stdout_file, stderr_path.open("wb") as stderr_file:
        try:
            process = subprocess.Popen(step["command"], cwd=str(cwd), env=env,
                                       stdout=stdout_file, stderr=stderr_file,
                                       stdin=subprocess.DEVNULL,
                                       start_new_session=os.name == "posix")
            try:
                step["returncode"] = process.wait(timeout=timeout)
                step["status"] = "passed" if process.returncode == 0 else "failed"
            except subprocess.TimeoutExpired:
                terminate_process(process)
                step.update({"status": "timeout", "returncode": process.returncode,
                             "error": "subprocess exceeded its timeout"})
            except KeyboardInterrupt:
                terminate_process(process)
                step.update({"status": "interrupted", "returncode": process.returncode,
                             "error": "verification interrupted"})
        except FileNotFoundError as error:
            step.update({"status": "missing_tool", "error": str(error)})
            stderr_file.write(str(error).encode("utf-8", errors="replace"))
        except OSError as error:
            step.update({"status": "failed", "error": str(error)})
            stderr_file.write(str(error).encode("utf-8", errors="replace"))
    step["duration_seconds"] = round(time.monotonic() - started, 6)
    step["finished_at"] = utc_now()
    for channel, path in (("stdout", stdout_path), ("stderr", stderr_path)):
        step[channel + "_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
    if step["status"] == "passed" and step.get("contract"):
        try:
            validate_contract(step["contract"], stdout_path.read_text(encoding="utf-8"))
            step["contract_verified"] = True
        except (ValueError, UnicodeError) as error:
            step.update({"status": "failed", "contract_verified": False,
                         "error": str(error)})


def save_report(report, output):
    temporary = output / "report.json.tmp"
    temporary.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    temporary.replace(output / "report.json")


def positive_timeout(value):
    result = float(value)
    if not math.isfinite(result) or result <= 0:
        raise argparse.ArgumentTypeError("timeout must be a positive finite number")
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[4])
    parser.add_argument("--scope", choices=("rust", "skills", "smoke", "all"), default="rust")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--timeout", type=positive_timeout, default=600.0,
                        help="seconds per subprocess (default: 600)")
    parser.add_argument("--plan", action="store_true", help="print a plan without executing or writing files")
    args = parser.parse_args(argv)
    repo = args.repo.resolve()
    report = {"schema_version": 1, "repo": str(repo), "scope": args.scope,
              "status": "planned", "timeout_seconds": args.timeout,
              "steps": make_steps(repo, args.scope),
              "limitations": SMOKE_LIMITATIONS if args.scope in ("smoke", "all") else []}
    try:
        validate_repo(repo)
        if args.plan:
            print(json.dumps(report, indent=2, ensure_ascii=False))
            return 0
        output = (args.output_dir.resolve() if args.output_dir is not None
                  else Path(tempfile.mkdtemp(prefix="pstack-vcli-verify-")))
        skills_source = (repo / ".claude" / "skills").resolve()
        if args.scope in ("skills", "all") and (
                output == skills_source or skills_source in output.parents):
            raise ValueError("output directory must be outside the skill sources")
        output.mkdir(parents=True, exist_ok=True)
        if any(output.iterdir()):
            raise ValueError("output directory must be empty to preserve earlier evidence")
        logs = output / "logs"
        logs.mkdir()
        report.update({"status": "running", "started_at": utc_now(),
                       "output_dir": str(output), "report_path": str(output / "report.json")})
        save_report(report, output)
        failed = False
        smoke_env = None
        for step in report["steps"]:
            if failed:
                step.update({"status": "skipped", "reason": "an earlier step failed"})
                continue
            cwd, env = repo, None
            try:
                if step["kind"] == "skills":
                    cwd = output / "skills-snapshot"
                    snapshot_skills(repo, cwd)
                    report["skills_snapshot"] = str(cwd)
                elif step["kind"] == "smoke":
                    if smoke_env is None:
                        runtime = output / "smoke-runtime"
                        smoke_env = isolated_smoke_env(repo, runtime)
                        report["smoke_runtime"] = str(runtime)
                    env = smoke_env
                execute_step(step, cwd, env, args.timeout, logs)
            except (OSError, ValueError) as error:
                step.update({"status": "failed", "error": str(error)})
            failed = step["status"] != "passed"
            save_report(report, output)
        report.update({"status": "failed" if failed else "passed", "finished_at": utc_now()})
        save_report(report, output)
        print(json.dumps(report, indent=2, ensure_ascii=False))
        return 1 if failed else 0
    except (OSError, ValueError) as error:
        report.update({"status": "failed", "error": str(error)})
        print(json.dumps(report, indent=2, ensure_ascii=False))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
