# MY-138：virtuoso-bridge-lite 上游更新移植评估

分析对象：`Arcadia-1/virtuoso-bridge-lite`，HEAD `168943ca917e592c492a0db1b70aca0544f311c0`（2026-09-28）。
本报告只做代码差异分析，不修改生产代码。

## 结论

建议按以下顺序跟进：

1. **优先移植 Maestro history lock**：这是当前明确能力缺口，且与已有 Maestro result export 直接相关。
2. **精确补齐 Maestro session fail-closed 语义**：本仓库已有一 RTT probe，但没有上游那样的结构化 inventory、严格重复/冲突判定和明确 `unknown` 状态。
3. **nonce/IPC/daemon 隔离做差距确认即可**：本仓库已有较强 Rust 实现，未发现需要立即移植的上游功能。
4. **SOS 只保留调研项**：它依赖 Cliosoft 环境，本仓库没有同类依赖。
5. **schematic manifest 单独立项评估**：价值很高，但范围远大于一次移植，不应随本任务开工。

## 逐项 diff

### 1. Fail-closed Maestro session state（建议补齐）

上游新增 `src/virtuoso_bridge/virtuoso/maestro/reader/state.py`：

- `:19-24` 用严格 ADE 标题正则，无法匹配时 `parse_maestro_title()` 在 `:70-83` 返回 `None`，不猜测 lib/cell/view。
- `:86-105` 一次 SKILL probe 同时收集当前窗口、窗口列表、`axlGetWindowSession`、`davSession` 和 `maeGetSessions()`。
- `:116-172` 对返回 envelope、窗口 ID、重复 session、空值进行结构化校验。
- `:189-225` 检查 `axlGetWindowSession` 与 `davSession` 冲突；标题无法识别时保留诊断而不补全设计字段。
- `:228-266` 将 GUI window session 与 headless session 分开表示；无窗口绑定的 session 进入 `headless` 并带有“无法区分后台/陈旧 GUI session”的诊断。
- `:276-325` 精确查询 session 时，找不到返回 `not_found`，多窗口绑定返回 `unknown`，当前窗口多重匹配也返回 `unknown`。

本仓库已有：

- `src/client/maestro_ops.rs:266-275` 的一 RTT probe，返回 title、`davSession`、所有窗口标题、session 列表和 run_dir。
- `src/commands/session.rs:119-180` 的 tunnel/session 六步验证链，失败时跳过在线探测并展示缓存数据。
- `src/transport/ipc/messages.rs:60-64,138-165` 的 challenge 和 daemon nonce。

差距判断：本仓库 probe 的数据字段接近，但返回值仍是未分层的 SKILL list，缺少上游的严格 envelope 校验、标题解析模型、session/window 冲突状态和 `unknown` 语义。上游也不是“标题不识别就所有情况返回 None”：`state.py:197-210` 在 session 已知时仍返回 GUI state，但不填充设计字段；真正的 fail-closed 体现在歧义不自动选择。

**建议改动层**：Rust `src/client/maestro_ops.rs` 增加带 marker 的结构化 probe；Rust command/model 层增加 `MaestroSessionState`、`unknown`/`not_found` 等显式状态；SKILL 只负责一次采集，不在 SKILL 中猜测。也可以先在 Python skill 层实现验证规则，作为兼容过渡。

**风险**：严格标题正则会暴露不同 IC 版本标题差异；把已有缓存探测改成 fail-closed 可能改变用户看到的在线/缓存切换行为。

**验证**：fake probe fixture 覆盖 malformed envelope、重复 window、axl/dav 冲突、未知标题、多窗口同 session、headless session；验证任何歧义都不选择目标窗口。

### 2. Maestro history locks（最高优先级）

上游 `src/virtuoso_bridge/virtuoso/maestro/history.py`：

- `:33-55` 定义不可变 `MaestroHistory` 和 `MaestroHistoryLockResult`，结果包含 before/after、changed 和三种 outcome。
- `:64-113` 用 `axlGetMainSetupDB`、`axlGetHistory`、`axlGetHistoryEntry`、`axlGetHistoryLock` 发现 history；API 不存在或 payload 异常时明确返回 unsupported/probe_error/malformed。
- `:116-128` 用 `maeSetHistoryLock` 修改指定 session/history，并带 marker。
- `:135-178` 严格解析 history 与 lock payload，拒绝重复、空名称和非布尔锁状态。
- `:188-218` 所有读取操作都要求显式 session 和精确 history 名称。
- `:221-300` 先读 before，已满足时返回 `already_satisfied`；修改后再次读取 after 验证；传输不确定时不重试 mutation，仅在 after 已满足时返回 `confirmed_after_unknown`。

本仓库只有：

- `src/client/maestro_ops.rs:300-305` 支持按 history 名称导出 CSV；没有 `axlGetHistory`/`axlGetHistoryLock` 的操作封装。
- `src/history.rs` 是 CLI 命令历史日志，不是 Maestro result history。

差距判断：这是明确 gap。history lock 能防止重要仿真结果被后续运行覆盖，且上游的“不确定 mutation 不重试、必须读回确认”与本仓库的 transport/SKILL 失败语义一致。

**建议改动层**：优先 Rust `src/client/maestro_ops.rs` 增加 probe/set-lock builder；`src/commands/maestro.rs` 增加 list/get/lock 命令和结构化 JSON；通过 `skill_ok()` 检查 SKILL nil；必要时在 RPC schema/MCP 暴露只读 list 与显式 lock 操作。不要复用 `src/history.rs`。

**风险**：`axl*` API 可能只在部分 ADE/IC 版本可用；lock 是有副作用的操作，误 session 或误 history 会影响后续仿真。传输超时后的状态必须报告 unknown，不能自动重试。

**验证**：fake bridge 覆盖 unsupported、before 已锁、mutation 成功后 after 匹配、mutation transport unknown 但 after 匹配、unknown 且 after 不匹配、post-read 失败；真实环境先只读 list，再对明确 history 做人工批准的 lock smoke test。

### 3. SOS cellview 操作（可选调研）

上游 `src/virtuoso_bridge/virtuoso/sos/cellview.py`：

- `:22-76` 定义 checkout/checkin/status 的结果模型和 outcome。
- `:79-89` 严格限制单行字符串和绝对远端路径，拒绝 `..`。
- `:92-140` 在同一 SKILL 表达式内检查 SOS library、cellview、master path、view type、dirty 状态，并在写操作前重新确认目标没有变化。
- `:143-220` 提供 native GUI fallback，同时检查 modal form、唯一目标行和预期状态。

本仓库没有 SOS/Cliosoft 模块或 command。其 transport、transaction 和 schematic 能力不能直接替代 SOS 版本管理。

**建议**：不移植；只在用户明确使用 Cliosoft SOS 时另立可选 skill。改动层应是 Python 编排 + 专门 SKILL/GUI adapter，不能默认加入 Rust 核心。

**验证**：需要真实 SOS 环境，覆盖 unmanaged library、dirty cellview、状态变化和 checkout/checkin 后读回状态。当前环境无法验证，标记为“需验证”。

### 4. Nonce、IPC 日志与 daemon 隔离（当前大体已覆盖）

上游变更涉及 daemon nonce 可配置、IPC 日志加固和共享服务器 daemon 隔离。本仓库证据显示：

- `src/transport/ipc/messages.rs:60-64` 已有 `Challenge` operation。
- `src/transport/ipc/messages.rs:138-165` 的 `HelloAck.daemon_nonce` 和 `RequestEnvelope.daemon_nonce` 明确要求每请求回显 nonce，daemon 重启使旧客户端失效。
- `src/transport/daemon_lifecycle.rs:33-81` 用 endpoint、token、profile 和 nonce 做 Tier-1 liveness proof；任何失败都返回 false。
- `src/transport/daemon_lifecycle.rs:84-135` shutdown 前重复 challenge，避免把旧 socket 或错误进程当成目标。
- `src/commands/session.rs:119-180` 对 remote tunnel 做 session ownership、attached session、host/port、context、进程身份和端口可达性检查。

差距判断：本仓库的 nonce 和多 session/动态端口隔离已经是 Rust 原生实现，上游更新不应直接回移植。IPC 日志的具体差异需要查看上游对应 commit 的 logging diff 后再决定，当前不构成立项依据。

**建议改动层**：只做逐项 audit；若发现缺口，优先 Rust daemon/IPC 层。验证应是 stale nonce、同 socket 换进程、共享服务器不同 profile、并发 session 的集成测试。

### 5. Schematic manifest 三件套（高价值、单独立项）

上游 `src/virtuoso_bridge/virtuoso/schematic/manifest.py`：

- `:1-11` 明确输入是 process-independent source geometry + process map，禁止推断 device/placement intent。
- `:51-54` 定义 manifest/process-map schema 版本。
- `:103-121` 对 instance/port/junction/route 等集合做唯一性检查。
- `:124-180` 校验 circuit、device class、source position、transform、pins、nets 和 source terms。
- `:192-285` 校验 bounds、port occurrences、junctions、route endpoints、bend steps 和 net 一致性。
- `:287-360` 继续校验 contacts、bulk labels、annotation stubs 的引用完整性。

本仓库当前 schematic 读取/导出：

- `src/client/schematic_ops.rs:125-145` 的 `list_instances` 只返回 name/master/x/y。
- `src/client/schematic_ops.rs` 的 `list_nets`/`list_pins` 只返回 net 名称和 terminal direction。
- `src/commands/schematic.rs:471-520` 分别调用这些读取操作并解析 JSON。
- `src/commands/schematic.rs:523-543` 的 `schematic export` 只组合 instances/nets/pins，尚无 source geometry、routes、junctions、端点和 process map。

差距判断：差距很大，但它与 schematic-to-Visio、transaction snapshot、跨 PDK 导入有共同基础。应先做 schema/读取能力设计评审，不在 MY-138 直接开工。

**建议改动层**：Rust 负责安全、只读 schematic 数据 RPC；Python skill 负责 manifest 校验、process map 和编排；SKILL 负责读取精确几何。验证需要 fake cellview fixture、跨 PDK pin map、旋转/镜像、路线端点和 transaction rollback。

## 排序后的移植清单

| 优先级 | 项目 | 建议 | 改动层 | 主要风险 | 验证 |
|---|---|---|---|---|---|
| P0 | Maestro history locks | 立项实施 | Rust Maestro ops/commands，必要时 RPC/MCP | API 版本、误锁、transport unknown | fake bridge + 人工批准 real smoke |
| P1 | Fail-closed session state | 精确 diff 后实施 | Rust probe/model，Python skill 可过渡 | 标题格式差异、行为变化 | malformed/ambiguous 多 session fixtures |
| P2 | Schematic manifest | 单独 PRD/设计，不开工 | Rust read RPC + Python schema/编排 | 范围大、几何语义不完整 | schema/geometry fixture |
| P3 | Nonce/IPC/daemon | 审计即可 | Rust daemon/IPC（仅发现缺口才改） | 重复实现、破坏现有协议 | stale nonce/profile/concurrency tests |
| P4 | SOS | optional 调研 | Python/SKILL adapter | Cliosoft 环境不可得 | 真实 SOS 环境，当前需验证 |

## 不能直接推出的结论

- 上游 fail-closed state 并不等于任何未知标题都返回 `None`；已知 session 仍可返回带 diagnostics 的 GUI state，不能把实现简化成“标题解析失败即拒绝所有操作”。
- 上游 SOS 代码不能证明本仓库需要 SOS 支持。
- 上游大规模 schematic manifest 不能由当前 `schematic export` 的三类数组直接实现；必须先补几何、端点和 process map 数据契约。
