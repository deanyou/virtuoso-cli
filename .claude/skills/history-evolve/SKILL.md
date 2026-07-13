---
name: history-evolve
description: |
  Evolve skills from vcli session history. Use when:
  (1) user says "根據歷史進化技能" / "evolve skills from history" / "覆盤技能",
  (2) after a debugging session to capture what went wrong,
  (3) periodic skill maintenance to close knowledge gaps revealed by real usage.
  Reads cmd.jsonl + per-session SKILL logs, finds failure/correction/gap signals,
  maps them to skills in .claude/skills/, and writes concrete improvements.
author: Claude Code
version: 1.0.0
date: 2026-05-02
argument-hint: [optional: session ID or date range, e.g. "meowu-meow-38371" or "last 7 days"]
allowed-tools: Bash(cat *) Bash(ls *) Bash(jq *) Bash(grep *) Bash(find *) Read Write Edit
---

# History Evolve

Mine vcli session history for failure and correction signals, then write targeted
improvements into the relevant skills.

---

## Phase 1 — Collect

### 1a. Load CLI history (filter test noise)

```bash
HIST=~/.cache/virtuoso_bridge/history/cmd.jsonl
# Real sessions: <hostname>-<user>-<port> (no "rt-" prefix)
jq -c 'select(.session == null or (.session | test("^rt-") | not))' "$HIST" 2>/dev/null \
  | tail -200
```

Apply a time window if the user specified one (e.g. "last 7 days"):
```bash
SINCE=$(date -d '7 days ago' --iso-8601=seconds 2>/dev/null || date -v-7d +%Y-%m-%dT%H:%M:%S)
jq -c --arg since "$SINCE" 'select(.ts >= $since) | select(.session == null or (.session | test("^rt-") | not))' "$HIST"
```

### 1b. Load SKILL history for active sessions

```bash
ls ~/.cache/virtuoso_bridge/history/*.jsonl 2>/dev/null | grep -v cmd.jsonl | while read f; do
  SESSION=$(basename "$f" .jsonl)
  echo "=== $SESSION ==="
  jq -c '.' "$f" | tail -50
done
```

---

## Phase 2 — Signal Detection

Scan the collected entries and classify each anomaly into one of four signal types:

### Signal A — Hard Failure (`exit_code != 0`)

```bash
jq -c 'select(.exit_code != 0)' "$HIST" \
  | jq -r '"\(.ts) [\(.exit_code)] \(.cmd | join(" "))"'
```

For each failure, note:
- The vcli subcommand (2nd/3rd token after `vcli`)
- The error output (re-run the command or read the last SKILL entry near that timestamp)

### Signal B — SKILL nil return (`ok == false`)

```bash
for f in ~/.cache/virtuoso_bridge/history/*.jsonl; do
  [[ "$f" == *cmd.jsonl ]] && continue
  jq -c 'select(.ok == false)' "$f" 2>/dev/null | while read line; do
    echo "$(basename $f .jsonl): $line"
  done
done
```

For each nil return, note the SKILL expression and what state was likely missing.

### Signal C — Correction Pattern

Look for the same subcommand appearing 2–3 times in quick succession (within ~2 min),
where early attempts fail and the last succeeds:
```bash
jq -r '"\(.ts[:16]) \(.exit_code) \(.cmd[1:3] | join(" "))"' "$HIST" \
  | sort | uniq -c | sort -rn | head -20
```

A `vcli skill exec` block with `exit_code 1` followed by `exit_code 0` for the same
session is a strong correction signal.

### Signal D — Command-Not-Found (subcommand typo / wrong noun)

```bash
# exit_code 2 = clap argument error (unrecognized subcommand / missing arg)
jq -c 'select(.exit_code == 2)' "$HIST" \
  | jq -r '"\(.ts) \(.cmd | join(" "))"'
```

These indicate the user tried a subcommand that doesn't exist — the relevant skill
may need to list valid alternatives or show the `--help` output.

---

## Phase 3 — Map Signals to Skills

For each signal, identify the relevant skill using this table:

| vcli subcommand | Primary skill | Secondary skill |
|-----------------|---------------|-----------------|
| `skill exec` | `skill-exec` | `skill-shell-gotchas` |
| `sim run/setup/measure` | `sim-run` / `sim-setup` / `sim-measure` | `ocean-netlist-regen` |
| `maestro *` | `maestro` | — |
| `session list/current/cleanup/history` | (no dedicated skill — check CLAUDE.md) | — |
| `tunnel start/stop` | `tunnel-connect` | — |
| `cell open/close` | `cell-explore` | — |
| `schematic *` | `schematic-gen` | — |
| `design size/explore` | `gm-over-id` | — |
| Nil SKILL return involving `run()` | `ocean-netlist-regen` | — |
| Nil SKILL return involving `geGetEditCellView()` | `skill-exec` | `cell-explore` |
| Nil SKILL return involving `maeGetSession` / `asiGetSession` | `maestro` | — |

---

## Phase 4 — Read Target Skills

For each signal-to-skill mapping, read the full skill content:
```bash
cat /home/meow/git/virtuoso-cli/.claude/skills/<skill-name>/SKILL.md
```

Identify the specific section to improve:
- **Missing prerequisite** → add to the Prerequisites section
- **Common mistake** → add to a Gotchas / ⚠️ section
- **Missing example** → add a concrete `vcli` command example
- **Wrong subcommand used** → add a "See also" or redirect note

---

## Phase 5 — Write Improvements

For each identified improvement, make a **surgical edit** (do not rewrite the whole skill):

1. State the signal that motivated the change (one line comment for yourself)
2. Add the minimum text that prevents recurrence:
   - A gotcha box: `> ⚠️ **Gotcha**: <what goes wrong and why>`
   - A working example with the corrected command
   - A prerequisite line if state is missing
3. Bump the `version` field in the frontmatter (patch: `1.2.3 → 1.2.4`)
4. Update the `date` field to today

After writing: confirm with the user before touching skills that have significant
user-visible behavior (e.g. `sim-run`, `ocean-netlist-regen`).

---

## Phase 6 — Pensieve Sync (if new reusable insight emerged)

If a signal reveals something not yet in `.pensieve/`:

```bash
# Knowledge: objective fact about system behavior
mkdir -p /home/meow/git/virtuoso-cli/.pensieve/knowledge/<name>
# write content.md following the knowledge format

# Then sync state:
PENSIEVE_SKILL_ROOT="$HOME/.claude/skills/pensieve" \
  bash "$HOME/.claude/skills/pensieve/.src/scripts/maintain-project-state.sh" \
  --event self-improve --note "<one-line description>"
```

---

## Signal Priority

When multiple signals exist, process in this order:

1. **Signal D** (wrong subcommand / exit 2) — highest confusion cost, cheapest fix
2. **Signal A** (hard failure / exit 1) — often a missing prerequisite in the skill
3. **Signal B** (nil return) — SKILL gotchas that prevent silent failures
4. **Signal C** (correction pattern) — reveals unclear examples or parameter names

---

## Output Format

After completing, report:
```
Signals found: <count>
Skills updated: <list>
Pensieve entries written: <list or "none">
```

If no actionable signals were found: state so explicitly and suggest running after
more actual usage (not just connection tests).
