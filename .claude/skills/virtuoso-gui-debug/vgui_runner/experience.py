"""Experience Compiler — pure function that turns Runner artifacts into
structured experience_events and experience_cases.

Fast loop: Runner runs → verifies → compiles → stores. Never modifies Skills.
Slow loop (offline): analyze → candidate → sandbox → verify → promote.

Provenance levels:
  OBSERVED          — program directly saw it (exit_code, window_exists)
  RULE_INFERRED     — Router/Compiler judged (risk_class, failure_type)
  VERIFIER_CONFIRMED — VerifierResult established (PASS/FAIL/UNKNOWN + evidence)
"""

import json
import hashlib
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


# ── Schema DDL (idempotent, safe to run on existing DB) ──────────────────

SCHEMA = """
CREATE TABLE IF NOT EXISTS experience_events (
    event_id      TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL,
    task_id       TEXT,
    step_id       TEXT,
    attempt       INTEGER DEFAULT 0,
    event_type    TEXT NOT NULL,
    timestamp     TEXT,
    state         TEXT,
    outcome       TEXT,
    channel       TEXT,
    risk_class    TEXT,
    failure_type  TEXT,
    action        TEXT,
    duration_ms   INTEGER,
    value_source  TEXT DEFAULT 'OBSERVED',
    evidence_refs TEXT,
    details       TEXT
);

CREATE TABLE IF NOT EXISTS experience_cases (
    case_id            TEXT PRIMARY KEY,
    run_id             TEXT NOT NULL,
    task_id            TEXT,
    case_type          TEXT NOT NULL,
    failure_type       TEXT,
    problem_signature  TEXT,
    initial_state      TEXT,
    action_sequence    TEXT,
    final_state        TEXT,
    outcome            TEXT,
    verification_status TEXT,
    quality_score      REAL DEFAULT 0.0,
    event_refs         TEXT,
    provenance         TEXT,
    created_at         TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_events_run ON experience_events(run_id);
CREATE INDEX IF NOT EXISTS idx_events_step ON experience_events(step_id);
CREATE INDEX IF NOT EXISTS idx_events_failure ON experience_events(failure_type);
CREATE INDEX IF NOT EXISTS idx_cases_type ON experience_cases(case_type);
CREATE INDEX IF NOT EXISTS idx_cases_failure ON experience_cases(failure_type);
"""


def init_schema(conn) -> None:
    conn.executescript(SCHEMA)
    conn.commit()


# ── Helpers ─────────────────────────────────────────────────────────────

def _sha256(*parts: str) -> str:
    h = hashlib.sha256()
    for p in parts:
        h.update(p.encode("utf-8"))
        h.update(b"\0")
    return h.hexdigest()[:32]


def _redact(d: Any) -> Any:
    """Recursively redact sensitive keys from a dict/list."""
    SENSITIVE = {"token", "password", "secret", "key", "credential",
                 "private_key", "env", "api_key", "ssh_key"}
    if isinstance(d, dict):
        out = {}
        for k, v in d.items():
            if any(s in k.lower() for s in SENSITIVE):
                out[k] = "<redacted>"
            else:
                out[k] = _redact(v)
        return out
    if isinstance(d, list):
        return [_redact(x) for x in d]
    if isinstance(d, tuple):
        return [_redact(x) for x in d]
    return d


# ── Experience Compiler ────────────────────────────────────────────────

def compile_run(output_dir: Path, run_id: str, task_id: str,
               virtuoso_version: str = "unknown") -> Tuple[List[Dict], Optional[Dict]]:
    """Compile Runner output artifacts into experience events and one case.

    Pure function: reads trace.jsonl + summary.json, returns (events, case).
    Does NOT write to DB. Caller inserts idempotently.

    Gate A (determinism): same artifacts → same events/case.
    Gate B (provenance): every event traces to trace line, case traces to events.
    """
    trace_path = output_dir / "agent-actions.jsonl"
    summary_path = output_dir / "summary.json"

    events: List[Dict] = []
    raw_lines: List[dict] = []

    if trace_path.exists():
        with open(trace_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if line:
                    raw_lines.append(json.loads(line))

    summary = {}
    if summary_path.exists():
        with open(summary_path, "r", encoding="utf-8") as f:
            summary = json.load(f)

    # ── Classify each trace line into an experience event ──
    route_channel = None
    risk_class = None
    verifier_status = None
    recovery_actions: List[Dict] = []
    step_outcomes: Dict[str, Dict] = {}

    for line in raw_lines:
        seq = line.get("seq", 0)
        state = line.get("state", "")
        step_id = line.get("step_id")
        attempt = line.get("attempt", 0)
        outcome = line.get("outcome")
        details = line.get("details") or {}
        duration = line.get("duration_ms")

        # Determine value_source
        value_source = "OBSERVED"
        if state in ("VERIFY",) and outcome in ("PASSED", "FAILED"):
            value_source = "VERIFIER_CONFIRMED"
        elif state in ("ROUTE_DECIDED", "ROUTE_PROVENANCE"):
            value_source = "RULE_INFERRED"
        elif state == "RECOVERY_DECIDED":
            value_source = "RULE_INFERRED"

        # Extract channel/risk from ROUTE_DECIDED
        if state == "ROUTE_DECIDED":
            route_channel = details.get("channel")
            risk_class = details.get("risk_class")
            if details.get("rejected"):
                value_source = "VERIFIER_CONFIRMED"  # Router has truth here

        # Extract verifier status
        if state == "VERIFY" and outcome:
            verifier_status = outcome.upper()

        # Track recovery actions
        if state == "RECOVERY_DECIDED":
            recovery_actions.append({
                "step_id": step_id,
                "attempt": attempt,
                "action": details.get("action"),
                "cause": details.get("cause"),
            })

        # Track step outcomes
        if step_id and state in ("EXECUTE", "VERIFY", "RECOVER"):
            if step_id not in step_outcomes:
                step_outcomes[step_id] = {}
            step_outcomes[step_id][state.lower()] = outcome

        # Build event
        event = {
            "event_id": _sha256(run_id, step_id or "", str(attempt), state, str(seq)),
            "run_id": run_id,
            "task_id": task_id,
            "step_id": step_id,
            "attempt": attempt,
            "event_type": state,
            "timestamp": line.get("ts"),
            "state": state,
            "outcome": outcome,
            "channel": route_channel,
            "risk_class": risk_class,
            "failure_type": details.get("error_code") or details.get("reason_code"),
            "action": details.get("result") or details.get("action"),
            "duration_ms": duration,
            "value_source": value_source,
            "evidence_refs": json.dumps([f"trace#{seq}"]),
            "details": json.dumps(_redact(details)),
        }
        events.append(event)

    # ── Build one case per run ──
    case = None
    if events:
        run_passed = summary.get("status") == "passed"
        failed_step = summary.get("failed_step")
        error_code = summary.get("error_code")
        phase = summary.get("phase")

        # Determine case_type
        if run_passed and not recovery_actions:
            case_type = "success"
        elif recovery_actions:
            case_type = "recovery"
        elif not run_passed:
            case_type = "failed"
        else:
            case_type = "unknown"

        # Verification status
        if verifier_status in ("PASSED", "FAILED", "UNAVAILABLE"):
            verification_status = verifier_status
        else:
            verification_status = None

        # Problem signature
        failure_type = None
        if not run_passed:
            failure_type = error_code or phase or "unknown_failure"

        # Quality score: verified > recovery > failed > unknown
        quality = 0.0
        if verification_status == "PASSED":
            quality = 1.0
        elif verification_status == "FAILED":
            quality = 0.7
        elif recovery_actions:
            quality = 0.5
        elif not run_passed:
            quality = 0.3

        event_ids = [e["event_id"] for e in events]

        case = {
            "case_id": _sha256(run_id, case_type, str(verification_status or "")),
            "run_id": run_id,
            "task_id": task_id,
            "case_type": case_type,
            "failure_type": failure_type,
            "problem_signature": f"virtuoso:gui:{case_type}:{verification_status or 'unknown'}",
            "initial_state": json.dumps({"run_status": "started"}),
            "action_sequence": json.dumps([
                {"step_id": s, "outcomes": o} for s, o in step_outcomes.items()
            ]),
            "final_state": json.dumps({"status": summary.get("status"),
                                       "error_code": error_code,
                                       "failed_step": failed_step}),
            "outcome": "passed" if run_passed else "failed",
            "verification_status": verification_status,
            "quality_score": quality,
            "event_refs": json.dumps(event_ids),
            "provenance": json.dumps({
                "compiled_at": "deterministic",
                "virtuoso_version": virtuoso_version,
                "event_count": len(events),
                "value_sources": list(set(e["value_source"] for e in events)),
            }),
        }

    return events, case


# ── Idempotent write ────────────────────────────────────────────────────

def write_experience(conn, events: List[Dict], case: Optional[Dict]) -> Tuple[int, int]:
    """Insert events and case idempotently. Returns (events_inserted, cases_inserted)."""
    before_e = conn.execute("SELECT COUNT(*) FROM experience_events").fetchone()[0]
    before_c = conn.execute("SELECT COUNT(*) FROM experience_cases").fetchone()[0]

    for e in events:
        try:
            conn.execute("""
                INSERT OR IGNORE INTO experience_events
                (event_id, run_id, task_id, step_id, attempt, event_type, timestamp,
                 state, outcome, channel, risk_class, failure_type, action, duration_ms,
                 value_source, evidence_refs, details)
                VALUES (:event_id, :run_id, :task_id, :step_id, :attempt, :event_type,
                 :timestamp, :state, :outcome, :channel, :risk_class, :failure_type,
                 :action, :duration_ms, :value_source, :evidence_refs, :details)
            """, e)
        except Exception:
            pass

    if case:
        try:
            conn.execute("""
                INSERT OR IGNORE INTO experience_cases
                (case_id, run_id, task_id, case_type, failure_type, problem_signature,
                 initial_state, action_sequence, final_state, outcome, verification_status,
                 quality_score, event_refs, provenance)
                VALUES (:case_id, :run_id, :task_id, :case_type, :failure_type,
                 :problem_signature, :initial_state, :action_sequence, :final_state,
                 :outcome, :verification_status, :quality_score, :event_refs, :provenance)
            """, case)
        except Exception:
            pass

    conn.commit()
    after_e = conn.execute("SELECT COUNT(*) FROM experience_events").fetchone()[0]
    after_c = conn.execute("SELECT COUNT(*) FROM experience_cases").fetchone()[0]
    return after_e - before_e, after_c - before_c
