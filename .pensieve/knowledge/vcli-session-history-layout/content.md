---
id: content
type: knowledge
title: vcli Session History — File Layout and Test Pollution
status: active
created: 2026-05-27
updated: 2026-05-27
tags: ["knowledge"]
---

# vcli Session History — File Layout and Test Pollution

## Source
2026-05-02: Implemented in v0.3.18 (`src/history.rs`). Observed that `cmd.jsonl` contained
150+ lines of test-generated entries mixed with real user commands.

## Summary
Session history uses two files: per-session SKILL log and global CLI log. The global
`cmd.jsonl` is written by `cargo test` as well as real usage — filter by session ID pattern
when analyzing user behavior.

## Content

### File Locations

```
~/.cache/virtuoso_bridge/history/
├── cmd.jsonl                      ← ALL vcli invocations (real + test)
├── <session_id>.jsonl             ← SKILL executions for that session only
└── <session_id>.jsonl             ...
```

### Entry Formats

**SKILL entry** (`<session_id>.jsonl`):
```json
{"ts":"2026-05-02T01:07:19Z","ok":true,"skill":"list(geGetEditCellView()~>...","output":"(\"FT0001A_SH\" ...)"}
```
- `output` is truncated at 512 chars to keep files small

**CLI entry** (`cmd.jsonl`):
```json
{"ts":"2026-05-02T01:07:19Z","session":"meowu-meow-38371","cmd":["vcli","--session","...","skill","exec","..."],"exit_code":0}
```
- `session` is `null` when no session was resolved (e.g. `vcli session list`)

### Test Pollution in cmd.jsonl

`cargo test` calls real `append_cmd()` code paths. Test-generated entries use synthetic
session IDs like `rt-hist-cmd-limit-33333`, `rt-hist-cmd-err-54321`, etc.

**Filter real user commands:**
```bash
# Real sessions follow the pattern: <hostname>-<user>-<port>
cat ~/.cache/virtuoso_bridge/history/cmd.jsonl \
  | jq -r 'select(.session == null or (.session | test("^rt-")|not))'
```

### vcli CLI Commands

```bash
vcli session history <id>               # skill + cmd entries, last 50
vcli session history <id> --skill       # only SKILL log
vcli session history <id> --cmd         # only CLI log
vcli session history <id> --limit 0     # all entries (no limit)
vcli session history <id> --limit 20    # last 20 entries
```

### Multi-Session Context

- `vcli session current` → which session auto-selects; returns `ambiguous` if multiple alive
- `vcli session cleanup` → deletes session files for ports that no longer accept connections
- Explicitly specifying a dead session via `--session` still hits `connection_failed` (bypass of
  auto-filter); auto-filter only works when session is resolved from the registry

## When to Use
- Debugging what SKILL was sent to Virtuoso before a failure
- Auditing which vcli commands an automation script actually ran
- Reviewing user interaction patterns across sessions

## Context Links
- Related: [[vcli-bridge-cli-name]] — session ID format and registry location
