# Virtuoso GUI Runner Foundation Implementation Plan

> **For agentic workers:** Implement this plan task-by-task with an independent review gate after each task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the safe, deterministic foundation for replaying validated Virtuoso GUI-debug scenarios without yet invoking a live `vcli`, X11 server, or `xdotool`.

**Architecture:** A project skill at `.claude/skills/virtuoso-gui-debug/` documents when and how an agent may use the runner. A Python 3.9+ standard-library package parses a JSON scenario into immutable typed values, rejects operations outside the DSL allowlist, executes one step at a time through an injected executor, and writes append-only JSONL events plus a final JSON summary. The first increment ships only a fake executor so parsing, state transitions, retry bounds, and evidence recording are testable offline.

**Tech Stack:** Python 3.9+ standard library (`argparse`, `dataclasses`, `enum`, `json`, `pathlib`, `tempfile`, `unittest`); Markdown Agent Skill.

**Spec:** `/Users/dean/Library/Containers/com.tencent.xinWeChat/Data/Documents/xwechat_files/wxid_5qr489ei4qkj22_4a32/temp/RWTemp/2026-09/9e20f478899dc29eb19741386f9343c8/virtuoso-agent-vcli-xdotool-gui-debug-plan.md`

## Global Constraints

- Place all Agent Skill and Python runner files under `.claude/skills/virtuoso-gui-debug/`.
- Accept JSON scenarios only in this increment; do not add PyYAML or another dependency.
- Allow only `VCLI_LOAD`, `VCLI_CALL`, `WINDOW_WAIT`, `WINDOW_ACTIVATE`, `KEY`, `TYPE`, `CLICK_REL`, `DRAG_REL`, `SCREENSHOT`, `VERIFY`, and `RECOVER`.
- Reject unknown fields, unknown operations, empty `session_id`, non-positive PID, invalid `DISPLAY`, empty cellView components, timeouts outside 1–300 seconds, retries outside 0–1, and actions without a verifier.
- Never execute arbitrary shell, raw SKILL, `vcli`, X11, or `xdotool` in this increment.
- Every run writes `task.json`, `agent-actions.jsonl`, and `summary.json` below a caller-selected output directory.
- Never log environment variables, credentials, license paths, or arbitrary process environment.

---

### Task 1: Skill entrypoint and scenario contract

**Files:**
- Create: `.claude/skills/virtuoso-gui-debug/SKILL.md`
- Create: `.claude/skills/virtuoso-gui-debug/references/scenario-schema.md`

**Interfaces:**
- Consumes: the approved GUI-debug design and existing `vcli` command conventions.
- Produces: the invocation contract for `scripts/gui_runner.py` and the exact JSON schema used by Task 2.

- [ ] **Step 1: Write the skill entrypoint**

Use frontmatter name `virtuoso-gui-debug`, a discriminating description for replayable Virtuoso GUI debugging, and `allowed-tools: Bash(python3 *) Read`. Require an explicit session/PID/DISPLAY/test cellView binding, database-first verification, a unique output directory, and a dry-run before a future live run. State that this foundation supports `--executor fake` only and must not be represented as live GUI automation.

- [ ] **Step 2: Write the schema reference**

Define the top-level fields `version`, `task_id`, `session_id`, `pid`, `display`, `cellview`, and `steps`. Define each step as `id`, `operation`, `arguments`, `verifier`, `timeout_seconds`, `max_retries`, and optional `rollback`. Include one complete valid JSON example and a table of per-operation argument keys. Explicitly state that extra fields are rejected.

- [ ] **Step 3: Validate the skill**

Run:

```bash
python3 /Users/dean/.codex/skills/.system/skill-creator/scripts/quick_validate.py .claude/skills/virtuoso-gui-debug
```

Expected: validation succeeds with no unfinished scaffold placeholders.

### Task 2: Strict scenario model and parser

**Files:**
- Create: `.claude/skills/virtuoso-gui-debug/scripts/vgui_runner/__init__.py`
- Create: `.claude/skills/virtuoso-gui-debug/scripts/vgui_runner/model.py`
- Create: `.claude/skills/virtuoso-gui-debug/tests/test_model.py`

**Interfaces:**
- Consumes: the JSON contract in `references/scenario-schema.md`.
- Produces: `Scenario.from_dict(value: object) -> Scenario`, `Scenario.load(path: Path) -> Scenario`, `Operation`, `Step`, `CellView`, and `ScenarioValidationError`.

- [ ] **Step 1: Write failing parser tests**

Cover a complete valid scenario, each unknown top-level/step/argument field, an unknown operation, duplicate step IDs, missing verifier, invalid session/PID/DISPLAY/cellView, timeout bounds, retry bounds, invalid argument types, and operation-specific required/allowed arguments.

- [ ] **Step 2: Run parser tests and confirm failure**

Run:

```bash
python3 -m unittest discover -s .claude/skills/virtuoso-gui-debug/tests -p 'test_model.py' -v
```

Expected: import failure because `vgui_runner.model` does not exist.

- [ ] **Step 3: Implement immutable typed parsing**

Use frozen dataclasses. Parse JSON primitives explicitly rather than coercing values. Centralize exact-key checks and operation argument schemas. Normalize nothing except preserving the supplied JSON values; invalid values must raise `ScenarioValidationError` with a JSON-path-like location such as `steps[1].timeout_seconds`.

- [ ] **Step 4: Run parser tests**

Expected: all parser tests pass.

### Task 3: State machine, fake executor, and evidence trace

**Files:**
- Create: `.claude/skills/virtuoso-gui-debug/scripts/vgui_runner/engine.py`
- Create: `.claude/skills/virtuoso-gui-debug/scripts/vgui_runner/trace.py`
- Create: `.claude/skills/virtuoso-gui-debug/tests/test_engine.py`

**Interfaces:**
- Consumes: `Scenario` and `Step` from Task 2.
- Produces: `RunState` (`PRECHECK`, `BASELINE`, `EXECUTE`, `VERIFY`, `RECOVER`, `PASSED`, `FAILED`), `StepOutcome`, `Executor` protocol, `FakeExecutor`, `Runner.run(scenario, output_dir) -> RunSummary`, and append-only event records.

- [ ] **Step 1: Write failing engine tests**

Cover the happy-path transition sequence; precheck failure; action failure followed by one successful retry; verifier failure followed by rollback and retry; exhausted retry; rollback failure; no retry when `max_retries` is zero; atomic creation of `task.json`; ordered JSONL sequence numbers; and a final summary that names the failed step and machine-readable error code.

- [ ] **Step 2: Run engine tests and confirm failure**

Run:

```bash
python3 -m unittest discover -s .claude/skills/virtuoso-gui-debug/tests -p 'test_engine.py' -v
```

Expected: import failure because the engine does not exist.

- [ ] **Step 3: Implement the minimal engine**

The executor interface must expose `precheck(scenario)`, `baseline(scenario)`, `execute(step)`, `verify(step)`, and `recover(step, rollback)`. `FakeExecutor` consumes scripted outcomes from test data and performs no subprocess or environment access. Emit an event before and after every executor call with monotonic sequence, UTC timestamp, state, step ID, attempt, outcome, duration in milliseconds, and sanitized details. Retry at most once because the parser rejects larger values.

- [ ] **Step 4: Implement deterministic evidence output**

Create the run directory with `exist_ok=False`. Write the validated scenario to `task.json` using a temporary sibling plus `Path.replace`. Flush each JSONL event immediately. Write `summary.json` atomically after reaching `PASSED` or `FAILED`.

- [ ] **Step 5: Run engine tests**

Expected: all model and engine tests pass.

### Task 4: CLI wrapper and offline acceptance tests

**Files:**
- Create: `.claude/skills/virtuoso-gui-debug/scripts/gui_runner.py`
- Create: `.claude/skills/virtuoso-gui-debug/tests/fixtures/pass.json`
- Create: `.claude/skills/virtuoso-gui-debug/tests/fixtures/fail.json`
- Create: `.claude/skills/virtuoso-gui-debug/tests/test_cli.py`

**Interfaces:**
- Consumes: `Scenario.load`, `FakeExecutor`, and `Runner.run`.
- Produces: `python3 scripts/gui_runner.py validate SCENARIO` and `python3 scripts/gui_runner.py run SCENARIO --output DIR --executor fake`.

- [ ] **Step 1: Write failing CLI tests**

Use `subprocess.run` with a sanitized minimal environment. Assert validate success, validation exit code `2`, fake pass exit code `0`, fake scenario failure exit code `1`, refusal of any executor other than `fake`, machine-readable JSON on stdout, and no files written outside the requested output directory.

- [ ] **Step 2: Run CLI tests and confirm failure**

Run:

```bash
python3 -m unittest discover -s .claude/skills/virtuoso-gui-debug/tests -p 'test_cli.py' -v
```

Expected: failure because `gui_runner.py` does not exist.

- [ ] **Step 3: Implement the CLI**

Resolve imports relative to the script directory without installing a package. Print one compact JSON object to stdout. Send human-readable diagnostics to stderr. Map validation/usage errors to exit `2`, scenario failure to exit `1`, and pass to exit `0`. Do not read `.env` or inherit behavior from repository configuration.

- [ ] **Step 4: Run all offline tests and syntax checks**

Run:

```bash
python3 -m unittest discover -s .claude/skills/virtuoso-gui-debug/tests -v
python3 -m py_compile .claude/skills/virtuoso-gui-debug/scripts/gui_runner.py .claude/skills/virtuoso-gui-debug/scripts/vgui_runner/model.py .claude/skills/virtuoso-gui-debug/scripts/vgui_runner/engine.py .claude/skills/virtuoso-gui-debug/scripts/vgui_runner/trace.py
python3 /Users/dean/.codex/skills/.system/skill-creator/scripts/quick_validate.py .claude/skills/virtuoso-gui-debug
```

Expected: all tests pass, all modules compile, and the skill validates.

## Self-review

- Spec coverage for this subproject: DSL allowlist, explicit binding, state machine, bounded retry/recovery, audit log, structured result, and skill placement are covered.
- Deferred deliberately: real `vcli`, SKILL adapter, X11/xdotool driver, Display lock, screenshots, live recovery, and PVT/PDK integration. Each requires a later independently reviewed increment.
- No third-party dependency or raw execution escape hatch is introduced.
