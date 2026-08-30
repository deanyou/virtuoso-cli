# vcli — Virtuoso CLI

<p align="center">
  <a href="https://crates.io/crates/virtuoso-cli"><img src="https://img.shields.io/crates/v/virtuoso-cli.svg" alt="crates.io"/></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.75+-blue.svg" alt="Rust 1.75+"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green.svg" alt="License: MIT"/></a>
  <a href="https://github.com/deanyou/virtuoso-cli/actions"><img src="https://github.com/deanyou/virtuoso-cli/actions/workflows/ci.yml/badge.svg" alt="CI"/></a>
</p>

<p align="center">
  <a href="#english">English</a> | <a href="#中文">中文</a>
</p>

---

## English

Control Cadence Virtuoso from anywhere — locally or remotely. Designed for AI Agents and humans alike.

> **Based on** [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite) by Arcadia-1.
> `vcli` is a full Rust rewrite and major extension of that project, adding multi-session support, dynamic port assignment, session registry, an agent-native CLI, and Spectre simulation integration.

### Overview

`vcli` is a lightweight Rust-based bridge tool for executing SKILL code outside of Virtuoso. It starts a Rust daemon inside Virtuoso via `ramic_bridge.il`, which accepts commands over TCP, calls `evalstring`, and returns results.

### Key Features

- **Multi-session support** — Multiple Virtuoso instances on the same server each get a unique session ID and random port, with no conflicts
- **Multi-session broadcast** — `vcli skill broadcast` fans out to all live sessions concurrently via scoped threads; subagent mode supports heterogeneous multi-step per-session workflows
- **Dynamic port assignment** — Daemon binds port 0 (OS assigns), eliminating port collision
- **Session auto-discovery** — Single session connects automatically; multiple sessions require `--session` or `VB_SESSION`; stale session files for dead daemons are silently filtered out
- **CLI/daemon version unification** — `vcli session show` reports `daemon_version` and a `version_skew` warning when the running vcli binary and the deployed daemon don't match, so you know when to rebuild
- **Stale-daemon recovery** — `ramic_bridge.il` checks `ipcIsAliveProcess` on `load` and silently reaps dead daemons before starting a new one; no manual cleanup required
- **Session history** — Per-session SKILL execution log and CLI command history; survives reconnection (`vcli session history <id>`)
- **Three programming modes** — Raw SKILL expressions, high-level API, or load `.il` files directly
- **Local + remote modes** — Direct local connection or SSH tunnel with ControlMaster multiplexing
- **Native cross-arch tunnel deploy** — `vcli tunnel start` detects remote CPU arch and uploads the matching `resources/daemons/virtuoso-daemon-{x86_64,aarch64}` binary — no need to build on the remote
- **Non-destructive tunnel attach** — `vcli tunnel attach` connects to a daemon already running inside Virtuoso via SSH session discovery, without deploying a new one; `tunnel detach` drops the local tunnel while leaving the remote daemon untouched (4-verb model: start/stop are destructive, attach/detach are not)
- **Per-client scratch scoping** — Concurrent vcli invocations from different `VB_CLIENT_ID` / `VB_PROFILE` env values get isolated `/tmp/virtuoso_bridge/<client>/` scratch dirs, so two operators on the same host never collide
- **Skill Finder** — Fuzzy / prefix / suffix / exact / regex search over Cadence's `~/.cdsinit` and `/opt/cadence/.../finder/SKILL/*.fnd` files (`vcli skill find query --mode fuzzy`)
- **Admin capability gate** — `VCLI_CAPABILITY=admin` unlocks `vcli skill broadcast` and raw SKILL exec for system-wide operations
- **Agent-native CLI** — Noun-verb command structure, JSON structured output, schema introspection, semantic exit codes
- **Schematic editing & reading** — Create, place, wire, connect + read instances, nets, pins, parameters
- **Maestro ADE management** — Open/close Explorer (`maestro`) view sessions, set variables, run simulations, export results (IC23.1+ unified ADE)
- **Spectre simulation** — Sync/async simulation, job registry with status tracking and atomic file writes, PSF parser
- **Multi-profile support** — `--profile` flag for concurrent connections to multiple Virtuoso instances
- **Command logging** — All SKILL executions logged to `~/.cache/virtuoso_bridge/logs/commands.log`
- **Interactive TUI** — `vtui` terminal dashboard showing sessions, jobs, tunnel status

### Installation

**From crates.io (recommended):**

```bash
cargo install virtuoso-cli                          # vcli (main CLI)
cargo install virtuoso-cli --bin vtui               # vtui (interactive TUI dashboard)
cargo install virtuoso-cli --features daemon        # virtuoso-daemon (bridge backend)
```

**From source:**

```bash
git clone https://github.com/deanyou/virtuoso-cli.git
cd virtuoso-cli
cargo install --path .
```

All binaries (`vcli`, `vtui`) are installed to `~/.cargo/bin/`.

> **Note**: Do not name the binary `virtuoso` — it conflicts with Cadence's `virtuoso` executable.

### Quick Start

**1. Load RAMIC Bridge in Virtuoso CIW:**

```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
```

`load` automatically stops any existing daemon, resets the path to `~/.cargo/bin/virtuoso-daemon`, starts fresh, and prints the Ready banner — works for first load and for reloading after updates.

Output:
```
┌─────────────────────────────────────────┐
│  vcli (Virtuoso CLI Bridge) — Ready     │
├─────────────────────────────────────────┤
│  Session : eda-meow-1                   │
│  Port    : 42109                        │
│  SSH     : 22                           │
│  Version : 1.0.0                        │
│  Daemon  : ~/.cargo/bin/virtuoso-daemon │
├─────────────────────────────────────────┤
│  Terminal: vcli skill exec 'version()'  │
│  Sessions: vcli session list            │
└─────────────────────────────────────────┘
```

`SSH` is the port used for SSH tunnel connections (host network mode → 22; bridge mode with port mapping users can override via `RB_SSH_PORT` in the Virtuoso shell env before `load`). `Port` is the daemon's own TCP listener (OS-assigned, never collides).

Add to `~/.cdsinit` for automatic loading on Virtuoso startup:
```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
```

**2. Connect from terminal:**

```bash
vcli session list                                        # list active sessions
vcli skill exec 'getCurrentTime()'                       # auto-connects if single session
vcli --session eda-meow-2 skill exec 'getCurrentTime()' # specify session explicitly
```

**Remote mode (deploy new daemon):**
```bash
vcli init           # generate .env template
# edit .env: set VB_REMOTE_HOST, VB_SPECTRE_CMD (absolute path)
vcli tunnel start
vcli skill exec 'getCurrentTime()'
vcli tunnel stop
```

**Remote mode (connect to existing Virtuoso daemon — non-destructive):**
```bash
# Virtuoso + daemon already running on the remote host
vcli tunnel attach                       # discover + connect; does NOT deploy a new daemon
vcli skill exec 'getCurrentTime()'
vcli tunnel detach                       # drop the SSH tunnel; daemon keeps listening
```

The 4-verb tunnel model:

| Verb | Lifecycle | Remote effect |
|------|-----------|---------------|
| `tunnel start`  | Deploy    | uploads daemon binary, starts daemon on remote, builds tunnel |
| `tunnel stop`   | Destroy   | kills daemon + tunnel, removes `/tmp/virtuoso_bridge_*/` |
| `tunnel attach` | Connect   | scans `~/.cache/virtuoso_bridge/sessions/*.json`, builds tunnel to live daemon |
| `tunnel detach` | Disconnect | kills tunnel only; daemon stays alive |

`start`/`stop` are destructive (you own what you deployed); `attach`/`detach` are non-destructive (Virtuoso owns the daemon). Use `attach` when Virtuoso is already running and you just want CLI access.

**Remote async simulation:**
```bash
vcli sim run-async --netlist my_tb.scs   # launch on remote server, return immediately
vcli sim job-list                        # check all jobs (auto-refreshes status via SSH)
vcli sim job-status <id>                 # detailed status for one job
vcli sim job-cancel <id>                 # kill remote spectre process
```

**Maestro ADE Explorer (IC23.1+):**
```bash
# IC23.1 unified ADE uses "maestro" view (formerly adexl/ade_xl)
vcli maestro open --lib myLib --cell myCell            # defaults to view=maestro
vcli maestro set-var --session fnxSession4 --name W --value 10u
vcli maestro run --session fnxSession4                 # async run
vcli maestro export --session fnxSession4 --path out.csv
```

**Multi-Session Operations:**
```bash
vcli skill broadcast 'getVersion(t)'            # Same SKILL on all sessions
VB_SESSION=eda-meow-1 vcli maestro run ...      # Different tasks via subagents
```

### Multi-Session Architecture

```
Virtuoso-1 → vcli() → daemon on port 42109 → session: eda-meow-1
Virtuoso-2 → vcli() → daemon on port 51337 → session: eda-meow-2

Terminal A: vcli skill exec '...'                  # auto-selects (single session)
Terminal B: vcli --session eda-meow-2 skill exec   # explicit selection
```

Session files: `~/.cache/virtuoso_bridge/sessions/<id>.json`

### Command Reference

```
vcli [--profile P] [--session S] [--format json|table]
├── init                              Generate .env config template
├── session                           Manage bridge sessions
│   ├── list                              List all active sessions
│   ├── show [id]                         Show session details (with daemon_version + version_skew check)
│   ├── current                           Show which session would be auto-selected
│   ├── cleanup                           Remove stale session files for dead daemons
│   └── history <id> [--skill] [--cmd] [--limit N]   SKILL + CLI history for a session
├── tunnel                            Manage SSH tunnel
│   ├── start [--timeout N] [--dry-run]    Deploy new daemon + tunnel (destructive)
│   ├── stop [--force] [--dry-run]         Destroy what start created
│   ├── restart [--timeout N]
│   ├── attach [--dry-run]                 Connect to an existing Virtuoso daemon (non-destructive)
│   ├── detach                             Drop attach tunnel; daemon keeps listening
│   ├── status
│   └── diagnose                          Full connection diagnostics
├── skill                             Execute SKILL code
│   ├── exec <code> [--timeout N]
│   ├── load <file>
│   ├── broadcast <code>              Fan out to all live sessions in parallel (requires VCLI_CAPABILITY=admin)
│   ├── find <query> [--mode M] [--include-desc]   Search Cadence .fnd (fuzzy/prefix/suffix/exact/regex)
│   └── info <name>                     Detailed info for one SKILL function
├── cell                              Manage cellviews
│   ├── open --lib L --cell C [--view V] [--mode M] [--dry-run]
│   ├── save / close / info
├── schematic                         Schematic editing & reading
│   ├── open / save / check / build --spec file.json
│   ├── place / wire / conn / label / pin
│   ├── list-instances / list-nets / list-pins
│   └── get-params --inst M1
├── maestro                           Maestro ADE Explorer (maestro view) sessions
│   ├── open --lib L --cell C
│   ├── close / list-sessions / save
│   ├── set-var / get-analyses / add-output
│   ├── run / export
├── sim                               Simulation
│   ├── setup / run / measure / sweep / corner
│   ├── run-async --netlist file.scs
│   ├── job-status / job-list / job-cancel
│   └── results / netlist
├── design                            gm/Id sizing tools
│   ├── size / explore
├── process                           Process characterization
│   └── char [--netlist]
└── schema [--all] [noun] [verb]      Output command schema (for Agent discovery)
```

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `VB_SESSION` | — | Target session ID (for multi-instance) |
| `VB_PORT` | per-user hash | Direct port (fallback when no session file) |
| `VB_REMOTE_HOST` | — | SSH remote hostname or alias |
| `VB_REMOTE_USER` | current user | SSH login username |
| `VB_JUMP_HOST` | — | Bastion/jump host address |
| `VB_TIMEOUT` | `30` | Connection/execution timeout (seconds) |
| `VB_SSH_PORT` | `22` | SSH port on the remote host (used by `vcli tunnel`); distinct from `VB_PORT` which is the daemon's TCP listener |
| `VB_PROFILE` | — | Config profile (reads `VB_*_<profile>` vars) |
| `VB_CLIENT_ID` | `$VB_PROFILE` or `gethostname()` | Per-client remote scratch scoping (e.g. `vcli-A`, `vcli-B`); isolates `/tmp/virtuoso_bridge/<client>/` paths between concurrent operators |
| `VCLI_CAPABILITY` | `user` | Set to `admin` to unlock `vcli skill broadcast` and raw SKILL exec |
| `RB_DAEMON_PATH` | auto-detected | Override daemon binary path |

### How It Works

```
Terminal                      Virtuoso Process
────────                      ────────────────

vcli skill exec "1+2"
      │
      │ TCP: {"skill":"1+2"}
      ├──────────────────► virtuoso-daemon (port 42109)
      │                          │
      │                          │ evalstring("1+2")
      │                          │
      │ TCP: "3"
      ◄──────────────────────────┘
```

Session registration flow:
```
vcli() in CIW
  → RBStart(): ipcBeginProcess(daemon, port=0)
  → OS assigns port N; daemon prints "PORT:N" to stderr
  → RBIpcErrHandler: RBPort=N, writes session file
  → ~/.cache/virtuoso_bridge/sessions/<id>.json

vcli session list  # reads session files
vcli skill exec    # connects to port N
```

---

## 中文

从任何地方控制 Cadence Virtuoso，本地或远程均可。为 AI Agent 和人类共同设计。

> **基于** [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite)（作者 Arcadia-1）重构。
> `vcli` 是对该项目的完整 Rust 重写与大幅扩展，新增了多 session 支持、动态端口分配、session 注册表、Agent 原生 CLI 以及 Spectre 仿真集成。

### 简介

`vcli` 是一个用 Rust 编写的轻量级桥接工具，用于在 Virtuoso 外部执行 SKILL 代码。它通过 `ramic_bridge.il` 在 Virtuoso 内启动一个 Rust daemon，并通过 TCP 接收来自 CLI 的命令，调用 `evalstring` 执行 SKILL 并返回结果。

### 核心特性

- **多 session 支持** — 同一台服务器上可同时运行多个 Virtuoso 实例，每个实例自动分配唯一 session_id 和随机端口，互不干扰
- **多 session 并发广播** — `vcli skill broadcast` 通过 scoped threads 并发广播到所有活跃 session；subagent 模式支持不同 session 执行不同多步工作流
- **动态端口分配** — daemon 绑定端口 0（OS 自动分配），彻底避免端口冲突
- **session 自动发现** — 只有一个 session 时无需指定；多个 session 时通过 `--session` 或 `VB_SESSION` 选择；已死亡的 daemon 对应的 session 文件自动过滤
- **CLI/daemon 版本统一** — `vcli session show` 显示 `daemon_version` 字段，若 vcli 二进制版本与运行中的 daemon 不一致会报 `version_skew` 警告，提示需重新部署
- **陈旧 daemon 自动恢复** — `ramic_bridge.il` 在 `load` 时检查 `ipcIsAliveProcess`，静默清理已死的 daemon 后再启动新实例；无需手动清理
- **Session 历史记录** — 每个 session 独立保存 SKILL 执行日志和 CLI 命令历史，断线重连后可恢复（`vcli session history <id>`）
- **三种编程方式** — 原始 SKILL 表达式、高阶 API、或直接加载 .il 文件
- **本地+远程模式** — 支持本地直连或 SSH 隧道（ControlMaster 连接复用）
- **隧道跨架构原生部署** — `vcli tunnel start` 自动检测远端 CPU 架构，并上传对应的 `resources/daemons/virtuoso-daemon-{x86_64,aarch64}` 二进制，无需在远端 build
- **非破坏性隧道连接** — `vcli tunnel attach` 通过 SSH session 发现机制连接 Virtuoso 内已运行的 daemon，不部署新 daemon；`tunnel detach` 仅断开本地隧道，远端 daemon 保持运行（4 动词模型：start/stop 破坏性，attach/detach 非破坏性）
- **每客户端 scratch 隔离** — 不同 `VB_CLIENT_ID` / `VB_PROFILE` 环境变量下的并发 vcli 调用拥有独立的 `/tmp/virtuoso_bridge/<client>/` 目录，多个操作员在同一主机操作时互不冲突
- **Skill Finder** — 模糊 / 前缀 / 后缀 / 精确 / 正则 五种模式搜索 Cadence `~/.cdsinit` 与 `/opt/cadence/.../finder/SKILL/*.fnd` 文件（`vcli skill find query --mode fuzzy`）
- **Admin 权限门** — `VCLI_CAPABILITY=admin` 解锁 `vcli skill broadcast` 与原始 SKILL 执行权限
- **Agent 原生 CLI** — noun-verb 命令结构、JSON 结构化输出、schema 自省、语义化退出码
- **原理图编辑与读取** — 创建、放置、连线 + 读取实例/网络/引脚/参数
- **Maestro ADE 管理** — 打开/关闭 Explorer（`maestro` view）session、设置变量、运行仿真、导出结果（IC23.1+ 统一 ADE）
- **Spectre 仿真** — 同步/异步仿真、Job 注册与状态跟踪、PSF 结果解析
- **多 Profile 支持** — `--profile` 参数支持同时连接多个 Virtuoso 实例
- **命令日志** — 所有 SKILL 调用记录到 `~/.cache/virtuoso_bridge/logs/commands.log`
- **交互式 TUI** — `vtui` 终端仪表盘，实时显示 session、仿真 job、隧道状态

### 安装

**从 crates.io 安装（推荐）：**

```bash
cargo install virtuoso-cli                          # vcli（主 CLI）
cargo install virtuoso-cli --bin vtui               # vtui（交互式 TUI 仪表盘）
cargo install virtuoso-cli --features daemon        # virtuoso-daemon（bridge 后端）
```

**从源码安装：**

```bash
git clone https://github.com/deanyou/virtuoso-cli.git
cd virtuoso-cli
cargo install --path .
```

安装后 `vcli` 和 `virtuoso-daemon` 均位于 `~/.cargo/bin/`。

> **注意**：不要将 CLI 命名为 `virtuoso`，与 Cadence Virtuoso 二进制名冲突。

### 快速开始

**第一步：在 Virtuoso CIW 中加载 RAMIC Bridge：**

```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
```

`load` 会自动停止旧 daemon、将路径重置为 `~/.cargo/bin/virtuoso-daemon` 并重启，首次加载和更新后重载均适用。

输出：
```
┌─────────────────────────────────────────┐
│  vcli (Virtuoso CLI Bridge) — Ready     │
├─────────────────────────────────────────┤
│  Session : eda-meow-1                   │
│  Port    : 42109                        │
│  SSH     : 22                           │
│  Version : 1.0.0                        │
│  Daemon  : ~/.cargo/bin/virtuoso-daemon │
├─────────────────────────────────────────┤
│  Terminal: vcli skill exec 'version()'  │
│  Sessions: vcli session list            │
└─────────────────────────────────────────┘
```

`SSH` 是 SSH 隧道连接用的端口（host 网络模式下为 22；bridge 模式 + 端口映射的用户可在 `load` 前通过 Virtuoso shell 环境变量 `RB_SSH_PORT` 覆盖）。`Port` 是 daemon 自身的 TCP 监听端口（由 OS 分配，永不冲突）。

在 `~/.cdsinit` 中加入以下内容，实现 Virtuoso 启动时自动加载：
```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
```

**第二步：从终端连接：**

```bash
vcli session list                                        # 查看所有活跃 session
vcli skill exec 'getCurrentTime()'                       # 单 session 时自动连接
vcli --session eda-meow-2 skill exec 'getCurrentTime()' # 多 session 时指定目标
```

**远程模式（部署新 daemon）：**
```bash
vcli init           # 生成 .env 配置模板
# 编辑 .env：设置 VB_REMOTE_HOST、VB_SPECTRE_CMD（绝对路径）
vcli tunnel start
vcli skill exec 'getCurrentTime()'
vcli tunnel stop
```

**远程模式（连接已存在的 Virtuoso daemon — 非破坏性）：**
```bash
# Virtuoso + daemon 已在远端主机运行
vcli tunnel attach                       # 自动发现 + 连接；不会部署新 daemon
vcli skill exec 'getCurrentTime()'
vcli tunnel detach                       # 断开 SSH 隧道；daemon 继续监听
```

隧道 4-动词模型：

| 动词 | 生命周期 | 远端副作用 |
|------|---------|-----------|
| `tunnel start`  | 部署   | 上传 daemon 二进制，远端启动 daemon，建隧道 |
| `tunnel stop`   | 销毁   | 杀 daemon + 隧道，删除 `/tmp/virtuoso_bridge_*/` |
| `tunnel attach` | 连接   | 扫描 `~/.cache/virtuoso_bridge/sessions/*.json`，建隧道到活跃 daemon |
| `tunnel detach` | 断开   | 仅杀隧道；daemon 保持运行 |

`start`/`stop` 是破坏性的（你部署的你能清）；`attach`/`detach` 是非破坏性的（daemon 归 Virtuoso 所有）。当 Virtuoso 已在跑、你只想用 CLI 连进去时，用 `attach`。

**远程异步仿真：**
```bash
vcli sim run-async --netlist my_tb.scs   # 在远程服务器启动仿真，立即返回
vcli sim job-list                        # 查看所有 job（通过 SSH 自动刷新状态）
vcli sim job-status <id>                 # 查看单个 job 详情
vcli sim job-cancel <id>                 # 终止远程 spectre 进程
```

**Maestro ADE Explorer（IC23.1+）：**
```bash
# IC23.1 统一 ADE 使用 "maestro" view（旧版本为 adexl/ade_xl）
vcli maestro open --lib myLib --cell myCell            # 默认 view=maestro
vcli maestro set-var --session fnxSession4 --name W --value 10u
vcli maestro run --session fnxSession4                 # 异步运行
vcli maestro export --session fnxSession4 --path out.csv
```

**多 session 并发操作：**
```bash
vcli skill broadcast 'getVersion(t)'            # 同一 SKILL 广播到所有 session
VB_SESSION=eda-meow-1 vcli maestro run ...      # 通过 subagent 实现不同任务
```

### 多 Session 工作原理

```
Virtuoso-1 → vcli() → daemon on port 42109 → session: eda-meow-1
Virtuoso-2 → vcli() → daemon on port 51337 → session: eda-meow-2

终端 A: vcli skill exec '...'                  # 自动连接（单 session）
终端 B: vcli --session eda-meow-2 skill exec   # 显式指定
```

Session 注册文件保存在 `~/.cache/virtuoso_bridge/sessions/<id>.json`。

### 命令参考

```
vcli [--profile P] [--session S] [--format json|table]
├── init                              创建 .env 配置模板
├── session                           管理 bridge session
│   ├── list                              列出所有活跃 session
│   ├── show [id]                         查看 session 详情（含 daemon_version + version_skew 检查）
│   ├── current                           显示会被自动选中的 session
│   ├── cleanup                           删除已死亡 daemon 的 session 文件
│   └── history <id> [--skill] [--cmd] [--limit N]   查看 SKILL + CLI 历史
├── tunnel                            管理 SSH 隧道
│   ├── start [--timeout N] [--dry-run]    部署新 daemon + 隧道（破坏性）
│   ├── stop [--force] [--dry-run]         销毁 start 创建的内容
│   ├── restart [--timeout N]
│   ├── attach [--dry-run]                 连接已存在的 Virtuoso daemon（非破坏性）
│   ├── detach                             断开 attach 隧道；daemon 继续监听
│   ├── status
│   └── diagnose                          完整连接诊断
├── skill                             执行 SKILL 代码
│   ├── exec <code> [--timeout N]
│   ├── load <file>
│   ├── broadcast <code>              并发广播到所有活跃 session（需 VCLI_CAPABILITY=admin）
│   ├── find <query> [--mode M] [--include-desc]   搜索 Cadence .fnd（fuzzy/prefix/suffix/exact/regex）
│   └── info <name>                     查看单个 SKILL 函数详情
├── cell                              管理 cellview
│   ├── open / save / close / info
├── schematic                         原理图编辑与读取
│   ├── open / save / check / build --spec file.json
│   ├── place / wire / conn / label / pin
│   ├── list-instances / list-nets / list-pins
│   └── get-params --inst M1
├── maestro                           Maestro ADE Explorer（maestro view）仿真
│   ├── open / close / list-sessions / save
│   ├── set-var / get-analyses / add-output
│   ├── run / export
├── sim                               仿真
│   ├── setup / run / measure / sweep / corner
│   ├── run-async / job-status / job-list / job-cancel
│   └── results / netlist
├── design                            gm/Id 设计工具
├── process                           工艺表征
└── schema [--all] [noun] [verb]      输出命令 schema（供 Agent 发现）
```

### 配置说明

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `VB_SESSION` | - | 目标 session ID（多实例时使用） |
| `VB_PORT` | 按用户名 hash | 直连端口（无 session 文件时的回退值） |
| `VB_REMOTE_HOST` | - | SSH 远程主机名或别名 |
| `VB_REMOTE_USER` | 当前用户 | SSH 登录用户名 |
| `VB_JUMP_HOST` | - | 跳板机/堡垒机地址 |
| `VB_TIMEOUT` | `30` | 连接/执行超时（秒） |
| `VB_SSH_PORT` | `22` | 远端主机 SSH 端口（`vcli tunnel` 使用）；与 `VB_PORT`（daemon TCP 监听端口）不同 |
| `VB_PROFILE` | - | 配置 profile（读取 `VB_*_<profile>` 变量） |
| `VB_CLIENT_ID` | `$VB_PROFILE` 或 `gethostname()` | 每客户端远端 scratch 隔离标识（如 `vcli-A`、`vcli-B`）；隔离 `/tmp/virtuoso_bridge/<client>/` 路径，避免多操作员并发冲突 |
| `VCLI_CAPABILITY` | `user` | 设为 `admin` 解锁 `vcli skill broadcast` 与原始 SKILL 执行权限 |
| `RB_DAEMON_PATH` | 自动检测 | 覆盖 daemon 二进制路径 |

### 工作原理

```
终端                          Virtuoso 进程
────                          ─────────────

vcli skill exec "1+2"
      │
      │ TCP: {"skill":"1+2"}
      ├──────────────────► virtuoso-daemon (port 42109)
      │                          │
      │                          │ evalstring("1+2") → "3"
      │                          │
      │ TCP: "3"
      ◄──────────────────────────┘
```

Session 注册流程：
```
vcli() in CIW
  → RBStart(): ipcBeginProcess(daemon, port=0)
  → OS 分配端口 N；daemon 打印 "PORT:N" 到 stderr
  → RBIpcErrHandler: RBPort=N，写入 session 文件
  → ~/.cache/virtuoso_bridge/sessions/<id>.json

vcli session list  # 读取 session 文件
vcli skill exec    # 连接到端口 N
```

---

## License / 许可证

MIT License — see [LICENSE](LICENSE)
