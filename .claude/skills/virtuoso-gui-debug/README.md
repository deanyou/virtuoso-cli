# virtuoso-gui-debug

GUI debugging and automation skill for Virtuoso. Uses X11 (xdotool/vcli action-x11) and SKILL API to interact with Cadence Virtuoso remotely.

## Quick Start

```bash
# Verify session is alive
vcli --session <id> skill exec "version()"

# Run a scenario
python3 scripts/gui_runner.py run scenario.json --output /tmp/out --executor live --session <id>

# Offline test (no Virtuoso needed)
python3 scripts/gui_runner.py run scenario.json --output /tmp/out --executor fake
```

## Executors

| Executor | Purpose |
|----------|---------|
| `fake` | Offline deterministic replay for CI/regression |
| `live` | Real vcli over SSH, drives remote Virtuoso |
| `local` | Direct xdotool on local X11 DISPLAY |

## RSI Knowledge Base

Recursive Self-Improvement: persistent SQLite database of discovered SKILL functions.

### Structure

```
data/
  skill_db.sqlite3      # Main database (source of truth)
  skill_db_backup.json  # JSON backup (git diff friendly)
scripts/
  skill_db.py           # DB management
  enumerate_functions.py # Pattern-based discovery
  probe_signatures.py   # Extract signatures from error messages
  verify_db.py          # Data integrity check
  generate_report.py    # HTML report generator
report/
  rsi_report.html       # Latest status report
```

### Usage

```bash
# Verify database integrity
python3 scripts/verify_db.py

# Query known functions
sqlite3 data/skill_db.sqlite3 \
  "SELECT signature FROM functions WHERE name='dbCreateRect' AND confidence='verified'"

# Search by pattern
sqlite3 data/skill_db.sqlite3 \
  "SELECT name, signature FROM functions WHERE name LIKE 'dbCreate%'"
```

### Confidence Levels

| Level | Meaning | When to use |
|-------|---------|-------------|
| `verified` | Actual call succeeded, signature confirmed | Production use |
| `discovered` | Function exists, signature inferred from errors | Experimental |

### Versioning

Functions are tagged by Virtuoso version. Current database: **IC251** on host `192.168.1.111`.

When testing on a different version:
1. Run enumeration with `--version IC231` (or whatever)
2. Same function name may have different signature
3. Database supports multiple versions side-by-side

## Key Pitfalls

1. SQLite reserved word `exists` → use `func_exists`
2. Python 3.6: no `capture_output`, use `stdout=PIPE`
3. vcli errors in `errors` array, not `output`
4. Motif/Xt apps ignore XTest input events
5. F1 opens Find/Replace, not Help
6. layer+purpose must be `list("layer" "purpose")`
7. `let` requires double parens: `let(((v e)) body)`
8. Zero-area rect silently returns nil
9. Dead session hangs — check port alive first

## Testing

```bash
python3 -m unittest discover tests
```

## References

- [Scenario Schema](references/scenario-schema.md)
- [xdotool Cheatsheet](references/xdotool-cheatsheet.md)
