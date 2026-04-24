---
name: skill-review
description: Audit a Claude Code skill file against the official skills specification. Use when the user asks to review, audit, or check a skill for spec compliance, or when a skill file may be too long, have wrong frontmatter, or needs structural improvement.
argument-hint: [skill name or path, e.g. "veriloga" or ".claude/skills/veriloga/SKILL.md"]
allowed-tools: Read Bash(wc *) Bash(find *) Bash(ls *)
---

# Claude Code Skill Spec Auditor

Audit the skill named "$ARGUMENTS". If no argument was given, ask the user which skill to review.

## Step 1 — Locate the skill file

```bash
find .claude/skills ~/.claude/skills -name "SKILL.md" 2>/dev/null | head -30
wc -l <path/to/SKILL.md>
ls -la <skill-dir>/
```

Read the SKILL.md file in full.

## Step 2 — Checklist (score PASS / WARN / FAIL)

### A. File size

| Check | Limit | Notes |
|-------|-------|-------|
| SKILL.md line count | ≤ 500 lines | Excess is silently truncated before Claude sees it |
| reference.md exists when needed | — | Required if SKILL.md would overflow |

Module templates, long examples, and reference tables belong in `reference.md`, not the main file.

### B. Frontmatter fields

| Field | Required | Constraint |
|-------|----------|------------|
| `name` | Yes | Must match the slash-command name exactly |
| `description` | Yes | ≤ 1,536 chars; drives auto-trigger matching |
| `allowed-tools` | Recommended | `ToolName` or `ToolName(glob)`; glob uses bare `*`, not `*/prefix/` |
| `argument-hint` | Recommended | Short hint shown in autocomplete UI |

**`allowed-tools` patterns**:
```
CORRECT: Bash(virtuoso *)   Bash(vcli *)   Read   Write
WRONG:   Bash(*/virtuoso *) — leading */ has unclear glob semantics
WRONG:   Bash               — no glob = allows ALL bash commands (too broad)
```

### C. Skill body

| Check | What to look for |
|-------|-----------------|
| Arguments routing | Body references the ARGUMENTS variable to route on user input |
| Skill-dir-relative paths | Companion files use the CLAUDE_SKILL_DIR variable, not hardcoded absolute paths |
| Dynamic blocks | Time-sensitive content (git branch, date, env vars) injected at load time |
| No hardcoded paths | Absolute paths to project dirs should not appear in skill body |

### D. Content quality

| Check | Guidance |
|-------|----------|
| Templates vs reference.md | Multiple long code blocks (>30 lines each) → extract to `reference.md` |
| No commented-out code | Clean, actionable content only |
| Runnable examples | Snippets work as-is or with minimal substitution |

## Step 3 — Report

Produce a findings table:

```
| # | Check             | Status | Finding                                           |
|---|-------------------|--------|---------------------------------------------------|
| 1 | File size         | FAIL   | 792 lines — move templates to reference.md        |
| 2 | allowed-tools     | WARN   | Bash(*/vcli *) — remove leading */                |
| 3 | argument-hint     | FAIL   | Missing from frontmatter                          |
| 4 | Arguments routing | WARN   | ARGUMENTS variable not used — skill ignores input |
```

List recommended fixes in priority order (FAIL first, then WARN).

## Step 4 — Offer to fix

Ask: "Apply all fixes now?" If yes:
1. Extract overflow content to `reference.md` in the same directory
2. Fix frontmatter: `allowed-tools`, `argument-hint`, `name`
3. Add arguments routing block and a load instruction pointing to `reference.md`

## Spec reference (invariants)

- SKILL.md > 500 lines: content past line 500 is dropped silently
- `description` is used for semantic matching — keep it focused, under ~1,000 chars
- `allowed-tools` controls which tools Claude may call during skill execution
- The ARGUMENTS variable = everything the user typed after the slash command name
- The CLAUDE_SKILL_DIR variable = absolute path to the skill's directory (currently: ${CLAUDE_SKILL_DIR})
