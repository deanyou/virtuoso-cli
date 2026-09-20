# VCLI-GUI Architecture v2.1 — Implementation & Evolution Playbook

> **What this is**: How to use the architecture today, and what evidence triggers what change.
> **What this is NOT**: A development roadmap. No scheduled P1.7/P1.8/P2.

## Two Governance Principles

```
Learn Above, Freeze Below    — Skill/Knowledge layer learns; Runtime/Executor stays deterministic
No Evidence, No Architecture Change — Nothing changes without repeated, material real-task evidence
```

---

## 1. Current Baseline & Entry Points

The following is production-ready. No "implement v2.1" needed.

```
SKILL.md (contract)
  → RSI retrieval / experience lookup
  → Router (pure decision)
  → Executor (fake / live / local)
  → VerifierResult (passed / failed / skipped / unavailable)
  → Evidence manifest + Recovery (bounded)
```

**Entry points**:
- Normal task: `gui_runner.py --executor live`
- Knowledge lookup: `rsi_query.py` / `experience_retrieval.py`
- Observation: RSI HTML report Experience tab
- Regression: `tests/test_*.py`

---

## 2. Real Task Run SOP

Each real Virtuoso task produces:

| Artifact | Content |
|---|---|
| `agent-actions.jsonl` | Append-only event trace |
| `manifest.json` | Run index, status, artifact hashes |
| `summary.json` | Final status, error code, phase |
| `experience_events` | Atomic events (STARTED/EXECUTE/VERIFY) |
| `experience_cases` | Classified case (GOLD/NEGATIVE/UNKNOWN) |

**Outcome handling**:
- `PASSED`: Normal completion, evidence committed
- `FAILED`: Verifier-confirmed postcondition failure → NEGATIVE pool
- `UNAVAILABLE` / `UNKNOWN`: Cannot confirm result → UNKNOWN pool, **no auto-retry**
- `MANUAL_INTERVENTION_REQUIRED`: Terminal, human takes over

**What needs human review**:
- UNKNOWN on destructive operation
- Recovery fallback to a different channel
- Rollback failure
- Window identity mismatch

---

## 3. Natural Production Learning SOP

Knowledge accumulates **only from real design tasks**.

**Do NOT**:
- Batch-run read-only screenshots to inflate case count
- Inject synthetic failures into production runs
- Promote a single observation to a rule

**Do**:
- On retrieval miss/irrelevant: record `query`, `expected`, actual `top-5`
- On new failure_type: add to taxonomy with evidence
- On reusable recovery: enter candidate pool (not production)
- On every run: evidence closure checked automatically

---

## 4. Trigger Matrix

| Real Evidence | Allowed Action | NOT Allowed |
|---|---|---|
| Retrieval miss / irrelevant | Improve aliases / filters / ranking | Introduce vector RAG immediately |
| New failure_type appears | Extend taxonomy with evidence | Refactor Runtime |
| CapabilitySnapshot state expression friction | Evaluate P1.7 State Promotion | Build RuntimeState preemptively |
| Many mature, verified candidates | Evaluate P1.8 Controlled Evolution | Auto-modify production Skill |
| SKILL.md bloats / retrieval degrades | Structured Skill Package split | Split files for architectural aesthetics |
| SKILL/shortcut/X11 all fail on real gap | Evaluate P2 Vision | Vision-first default |
| Recovery pattern verified on real write ops | Expand recovery knowledge | Infer write-op safety from read-only cases |

---

## 5. Change Gate / Definition of Done

```
Real Task
   ↓
Evidence produced
   ↓
Repeated / Material Problem?
   │
   ├── NO  →  Keep Frozen, continue normal use
   │
   └── YES
         ↓
   Minimal Change Proposal (smallest possible)
         ↓
   Regression suite passes
         ↓
   Real / Representative validation
         ↓
   Evidence comparison (before vs after)
         │
         ├── No improvement → Reject, document why
         │
         └── Improvement confirmed
               ↓
            Promote
               ↓
            Update Architecture Baseline (this document)
```

**Minimum requirements for any change**:
- Reproducible evidence (not a single anecdote)
- Regression test added
- No risk_class / permission / verifier / retry_budget relaxation without human approval
- Evidence manifest still closes
- No SSH connection storm

---

## Out of Scope (until triggered)

- vcli batch multi-op API
- Additional Planner templates
- Write / destructive operation acceptance
- Windows live executor
- Vector / hybrid retrieval
- RuntimeState first-class object
- Controlled Evolution pipeline
- Vision grounding