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
- **Dynamic port assignment** — Daemon binds port 0 (OS assigns), eliminating port collision
- **Session auto-discovery** — Single session connects automatically; multiple sessions require `--session` or `VB_SESSION`
- **Three programming modes** — Raw SKILL expressions, high-level API, or load `.il` files directly
- **Local + remote modes** — Direct local connection or SSH tunnel for remote servers
- **Agent-native CLI** — Noun-verb command structure, JSON structured output, schema introspection, semantic exit codes
- **Spectre simulation integration** — Built-in local/remote simulation runner and PSF result parser

### Installation

**From crates.io (recommended):**

```bash
cargo install virtuoso-cli                          # vcli (main CLI)
cargo install virtuoso-cli --features daemon        # virtuoso-daemon (bridge backend)
```

**From source:**

```bash
git clone https://github.com/deanyou/virtuoso-cli.git
cd virtuoso-cli
cargo install --path . --bin vcli
cargo install --path . --bin virtuoso-daemon --features daemon
```

Both binaries are installed to `~/.cargo/bin/`.

> **Note**: Do not name the binary `virtuoso` — it conflicts with Cadence's `virtuoso` executable.

### Quick Start

**1. Load RAMIC Bridge in Virtuoso CIW:**

```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
vcli()
```

Output:
```
┌─────────────────────────────────────────┐
│  vcli (Virtuoso CLI Bridge) — Ready     │
├─────────────────────────────────────────┤
│  Session : eda-meow-1                   │
│  Port    : 42109                        │
├─────────────────────────────────────────┤
│  Terminal: vcli skill exec 'version()'  │
│  Sessions: vcli session list            │
└─────────────────────────────────────────┘
```

Add to `~/.cdsinit` for automatic loading on Virtuoso startup:
```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
vcli()
```

**2. Connect from terminal:**

```bash
vcli session list                                        # list active sessions
vcli skill exec 'getCurrentTime()'                       # auto-connects if single session
vcli --session eda-meow-2 skill exec 'getCurrentTime()' # specify session explicitly
```

**Remote mode:**
```bash
vcli init           # generate .env template
# edit .env: set VB_REMOTE_HOST
vcli tunnel start
vcli skill exec 'getCurrentTime()'
vcli tunnel stop
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
vcli
├── init                              Generate .env config template
├── session                           Manage bridge sessions
│   ├── list                              List all active sessions
│   └── show [id]                         Show session details
├── tunnel                            Manage SSH tunnel
│   ├── start [--timeout N] [--dry-run]
│   ├── stop [--force] [--dry-run]
│   ├── restart [--timeout N]
│   └── status
├── skill                             Execute SKILL code
│   ├── exec <code> [--timeout N]
│   └── load <file>
├── cell                              Manage cellviews
│   ├── open --lib L --cell C [--view V] [--mode M] [--dry-run]
│   ├── save / close / info
└── schema [--all] [noun] [verb]      Output command schema (for Agent discovery)
```

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `VB_SESSION` | — | Target session ID (for multi-instance) |
| `VB_PORT` | — | Direct port (fallback when no session file) |
| `VB_REMOTE_HOST` | — | SSH remote hostname or alias |
| `VB_REMOTE_USER` | current user | SSH login username |
| `VB_JUMP_HOST` | — | Bastion/jump host address |
| `VB_TIMEOUT` | `30` | Connection/execution timeout (seconds) |
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
- **动态端口分配** — daemon 绑定端口 0（OS 自动分配），彻底避免端口冲突
- **session 自动发现** — 只有一个 session 时无需指定；多个 session 时通过 `--session` 或 `VB_SESSION` 选择
- **三种编程方式** — 原始 SKILL 表达式、高阶 API、或直接加载 .il 文件
- **本地+远程模式** — 支持本地直连或 SSH 隧道远程控制
- **Agent 原生 CLI** — noun-verb 命令结构、JSON 结构化输出、schema 自省、语义化退出码
- **Spectre 仿真集成** — 内置本地/远程仿真运行器和 PSF 结果解析

### 安装

**从 crates.io 安装（推荐）：**

```bash
cargo install virtuoso-cli                          # vcli（主 CLI）
cargo install virtuoso-cli --features daemon        # virtuoso-daemon（bridge 后端）
```

**从源码安装：**

```bash
git clone https://github.com/deanyou/virtuoso-cli.git
cd virtuoso-cli
cargo install --path . --bin vcli
cargo install --path . --bin virtuoso-daemon --features daemon
```

安装后 `vcli` 和 `virtuoso-daemon` 均位于 `~/.cargo/bin/`。

> **注意**：不要将 CLI 命名为 `virtuoso`，与 Cadence Virtuoso 二进制名冲突。

### 快速开始

**第一步：在 Virtuoso CIW 中加载 RAMIC Bridge：**

```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
vcli()
```

输出：
```
┌─────────────────────────────────────────┐
│  vcli (Virtuoso CLI Bridge) — Ready     │
├─────────────────────────────────────────┤
│  Session : eda-meow-1                   │
│  Port    : 42109                        │
├─────────────────────────────────────────┤
│  Terminal: vcli skill exec 'version()'  │
│  Sessions: vcli session list            │
└─────────────────────────────────────────┘
```

在 `~/.cdsinit` 中加入以下内容，实现 Virtuoso 启动时自动加载：
```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
vcli()
```

**第二步：从终端连接：**

```bash
vcli session list                                        # 查看所有活跃 session
vcli skill exec 'getCurrentTime()'                       # 单 session 时自动连接
vcli --session eda-meow-2 skill exec 'getCurrentTime()' # 多 session 时指定目标
```

**远程模式：**
```bash
vcli init           # 生成 .env 配置模板
# 编辑 .env：设置 VB_REMOTE_HOST
vcli tunnel start
vcli skill exec 'getCurrentTime()'
vcli tunnel stop
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
vcli
├── init                              创建 .env 配置模板
├── session                           管理 bridge session
│   ├── list                              列出所有活跃 session
│   └── show [id]                         查看 session 详情
├── tunnel                            管理 SSH 隧道
│   ├── start [--timeout N] [--dry-run]
│   ├── stop [--force] [--dry-run]
│   ├── restart [--timeout N]
│   └── status
├── skill                             执行 SKILL 代码
│   ├── exec <code> [--timeout N]
│   └── load <file>
├── cell                              管理 cellview
│   ├── open --lib L --cell C [--view V] [--mode M] [--dry-run]
│   ├── save / close / info
└── schema [--all] [noun] [verb]      输出命令 schema（供 Agent 发现）
```

### 配置说明

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `VB_SESSION` | - | 目标 session ID（多实例时使用） |
| `VB_PORT` | - | 直连端口（无 session 文件时的回退值） |
| `VB_REMOTE_HOST` | - | SSH 远程主机名或别名 |
| `VB_REMOTE_USER` | 当前用户 | SSH 登录用户名 |
| `VB_JUMP_HOST` | - | 跳板机/堡垒机地址 |
| `VB_TIMEOUT` | `30` | 连接/执行超时（秒） |
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
