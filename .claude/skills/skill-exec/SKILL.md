---
name: skill-exec
description: Execute SKILL code on Virtuoso. Use when running SKILL expressions, querying cellview data, listing libraries/cells, or interacting with Virtuoso programmatically.
argument-hint: [SKILL expression to run]
allowed-tools: Bash(vcli *) Bash(virtuoso *)
---

# Execute SKILL Code on Virtuoso

Run SKILL expressions via `vcli skill exec` and parse results.

## Quick reference

```bash
# Arithmetic
vcli skill exec "1+2" --format json

# String operations
vcli skill exec 'strcat("hello" " " "world")' --format json

# List all libraries
vcli skill exec 'foreach(mapcar lib ddGetLibList() lib~>name)' --format json

# List cells in a library
vcli skill exec 'let((lib) lib=ddGetObj("myLib") foreach(mapcar c lib~>cells c~>name))' --format json

# Get cell views
vcli skill exec 'let((cell) cell=ddGetObj("myLib" "myCell") foreach(mapcar v cell~>views v~>name))' --format json

# Current cellview info
vcli skill exec 'let((cv) cv=geGetEditCellView() list(cv~>libName cv~>cellName cv~>viewName))' --format json

# Instance count
vcli skill exec 'let((cv) cv=geGetEditCellView() length(cv~>instances))' --format json

# Net names
vcli skill exec 'let((cv result n) cv=geGetEditCellView() result=nil n=0 foreach(net cv~>nets when(n<20 result=cons(net~>name result) n=n+1)) result)' --format json

# Schematic read (read-only)
vcli skill exec 'let((cv) cv=dbOpenCellViewByType("lib" "cell" "schematic" nil "r") sprintf(nil "inst=%d nets=%d" length(cv~>instances) length(cv~>nets)))' --format json
```

## Multi-session usage

When multiple Virtuoso instances are running, specify the session explicitly:

```bash
vcli session list                                          # find alive session IDs
vcli --session meowu-meow-38371 skill exec 'getCurrentTime()'
export VB_SESSION=meowu-meow-38371                        # or set once for the shell
vcli skill exec 'getCurrentTime()'
```

## Connection failure recovery

> ⚠️ **Gotcha**: `--session <id>` bypasses auto-filtering. If Virtuoso restarts, the
> session ID changes (new port → new ID). Explicitly specifying a dead session returns
> `connection_failed: Connection refused` (exit 1) even though other sessions are alive.

Recovery pattern (observed signal: exit 1 on `skill exec` followed by `session list`):

```bash
# 1. Stale session detected
vcli --session meowu-meow-32987 skill exec 'getCurrentTime()'
# → connection_failed: Connection refused (exit 1)

# 2. Purge dead session files and find new IDs
vcli session cleanup
vcli session list

# 3. Retry with the new session ID
vcli --session meowu-meow-38371 skill exec 'getCurrentTime()'
```

## Multi-line input — `skill eval --stdin`

For snippets that span more than one line, or that contain characters painful to
shell-quote (double quotes, parens, dollar signs), pipe the SKILL through stdin
instead of stuffing it on the argv:

```bash
# argv form — fine for one-liners
vcli skill exec '1+2' --format json

# stdin form — survives any quoting problem; here-doc friendly
cat <<'EOF' | vcli skill eval --stdin --format json
let((x y)
    x = 1 + 2
    y = x * x
    y
)
EOF
```

Both `skill exec` and `skill eval` go through the daemon, but they differ in two
ways that matter:

| | `skill exec` | `skill eval` |
|---|---|---|
| Input | argv only (`<code>` positional) | argv OR `--stdin` |
| Multi-statement | daemon `let((r) r=<code>)` — **single form only** | wrapped in `progn(\n<code>\n)` — **any number of forms** |
| Trailing `; comment` | safe | safe (the `\n)` before `)` terminates the line comment) |
| Return value | the daemon's `r` | the value of the **last form** inside `progn` |

**Rule of thumb**: use `eval --stdin` whenever your SKILL has more than one top-level
form, contains full-line comments, or defines a procedure then calls it.

> ⚠️ **Gotcha — `skill exec` silently drops earlier forms.** Inside the daemon's
> `let(((__vb_r <code>)) ...)`, `f1() f2()` parses as "apply the result of `f1()` to
> the arguments of `f2()`" — only `f2()` runs. If you see `printf(...)` succeed but
> the side effect you expected from the prior statement is missing, switch to
> `skill eval --stdin` so the input is wrapped in `progn(...)`.

## CIW output vs return value

`vcli skill exec` (and `eval`) send the SKILL expression to Virtuoso for evaluation
and capture the **return value** back into the JSON `output` field. They do **not**
echo anything to the CIW window unless your expression does it itself with `printf`.

```bash
# Pure return value — CIW stays silent; vcli stdout gets "3"
vcli skill exec '1+2' --format json
# → {"status":"success","output":"3","errors":[],"warnings":[],...}
# CIW: (nothing)

# Both — printf shows in CIW, the let body's final form is the return value
vcli skill exec 'let((v) v=1+2 printf("1+2 = %d\n" v) v)' --format json
# → {"status":"success","output":"3",...}
# CIW: 1+2 = 3
```

When debugging a long expression, the `printf(...)` + final-form pattern is the
cleanest way to inspect intermediate state without leaving CIW trace breadcrumbs
that pollute later reads.

## Attribution

The multi-line SKILL wrapping (`progn(\n<code>\n)`), `--stdin` input mode, and
the CIW output vs return-value distinction are adapted from
[virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite)
(MIT, 2026-06) — specifically `cli.py::cli_eval`,
`examples/01_virtuoso/basic/05_multiline_skill.py`, and
`examples/01_virtuoso/basic/00_ciw_output_vs_return.py`.

## Important notes

- Use `--format json` for structured output (auto in pipe mode)
- Use `--timeout N` for long-running operations (default 30s)
- SKILL strings use `"`, escape with `\"` inside bash single quotes
- `let` blocks work for local variables; Ocean functions (simulator, design, run) must be at top level
- View names may not be standard `schematic` — check with `v~>name` not `v~>viewName`
- Always wrap slot access in `let`: `let((cv) cv=geGetEditCellView() cv~>cellName)` not `geGetEditCellView()~>cellName` — the latter crashes if no design is open
