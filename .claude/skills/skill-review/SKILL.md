---
name: skill-review
description: Audit a Claude Code skill file against the official skills specification. Use when the user asks to review, audit, or check a skill for spec compliance, or when a skill file may be too long, have wrong frontmatter, or needs structural improvement.
argument-hint: [path to skill file or skill name, e.g. "veriloga" or ".claude/skills/veriloga/SKILL.md"]
allowed-tools: Read Bash(wc *) Bash(find *) Bash(ls *)
---

# Claude Code Skill Spec Auditor

Audit a skill at `$ARGUMENTS`. If no path given, ask the user which skill to review.

## Step 1 — Locate the skill file

```bash
# If $ARGUMENTS is a skill name, find it:
find .claude/skills/ -name "SKILL.md" | grep -i "$ARGUMENTS"
# Or check global skills:
find ~/.claude/skills/ -name "SKILL.md" | grep -i "$ARGUMENTS"
```

Read the SKILL.md file. Then count lines:

```bash
wc -l <path/to/SKILL.md>
```

Also list companion files:

```bash
ls -la $(dirname <path/to/SKILL.md>)/
```

## Step 2 — Run the checklist

Score each item PASS / WARN / FAIL with a one-line reason.

### A. File size

| Check | Limit | Notes |
|-------|-------|-------|
| `SKILL.md` line count | ≤ 500 lines | Excess goes to `reference.md` in same dir |
| `reference.md` exists if needed | — | Required when SKILL.md overflows |

**Rule**: Content past line 500 is truncated before Claude sees it. Module templates, long examples, and reference tables belong in `reference.md`.

### B. Frontmatter fields

Required fields and constraints:

| Field | Required | Constraint |
|-------|----------|------------|
| `name` | Yes | Matches the slash-command name exactly |
| `description` | Yes | ≤ 1,536 chars combined with `when_to_use`; drives auto-trigger matching |
| `allowed-tools` | Recommended | Syntax: `ToolName` or `ToolName(glob_pattern)`; glob uses `*` not `*/prefix/` |
| `argument-hint` | Recommended | Short hint shown in autocomplete UI (e.g. `[module type or error message]`) |

**`allowed-tools` patterns — correct vs wrong**:
```
# CORRECT
Bash(virtuoso *)     ← bare command glob
Bash(vcli *)
Read
Write

# WRONG
Bash(*/virtuoso *)   ← leading */ has unclear semantics, avoid
Bash                 ← no glob = allows ALL bash commands (too broad)
```

### C. Skill body

| Check | Requirement |
|-------|-------------|
| `$ARGUMENTS` referenced | Skill should route based on user-provided argument |
| `${CLAUDE_SKILL_DIR}` used for file refs | Use instead of hardcoded paths to companion files |
| Dynamic execution blocks | Inline shell results (git branch, date, env vars) injected before skill loads |
| No hardcoded absolute paths | Use `${CLAUDE_SKILL_DIR}` or relative refs |

### D. Content quality

| Check | Guidance |
|-------|----------|
| Templates in body vs reference.md | Long code blocks (>30 lines each, multiple templates) → reference.md |
| No commented-out code | Clean, actionable content only |
| Examples are complete and runnable | Snippets should work as-is or with minimal substitution |

## Step 3 — Report findings

Format as a table:

```
| # | Check | Status | Finding |
|---|-------|--------|---------|
| 1 | File size | FAIL | 792 lines (limit 500) — move templates to reference.md |
| 2 | allowed-tools glob | WARN | Bash(*/vcli *) — remove leading */ |
| 3 | argument-hint | FAIL | Missing — add argument-hint to frontmatter |
| 4 | $ARGUMENTS routing | WARN | Not referenced in body — skill ignores user arguments |
```

Then list **recommended fixes** in priority order (FAIL first, then WARN).

## Step 4 — Offer to fix

Ask: "Apply all fixes now?" If yes, implement them:
1. Create/update `reference.md` for overflow content
2. Edit frontmatter in `SKILL.md`
3. Add `$ARGUMENTS` routing block and `${CLAUDE_SKILL_DIR}/reference.md` load instruction

## Official spec reference

The full Claude Code skills specification is in the Claude Code documentation.
Key invariants:
- Skills are loaded into context as plain text before Claude responds
- `SKILL.md` > 500 lines → content after line 500 is silently dropped
- `description` is used for semantic matching when user types a slash command — keep it focused
- `allowed-tools` controls which tools Claude can call while executing the skill
- `$ARGUMENTS` = everything the user typed after the slash command name
- `${CLAUDE_SKILL_DIR}` = absolute path to the directory containing the skill's SKILL.md
