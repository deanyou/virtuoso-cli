# Claude Code Skill Body: Variable Expansion is Global

## Source
Session 2026-04-21: authoring skill-review/SKILL.md; checklist rows corrupted
after the skill loaded because `$ARGUMENTS` and `${CLAUDE_SKILL_DIR}` were
mentioned by name inside table cells and backtick spans.

## Summary
`$ARGUMENTS` and `${CLAUDE_SKILL_DIR}` are expanded everywhere in a SKILL.md
body — code fences, table cells, backtick spans — before Claude sees the content.
There is no escape mechanism.

## Content

### What expands

| Variable | Expands to |
|----------|-----------|
| `$ARGUMENTS` | Everything the user typed after the slash-command name |
| `${CLAUDE_SKILL_DIR}` | Absolute path to the skill's directory |

Both are replaced textually in the **entire skill body** at load time, including:
- Inside ` ``` ` code fences
- Inside `\`inline code\`` spans
- Inside Markdown table cells
- In plain prose

### Symptom of accidental expansion

A skill that says (in a documentation table):

```
| Body references the ARGUMENTS variable to route |
```

will render as (when the skill is triggered with no argument):

```
| Body references the  variable to route |
```

…because `$ARGUMENTS` is replaced with the empty string.

Similarly, a table cell referencing `${CLAUDE_SKILL_DIR}` will show the
actual resolved path, not the variable name.

### Rules when writing skills

1. **Intentional use**: put `$ARGUMENTS` and `${CLAUDE_SKILL_DIR}` exactly where
   you want expansion — routing `if` blocks, `Read` instructions, file paths.

2. **Documentation/examples**: describe the variables in plain English instead
   of mentioning the literal variable name, e.g. "the ARGUMENTS variable" or
   "the skill directory variable".

3. **Dynamic blocks**: `` !`cmd` `` patterns anywhere in the body are executed
   as shell commands on load. Never use backtick-bang syntax except intentionally.

### Correct pattern

```markdown
**If the user named a module type**, read the reference file:
Read ${CLAUDE_SKILL_DIR}/reference.md
```

### Anti-pattern (will corrupt table)

```markdown
| Arguments routing | Body references the $ARGUMENTS variable |
```

## When to Use
- Authoring or editing any SKILL.md file
- Debugging corrupted skill output or unexpected empty strings in skill body
- Running skill-review audits

## Context Links
- Leads to: [[skill-review]] — the skill that audits for this and other spec issues
