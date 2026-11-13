---
name: skill-exec
description: Execute SKILL code on Virtuoso. Use when running SKILL expressions, querying cellview data, listing libraries/cells, or interacting with Virtuoso programmatically.
allowed-tools: Bash(*/vcli *) Bash(*/virtuoso *)
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

## 会话选择与实例核验

### 选哪个 Virtuoso 实例

当同时有多个 Virtuoso 实例在跑时，显式指定：

```bash
vcli session list                                          # 列出 alive 的 session ID
export VB_SESSION=meowu-meow-38371                        # 合法用法：export 设一次，整条 shell 生效
VB_SESSION=meowu-meow-38371 vcli skill exec '1+1'         # 或者用单命令前缀
```

- `export VB_SESSION=...` 是**合法用法**，不是缺陷——它就是官方文档里的写法。
- 禁止 `export` 的是 `VCLI_CAPABILITY=admin`：admin 是提权能力，只用**单命令前缀**，
  不要 export 到后续每一条命令的环境里。

```bash
# ✓ 单命令前缀
VCLI_CAPABILITY=admin vcli skill exec '1+1' --format json
```

### Maestro 子命令的会话是两回事

| 要选的东西 | 用什么 |
|-----------|--------|
| 哪个 Virtuoso 实例（bridge 会话，id 形如 `<host>-<user>-<port>`） | `VB_SESSION`，或全局 `--session` |
| 哪个 Maestro 会话（`fnxSessionN`） | 子命令自己的 `--session` |

- Maestro 子命令的 `--session fnxSession0` **不会**写进 `VB_SESSION`——它是 Maestro
  会话名，不是 bridge 会话 id。
- 全局 `--session` 的值会按形态自动判定：像 bridge 会话 id → 选中该 Virtuoso；
  像 `fnxSession*` → 只当作 Maestro 会话；两者都看不出来 → vcli **直接报错**（exit 2），
  要求你用 `VB_SESSION` 选 Virtuoso、用子命令的 `--session` 选 `fnxSession`，
  **不会**默默连到别的实例。
- 两个 `--session` 同名，**子命令位置的值会覆盖全局位置的值**。需要同时指定时用环境变量：

```bash
VB_SESSION=meowu-meow-38371 vcli maestro run --session fnxSession0
```

### 确认"我连的就是我以为的那个 Virtuoso"

> ⚠️ **新端口 ≠ 独立 Virtuoso。** Virtuoso 重启后端口会变、会话 id 也会变，
> 但同一台机器上可能仍然只有一个 Virtuoso 实例。要判定"是不是同一个 Virtuoso"，
> 交叉核验下面几项。

> ⚠️ **会话注册文件里的 `pid` 恒为 `0`，不是 daemon 的真实 PID。**
> `resources/ramic_bridge.il` 的 `RBWriteSession` 写注册文件时固定写 `"pid":0`
> —— 那是"未知"的占位符，不是进程号。远端同步（`SessionInfo::sync_from_remote`）
> 也是原样拷贝远端的注册文件，所以远程会话同样是 `0`。
> **不要**拿它去比进程、算 PPID、或据此判断会话是否 stale。

真实 PID 只能从**监听端口**反查：

| 核验项 | 怎么看 | 说明 |
|-------|-------|------|
| 监听端口 → 真实 PID | `ss -ltnp \| grep <port>` | 端口属于哪个 pid（这一步拿到的才是真 PID） |
| 该 PID 的 PPID | `ps -o pid,ppid,cmd -p <pid>` | PPID 指向启动 daemon 的父进程（即 Virtuoso） |
| `CDS_MPS_SESSION` | `tr '\0' '\n' < /proc/<ppid>/environ \| grep CDS_MPS_SESSION` | Virtuoso 的 MPS session 名，区分不同 Virtuoso 实例 |
| 注册文件里的 `pid` | `jq -r .pid <sessions_dir>/<id>.json` | **恒为 `0`（未知）**，只能当"没记录"，不能当进程号 |

各项要**互相印证**，只看一项会误判：

- 端口反查出的 pid 与你预期不符 → 不是那个 daemon；
- `CDS_MPS_SESSION` 与你以为的不同 → 是另一个 Virtuoso 实例；
- PPID 变了 → Virtuoso 重启过，端口和会话 id 都已经换了。

> vcli 自己判断"会话还活着不"**不看**这个 `pid`：它用的是端口探测
> （`SessionInfo::is_alive` 连 `127.0.0.1:<port>`）加一个 `<id>.stale` 标记文件。
> 所以注册 pid 为 `0` 不会让 vcli 误判 stale —— 但它也意味着**光看会话文件
> 你永远拿不到进程身份**，只能按上表从端口反查。

### 健康探测只用 `1+1`

```bash
VCLI_CAPABILITY=admin vcli skill exec '1+1' --format json
# → {"status":"success","output":"2",...}
```

健康探测**只**用 `1+1`。**不要**为了"再多确认一下"去调 `ipcGetPid()`：它不是公开 API，
而且每多一次 SKILL 往返，就多一次卡在 GUI 模态框上的机会。要 pid 就走上面的
`/proc` / `ps` / `ss` 交叉核验，不要占用 SKILL 通道。

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

## Windows PowerShell quoting

PowerShell 5.x strips inner double quotes when it starts a native exe, so
`vcli skill exec 'getShellEnvVar("USER")'` reaches SKILL as `getShellEnvVar(USER)`.
Read the expression from stdin or a file instead:

```powershell
Get-Content -Raw expr.il | vcli skill exec -
vcli skill exec --file expr.il
```

`--file` reads one SKILL expression for `exec`. It does not load a full `.il` file; that is `vcli skill load`.

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
| Input | argv, `-` (stdin), or `--file` | argv OR `--stdin` |
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
