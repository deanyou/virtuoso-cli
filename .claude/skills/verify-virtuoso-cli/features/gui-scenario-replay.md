# Replay a GUI debugging scenario

The `virtuoso-gui-debug` skill turns a GUI session into a replayable JSON
scenario. A scenario is validated, executed by one of three executors, and its
every state transition is recorded. Around it sits an evidence loop that records
manual interventions, mines them into candidate behaviors, and records decisions
against those candidates.

## Sub-features

- `scenario-validate` checks a scenario's shape before anything is executed.
- `scenario-fake-run` replays a scenario offline and reports pass or fail.
- `scenario-evidence-files` writes the run's summary, task and action trace.
- `scenario-route-decision` records the channel and risk class chosen per step.
- `scenario-live-run` drives a real Virtuoso over SSH.
- `scenario-local-run` drives a local X11 display.
- `evidence-record` records a manual intervention with a real database row.
- `evidence-list` reads interventions back.
- `evidence-verify` updates an intervention's verification status.
- `evidence-mine` groups interventions into candidate behaviors.
- `evidence-decide` records an adopt or reject decision against a candidate.

## How to get to it (user POV)

- Run `python3 .claude/skills/virtuoso-gui-debug/scripts/gui_runner.py validate <scenario.json>` in a terminal.
- Run `python3 .claude/skills/virtuoso-gui-debug/scripts/gui_runner.py run <scenario.json> --output <dir> --executor fake` to replay a scenario offline.
- Run `python3 .claude/skills/virtuoso-gui-debug/scripts/evidence/record_intervention.py record <run_id> <step_id> --reason "..." --action "..."` to log a manual intervention.
- Run `python3 .claude/skills/virtuoso-gui-debug/scripts/evidence/mine_candidates.py mine` to see the candidate behaviors mined from the log.

## Driving it with gui_runner.py

Preconditions:

- `python3` is available.
- The repo checkout is present; the harness lives in `.claude/skills/virtuoso-gui-debug/`.
- For the `live` executor: a reachable Virtuoso, a matching `--session`, and a `--vcli` path. For the `local` executor: a live X11 `DISPLAY` with `xdotool` installed.

- **Validate before running.** Run `python3 .claude/skills/virtuoso-gui-debug/scripts/gui_runner.py validate .claude/skills/virtuoso-gui-debug/test_scenario.json`. Exit code `0` and stdout has `"status": "valid"`, the `task_id`, the `session_id`, the `cellview` rendered as `lib/cell/view`, and a `steps` count.
- **Replay offline.** Run `python3 .claude/skills/virtuoso-gui-debug/scripts/gui_runner.py run .claude/skills/virtuoso-gui-debug/test_scenario.json --output <evidence>/gui --executor fake`. Exit code `0` and stdout is one compact JSON object with `"status": "passed"`, the `task_id`, and `"failed_step": null`.
- **The summary file.** Read `<evidence>/gui/summary.json`. It has `"status": "passed"` and `"failed_step": null`.
- **The task file.** Read `<evidence>/gui/task.json`. It is the scenario as it was actually executed, with each step's `id`, `operation`, `arguments`, `verifier` and `timeout_seconds`.
- **The action trace.** Read `<evidence>/gui/agent-actions.jsonl`. Every line is a JSON record with `seq`, `ts`, `state`, `step_id`, `attempt`, `outcome` and `duration_ms`. The `state` values walk the run: `PRECHECK`, `BASELINE`, `ROUTE_DECIDED`, `ROUTE_PROVENANCE`, `EXECUTE`, `VERIFY`, then `PASSED`.
- **The route decision is recorded, not assumed.** Find the `ROUTE_DECIDED` records. Each carries `channel` (`vcli_x11`, `skill` or `local_x11`), `risk_class` (`read_only` or `non_idempotent_write`), `confidence`, `reason_code` and a `fallbacks` list. A `VERIFY` record carries `predicate`, `"status": "passed"`, `expected` and `observed`.
- **Evidence loop — record.** Point the database at a scratch file and record an intervention: `VB_EXPERIENCE_DB=<scratch>.db python3 .claude/skills/virtuoso-gui-debug/scripts/evidence/record_intervention.py record <run_id> <step_id> --reason "dialog blocked on confirm" --action "clicked confirm button"`. Exit code `0` and stdout carries the new `intervention_id` plus the recorded `run_id`, `step_id`, `reason` and `action`.
- **Evidence loop — read back.** Run the same script with `list --run-id <run_id>`. Exit code `0` and stdout lists the intervention with its `Reason`, `Action` and a `Status` of `pending`.
- **Evidence loop — verify.** Run `record_intervention.py verify <intervention_id> --status VERIFIED`. Exit code `0` and the intervention's `verification_status` becomes `VERIFIED`.
- **Evidence loop — mine.** Run `mine_candidates.py mine --min-verified 1`. Exit code `0` and stdout has `total_ivs`, `total_cands`, `total_groups` and a `candidates` array. Mining groups by the `(reason, action, context)` signature, so one intervention produces one group and no candidate.
- **Evidence loop — decide.** Run `record_intervention.py adopt <candidate_id> <snapshot_hash> --reason "..."` against a mined candidate. Exit code `0` and the decision is recorded.
- **Side effect.** The scratch database file exists and its `interventions` table has one more row than before the record. The `candidate_decisions` table is untouched by a plain `record`.

## Gotchas

- The `fake` executor is a **replay of the harness**, not a mock of Virtuoso. It proves that the scenario engine routes, executes, verifies and records correctly. It proves nothing about Virtuoso, and a `fake` pass is never a tier-B pass.
- A `fake` run is **not side-effect free**: it writes to the repo's tracked
  `.claude/skills/virtuoso-gui-debug/data/skill_db.sqlite3` (the experience /
  calibration database the router reads). Check `git status` after the run and
  restore that file — a verification run must not leave a tracked file modified.
  The scenario evidence itself goes to `--output`, which you choose.
- A `fake` run emits no screenshot and no image evidence at all. The three files it writes — `summary.json`, `task.json`, `agent-actions.jsonl` — are the whole evidence set.
- In a `fake` run the `VERIFY` record for a `ciw_eval` predicate shows `expected` and `observed` as the string `<mappingproxy>`. That is a Python repr leaking into the trace, not a value comparison. Do not read it as "the expression evaluated to the expected value".
- The `duration_ms` of every `fake` step is `0`. The fake executor does no real waiting, so a timeout in the scenario is never exercised.
- The scenario files checked into the repo (`test_scenario.json`, `test_full.json`) carry a `session_id`, a `pid` and a `display` belonging to the machine that authored them. They are placeholders. The `live` executor refuses a scenario whose `session_id` does not match the `--session` you pass — edit the scenario's `session_id` to match, or write a new scenario.
- `VB_EXPERIENCE_DB` must be set explicitly, or the scripts fall back to an ambient location. Point it at a scratch file so a verification run never touches the real experience database. `--db` works too and takes precedence.
- Mining counts only **evidence-sourced** verification. `record_intervention.py verify --status VERIFIED` sets the status manually and the mined candidate count stays `0` — that is deliberate, and it is the same invariant the repo's own fake regression test pins.
- `record_intervention.py verify --verify-with-evidence` derives the status from recorded verifier events. With no verifier events present it reports `UNKNOWN` and `"derived_from": "no_verifier_events"`, and still mines nothing.
- The evidence scripts belong to the `virtuoso-gui-debug` skill, not to `vcli`. Editing them is out of scope for a vcli verification run; report a defect in them instead.
