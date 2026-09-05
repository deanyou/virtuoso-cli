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

- `src/target/resolve.rs`（新）：`resolve_selection` / `resolve_target` / `resolve` /
  `resolve_from_selection` + 17 个单测（优先级、冲突、VB_TARGET、active_target、
  失效 active_target 报错、最终配置层回归）。
- `src/main.rs`：接入 resolver；`--target`+`--profile` 冲突报错（exit 2）；
  目标不存在 exit 3；失效 active_target 报错（exit 2）；按 resolver 决定同步 env。
- `config.rs::from_env_with_profile` 的 `VB_TARGET` 分支标注为 TEMPORARY 桥接；
  新增 `from_env_with_profile_no_target`（resolver 隔离 VB_TARGET 干扰）；
  新增 `Config::digest()`（身份摘要，F05）。
- `src/context.rs`（新）：`CommandContext { config, target_id, config_digest }` +
  `validate_session_ownership`（4 单测：摘要稳定性、归属通过/拒绝/跳过）。

## 5. CommandContext 迁移进度（P0-A 进行中）

| 命令族 | 状态 |
|---|---|
| tunnel（start/stop/restart/status/diagnose/attach/detach） | ✅ 已迁移：接收 `&CommandContext`，经 `ctx.config()` / `SSHClient::from_config` / `VirtuosoClient::from_context` 使用单次解析配置 |
| `VirtuosoClient` | ✅ `from_context(ctx)` 新入口；`from_config(cfg, target_id)` 拆出；构造时做会话目标归属校验 |
| session 命令族 | ⏳ 未迁移（仍经 env 重读） |
| spectre/runner、skill、maestro 等 | ⏳ 未迁移（仍经 env 重读） |
| env 桥接（`VB_TARGET`） | ⚠️ 保留给未迁移命令族；与 CommandContext 共用同一 `resolve_selection`，保证一致性 |

- 端到端验证：`tunnel start --dry-run` 对 `--target`/`VB_TARGET`/active_target/
  `--profile` 四种选择均正确解析；目标缺失 exit 3、失效 active_target exit 2；
  `tunnel status` 输出含 `target` 与 `config_digest`。

### 5.1 tunnel 命令族二轮审阅修复（本地回退 / attach 归属 / profile 参数）

针对 f68bf12 后审阅的 4 项问题（提交后）：

| 问题 | 修复 |
|---|---|
| `ensure_tunnel` 本地端口递增会改写远端身份 | 拆分本地/远端：`SSHClient.remote_bridge_port = cfg.port` 固定，`try_ssh_tunnel(local, remote)` 各自独立；本地端口回退时 `TunnelState` 用 `remote_bridge_port` 单独记录远端端口；`validate_tunnel_ownership` 以远端端口为判别（`remote_bridge_port.or(attached_remote_port).unwrap_or(port)`）；建隧失败清理远端 setup 目录 |
| attach 未按目标端口筛选会话 | `scoped_attach_sessions()` 先按 `cfg.port` 过滤，再做存活探测与 `validate_session_ownership`；同主机不同端口会话不再误入本目标命名空间（3 单测） |
| `stop_saved_tunnel`/`SSHClient::stop` 远端清理重读 env | 改用传入 `cfg`（`SSHClient::from_config`）构造清理连接；`SSHClient` 持有 `config` 供 `stop()` 使用，杜绝"env 主机 + context 目录"组合 |
| `profile show` 丢失显式 `--profile` | `dispatch_profile(cmd, cli_profile)` 直传 CLI profile；配置管理命令仍校验 `--target`/`--profile` 冲突（exit 2）；2 bin 单测 |

`tunnel status` 输出新增 `remote_bridge_port`（归属判别端口），便于诊断本地回退。

### 5.2 tunnel 命令族三轮审阅修复（默认端口非约束 / 转发成功证明 / 部署-尚未就绪）

针对 8cbf83a 后审阅的 4 项问题（本轮，代码已改、未提交）：

| 问题 | 修复 |
|---|---|
| attach 无条件按 `cfg.port` 过滤，破坏 legacy 自动发现与 OS 分配端口 | `Config` 新增 `port_explicit: bool`（from_env 按 `VB_PORT` 是否存在、from_target 按 `target.port.is_some()`）。`attach_candidate_sessions()` 仅在「有 target 且 port_explicit」时按端口过滤；target 默认端口与 legacy/profile 一律保留自动发现。`CommandContext::validate_endpoint_ownership` 的端口臂同样只在 `port_explicit` 时生效（host 恒校验）——默认（hash-of-USER）端口不是端点约束，因为 daemon 绑定 OS 分配端口 |
| `try_ssh_tunnel` 只探测本地端口连通，误判转发成功（端口被其他服务占用时 SSH 已退出） | 去掉 `-f`，spawn 出的 ssh 进程即转发持有者；`wait_for_forward()` 要求「ssh 子进程存活 + 本地端口可达」双条件，探测后再查一次子进程存活；失败即 kill+reap 该次进程，供下一本地端口重试。补真实端口占用测试（occupied+exited→Err、live+open→Ok、live+无监听→超时且子进程被回收） |
| `ensure_tunnel` 失败时无条件 `cleanup_remote` 删除 profile 共享 setup 目录 | `ensure_tunnel`/`save_state`/`cleanup_remote` 整体删除（不再有"创建即部署隧道"的路径）；`SSHClient::warm()` 收敛为纯部署（返回 IL 路径，不建隧道、不写状态）。共享目录只由 `tunnel stop` 的既有归属决策回收 |
| 远端 daemon 端口从未解析（`remote_bridge_port = cfg.port` 假设错误） | `SSHClient` 移除 `remote_bridge_port` 字段；`open_tunnel(local, remote)` 显式双端口。`tunnel start` 诚实报告「已部署，尚未就绪」（`daemon_started:false` + `next` 指引 attach）；attach 发现并验证 session 实际监听端口（`live.port`）后转发，并记录 `start_identity`。三态语义收敛：已有 session→attach 发现验证；固定端口启动→由 target 显式 `port` 约束 + attach 过滤；仅部署→start 报告未就绪 |

`port_explicit` 补入全部 `Config { .. }` 字面量构造点（约 10 处，编译恢复）。

### 5.3 四轮审阅修复（转发持有者证明 / ControlMaster 禁用 / restart 语义 / digest 补字段）

针对第 4 轮审阅 4 项问题（本轮，代码已改、未提交）：

| 问题 | 修复 |
|---|---|
| `wait_for_forward` 在 SSH 握手期（未尝试绑定前）被已有服务骗过「子进程存活 + 端口可达」 | 成功判定改为「ssh 自身 stderr 绑定标记 + 端口可达 + 子进程存活」三条件：`try_ssh_tunnel` 增加 `-v`，ssh 绑定成功后打印 `Local forwarding listening on 127.0.0.1 port <n>`（`ExitOnForwardFailure` 保证绑定失败即退出）；`wait_for_forward` 后台线程持续排空 stderr 防管道填满，匹配该标记才算成功。原「sleep 30 + 独立监听器→Ok」的误设测试改为「子进程打印绑定标记→Ok」，并补「无关端口已监听 + ssh 延迟失败→拒绝」测试 |
| 记录的 PID 未必是转发持有者（ControlMaster=auto/ControlPersist=600 可能与假设冲突） | `try_ssh_tunnel` 显式加 `-o ControlMaster=no`，即使 `~/.ssh/config` 开启复用也不影响：spawn 出的 ssh 进程就是唯一的转发持有者，其 PID/start_identity 可验证 |
| `restart` 执行 stop→start，而 start 现仅部署，断开后不恢复连接 | `restart` 先做无副作用预检 `discover_live_session()`（同 attach 的发现+归属校验），无存活 daemon 时在断开前拒绝；预检通过后 stop→attach（重新发现并建立转发）。`discover_live_session` 抽为 attach/restart 共用 |
| `Config::digest` 未含 `port_explicit`（同端口数值下两种约束摘要相同） | digest 哈希输入补入 `port_explicit`；tests/config.rs 新增测试：仅切换 `port_explicit` 摘要必须变化、相同配置摘要确定性 |

### 5.4 四轮修复的复检加固（本轮，代码已改、未提交）

对 5.3 落地代码做静态复检 + 编译门禁后追加的 4 处加固：

| 问题 | 修复 |
|---|---|
| `-v` 并非无条件生效：OpenSSH 只在用户配置未设 `LogLevel` 时才被 `-v` 抬高，`~/.ssh/config` 里的 `LogLevel QUIET` 会静默掉绑定标记，导致每次 attach 必超时 | 追加 `-o LogLevel=DEBUG1`（命令行 `-o` 优先级高于配置文件），标记输出不再受用户配置影响 |
| `wait_for_forward` 固定 50×100 ms = 5 s 轮询上限，慢速跳板机握手来不及完成即误判超时；为覆盖慢链路而放大又会线性拖长「超时回收」单测 | 轮询预算参数化：`wait_for_forward(child, port, budget)`。生产走 `TUNNEL_FORWARD_BUDGET = 20 s`（失败路径由子进程退出立即返回，不依赖预算耗尽）；单测各自传短预算（5 s / 500 ms），整组仍约 3 s |
| 绑定标记未按端口匹配：ssh 若在别的端口上完成绑定，也可能被当成目标端口的证明 | 标记串含端口（`…listening on 127.0.0.1 port <n>`），新增单测 `wait_for_forward_ignores_bind_marker_for_another_port` 固定该行为 |
| `String::from_utf8_lossy(&log.lock().unwrap()[..])` 借用临时 `MutexGuard`（E0716，编译不通过）；且 `wait_for_forward` 头部残留两份重复文档块 | 抽出 `snapshot()` 辅助函数返回自有 `String`（并对 poisoned mutex 降级取值）；合并重复文档块 |

## 6. 待办（下一步）

- 迁移 session 命令族（list/show/current/cleanup/history）到 `&CommandContext`；
- 迁移 spectre/runner 与其余 `VirtuosoClient::from_env()` 调用点（76 处总量）；
- 删除 `main()` 的 `VB_TARGET` env 桥接 + `config.rs` 的 `VB_TARGET` 分支
  （全部命令族迁移完成后）；
- `config_digest` 接入 daemon Hello 校验（P0-B 前哨，F05）。
