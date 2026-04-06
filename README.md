# vcli — Virtuoso CLI

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.75+-blue.svg" alt="Rust 1.75+"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green.svg" alt="License: MIT"/></a>
</p>

从任何地方控制 Cadence Virtuoso，本地或远程均可。为 AI Agent 和人类共同设计。

---

## 简介

`vcli` 是一个用 Rust 编写的轻量级桥接工具，用于在 Virtuoso 外部执行 SKILL 代码。它通过 `ramic_bridge.il` 在 Virtuoso 内启动一个 Rust daemon，并通过 TCP 接收来自 CLI 的命令，调用 `evalstring` 执行 SKILL 并返回结果。

### 核心特性

- **多 session 支持** — 同一台服务器上可同时运行多个 Virtuoso 实例，每个实例自动分配唯一 session_id 和随机端口，互不干扰
- **动态端口分配** — daemon 绑定端口 0（OS 自动分配），彻底避免端口冲突
- **session 自动发现** — 只有一个 session 时无需指定；多个 session 时通过 `--session` 或 `VB_SESSION` 选择
- **三种编程方式** — 原始 SKILL 表达式、高阶 API、或直接加载 .il 文件
- **本地+远程模式** — 支持本地直连或 SSH 隧道远程控制
- **Agent 原生 CLI** — noun-verb 命令结构、JSON 结构化输出、schema 自省、语义化退出码
- **Spectre 仿真集成** — 内置本地/远程仿真运行器和 PSF 结果解析

---

## 安装

```bash
git clone https://github.com/your-repo/virtuoso-cli.git
cd virtuoso-cli

# 安装 vcli（主 CLI）
cargo install --path . --bin vcli

# 安装 virtuoso-daemon（RAMIC bridge 后端）
cargo install --path . --bin virtuoso-daemon --features daemon
```

安装后 `vcli` 和 `virtuoso-daemon` 均位于 `~/.cargo/bin/`。

> **注意**：不要将 `vcli` 命名为 `virtuoso`，与 Cadence Virtuoso 二进制名冲突。

---

## 快速开始

### 1. 加载 RAMIC Bridge

在 Virtuoso CIW 中：

```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
vcli()
```

`vcli()` 自动启动 daemon 并打印 session 信息：

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

也可在 `~/.cdsinit` 中加入以下内容，实现 Virtuoso 启动时自动加载：

```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
vcli()
```

### 2. 从终端连接

```bash
# 查看所有活跃 session
vcli session list

# 执行 SKILL（单 session 时自动连接）
vcli skill exec 'version()'

# 多 session 时指定目标
vcli --session eda-meow-2 skill exec 'version()'
```

### 远程模式

```bash
# 1. 初始化配置文件
vcli init

# 2. 编辑 .env（至少设置 VB_REMOTE_HOST）

# 3. 启动 SSH 隧道
vcli tunnel start

# 4. 执行 SKILL（与本地相同）
vcli skill exec "dbOpenCellViewByType(\"myLib\" \"myCell\" \"layout\" \"r\")"

# 5. 停止隧道
vcli tunnel stop
```

---

## 多 Session 工作原理

```
Virtuoso-1 → vcli() → daemon on port 42109 → session: eda-meow-1
Virtuoso-2 → vcli() → daemon on port 51337 → session: eda-meow-2

终端 A: vcli skill exec 'version()'           # 自动连接（单 session）
终端 B: vcli --session eda-meow-2 skill exec  # 显式指定
```

Session 注册文件保存在 `~/.cache/virtuoso_bridge/sessions/<id>.json`，由 bridge.il 自动写入，由 `vcli` 读取。

---

## 命令参考

```
vcli
├── init                              创建 .env 配置模板
├── session                           管理 bridge session
│   ├── list                              列出所有活跃 session
│   └── show [id]                         查看 session 详情
├── tunnel                            管理 SSH 隧道
│   ├── start [--timeout N] [--dry-run]   启动隧道 + 部署 daemon
│   ├── stop [--force] [--dry-run]        停止隧道
│   ├── restart [--timeout N]             重启隧道
│   └── status                            检查连接状态
├── skill                             执行 SKILL 代码
│   ├── exec <code> [--timeout N]         执行 SKILL 表达式
│   └── load <file>                       上传并加载 .il 文件
├── cell                              管理 cellview
│   ├── open --lib L --cell C [--view V] [--mode M] [--dry-run]
│   ├── save                              保存当前 cellview
│   ├── close                             关闭当前 cellview
│   └── info                              查看当前 cellview 信息
└── schema [--all] [noun] [verb]      输出命令 schema（供 Agent 发现）
```

### 全局参数

| 参数 | 说明 |
|------|------|
| `--session <id>` | 指定目标 session（多实例时必填） |
| `--format json\|table` | 输出格式（TTY 默认 table，管道默认 json） |
| `--no-color` | 禁用彩色输出 |
| `--quiet` / `-q` | 静默模式 |
| `--verbose` / `-v` | 调试日志 |

### 退出码

| 退出码 | 含义 |
|--------|------|
| 0 | 成功 |
| 1 | 一般错误 |
| 2 | 参数/用法错误 |
| 3 | 资源未找到 |
| 5 | 冲突（如 .env 已存在） |
| 10 | dry-run 通过 |

---

## 配置

运行 `vcli init` 生成 `.env` 配置模板。所有配置通过环境变量或 `.env` 文件设置。

### 配置变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `VB_SESSION` | - | 目标 session ID（多实例时使用） |
| `VB_PORT` | - | 直连端口（无 session 文件时的回退值） |
| `VB_REMOTE_HOST` | - | SSH 远程主机名或别名 |
| `VB_REMOTE_USER` | 当前用户 | SSH 登录用户名 |
| `VB_JUMP_HOST` | - | 跳板机/堡垒机地址 |
| `VB_JUMP_USER` | - | 跳板机用户名 |
| `VB_TIMEOUT` | `30` | 连接/执行超时（秒） |
| `VB_KEEP_REMOTE_FILES` | `false` | 停止时是否保留远程部署文件 |
| `VB_SPECTRE_CMD` | `spectre` | Spectre 可执行文件路径 |
| `VB_SPECTRE_ARGS` | - | Spectre 额外参数（支持 shell 引号语法） |
| `RB_DAEMON_PATH` | 自动检测 | 覆盖 daemon 二进制路径 |

### RAMIC Bridge 配置

`ramic_bridge.il` 加载后支持以下 CIW 变量（均在 `load()` 后保持，不会被重置）：

| 变量 | 说明 |
|------|------|
| `RBDPath` | daemon 路径（自动检测 `RB_DAEMON_PATH` → `which` → `~/.cargo/bin`） |
| `RBLocal` | `t` = 仅监听 127.0.0.1，`nil` = 监听所有接口 |
| `RBEcho` | `t` = 打印每条 IPC 消息（调试用） |
| `RBDLog` | `t` = daemon 日志写入 `/tmp/RB.log` |
| `RBPort` | 当前 session 绑定端口（由 OS 分配，只读） |
| `RBSessionId` | 当前 session ID（格式：`hostname-username-N`） |

---

## 工作原理

```
终端                          Virtuoso 进程
────                          ─────────────

vcli skill exec "1+2"
      │
      │ TCP: {"skill":"1+2"}
      ├──────────────────► virtuoso-daemon (port 42109)
      │                          │
      │                          │ ipcWriteProcess → evalstring("1+2")
      │                          │        │
      │                          │        ▼
      │                          │ ipcReadProcess ← "\x023\x1e"
      │                          │
      │ TCP: "3"
      ◄──────────────────────────┘
      │
      ▼
     "3"
```

Session 注册流程：
```
vcli() in CIW
  → RBStart(): ipcBeginProcess(daemon, port=0)
  → OS assigns port N
  → daemon prints "PORT:N" to stderr
  → RBIpcErrHandler: RBPort=N, RBWriteSession(id, N)
  → ~/.cache/virtuoso_bridge/sessions/<id>.json written

vcli session list    # reads session files
vcli skill exec ...  # connects to port N
```

---

## 构建

```bash
# 构建所有 binary
cargo build --release --features daemon

# 单独构建
cargo build --release              # 只构建 vcli
cargo build --release --features daemon --bin virtuoso-daemon
```

---

## 许可证

MIT License - 详见 LICENSE 文件
