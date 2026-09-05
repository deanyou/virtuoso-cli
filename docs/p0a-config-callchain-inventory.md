# P0-A · 配置调用链盘点（Config Call-Chain Inventory）

> 日期：2026-09-05
> 目标：为「命令层显式接收已解析 Config（CommandContext）」的改造提供量化依据。
> 对应外部审阅项：F06（删除 env 桥接还不够，调用链必须接收同一配置）、F05（配置摘要/身份校验）。

## 1. 现状问题

`main()` 把 `--target` / `--profile` / `--session` 通过 `set_var` 写进进程环境，
命令层随后再次调用 `Config::from_env()` / `VirtuosoClient::from_env()` 从环境重建配置。
后果：

- 配置在进程内被解析两次以上（main 一次、各命令层一次），无单一权威来源；
- `--profile` 与 `VB_TARGET` 的历史优先级矛盾（已修复：main 现在按
  `target::resolve` 的决定同步 env，见 `src/main.rs` 的 `resolve_selection` 分支）；
- 目标身份（target_id / config_digest）无法随配置传递，F05 的摘要校验无处挂载。

## 2. 生产路径上的配置重读入口

以下均为非测试代码里「从环境重建配置」的调用点：

| 调用点 | 位置 | 说明 |
|---|---|---|
| `Config::from_env()` | `src/commands/tunnel.rs:12` | `tunnel start` |
| `Config::from_env()` | `src/commands/tunnel.rs:61` | `tunnel attach` |
| `Config::from_env()` | `src/commands/tunnel.rs:171` | `tunnel status` |
| `Config::from_env()` | `src/transport/tunnel.rs:555,598` | SSHClient warm / 建连路径 |
| `Config::from_env()` | `src/spectre/runner.rs:688` | Spectre runner 建连 |
| `Config::from_env()` | `src/commands/session.rs:12` | session 命令读取 cfg |
| `SSHClient::from_env` | `src/transport/tunnel.rs:559` | 由 cfg 构造 SSHRunner |
| `VirtuosoClient::from_env()` | **76 处** | 主要入口，内部再走 `Config::from_env()` |
| `McpConfig::from_env()` | `src/mcp/server.rs:11` | MCP 独立配置（另议） |
| `TunnelState` / `SessionInfo` | `src/models.rs:472-495` | 按 profile 读写 session/tunnel 归属 |

## 3. 结论：改造面与顺序

- **大头是 `VirtuosoClient::from_env()`（76 处）**：它内部统一走
  `Config::from_env()`，只需把「接收显式 Config」做进 `VirtuosoClient` 的构造路径，
  即可一次性覆盖绝大多数调用点，无需逐命令改签名。
- 优先级：先 `VirtuosoClient`（76 处）→ `tunnel` 命令（start/attach/status）→
  `spectre/runner` → `session` 命令 → `MCP`。
- 落点设计（与审阅报告 P0-A 一致）：
  1. `main()` 调用 `target::resolve::resolve(cli.target, cli.profile)` 得到一次性的
     `ResolvedTarget { name, config }`；
  2. 构造轻量 `CommandContext { config, target_id: Option<String>, config_digest }`；
  3. `VirtuosoClient::from_context(&CommandContext)` 成为新入口，`from_env()` 保留为
     兼容封装；
  4. 删除 `main()` 的 `VB_TARGET` env 桥接 + `config.rs::from_env_with_profile` 的
     `VB_TARGET` 分支（当前已标注 TEMPORARY）。
- `config_digest`：对非秘密字段（host/port/user/jump/backend/超时等）做确定性摘要，
  供 daemon Hello / 请求身份校验（F05）。

## 4. 已完成（本阶段）

- `src/target/resolve.rs`（新）：`resolve_selection` / `resolve_target` / `resolve` +
  10 个单测（优先级、冲突、VB_TARGET、active_target、损坏配置报错）。
- `src/main.rs`：接入 `resolve_selection`；`--target`+`--profile` 冲突报错（exit 2）；
  目标不存在 exit 3；按 resolver 决定同步 env（profile/legacy 时清 `VB_TARGET`）。
- `config.rs::from_env_with_profile` 的 `VB_TARGET` 分支标注为 TEMPORARY 桥接。

## 5. 待办（下一步）

- `CommandContext` 结构 + `VirtuosoClient::from_context`；
- 迁移 76 处 `from_env` 调用点；
- 删除 env 桥接分支；
- target_id / config_digest 摘要计算（供 F05 校验）。
