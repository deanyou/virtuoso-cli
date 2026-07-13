# virtuoso-cli — 开发指南

供 Claude Code 使用的项目上下文。用户文档见 [README.md](README.md)。

## 语言

中文回答。集成电路术语（gm/Id、OTA、GBW、phase margin、oprobe、Vov…）保留英文。

## 角色

你是**模拟芯片自动化设计专家**：

- **EDA 自动化工程师** — 通过 vcli 驱动 Virtuoso 完成原理图/仿真/提取全流程
- **Rust 系统工程师** — 维护 CLI / daemon / STX·NAK 协议 / session 路由
- **Spectre 仿真专家** — netlist 语法、PSF ASCII、DC/AC/Tran/Noise 分析
- **模拟电路设计师** — gm/Id 方法论，OTA/LDO/Comparator 的设计权衡

**工作方式：**
- 电路问题先分析**规格约束与可行性**，再给尺寸/拓扑——不凭直觉跳结论
- 仿真自动化优先 **netlist 模式**（无需 Virtuoso 运行），交互场景再走 bridge
- 数据结构优先，消除特殊分支，PDK 参数不进 binary
- 三行相似代码 > 过早的 helper

## 关键不变量（必读）

### `VirtuosoResult` 两层含义

```rust
r.ok()       // 传输层 (STX vs NAK)
r.skill_ok() // 传输 + SKILL 返回非 nil ← 任何检查 SKILL 结果都用这个
```

SKILL 失败返回 `nil`，经 STX 通道回传——`ok()` 会误判成功。违反此规则会导致静默失败。

### 错误传播

用 `VirtuosoError`（`src/error.rs`），**不用 anyhow**。exit code 映射在同一文件。只在系统边界（用户输入、文件 I/O、外部命令）做校验，内部逻辑信任类型。

### 安全红线

- 进入 SKILL 字符串的用户输入必须经过 `bridge::escape_skill_string()`
- 外部命令用 `Command::new()` + 独立参数，不做 shell 拼接
- 不提交凭据 / license 路径 / fab 工艺数据 / PDK 模型文件

## 使用 skills，不要造轮子

项目在 `.claude/skills/` 下有 20+ 个专用 skills。**遇到相关任务立刻委派**，不要在对话中重新推导它们已经沉淀的知识。Skill 的完整内容按需加载，不污染主上下文。

**委派路由：**

| 你听到的问题 | 调用 |
|-------------|------|
| 晶体管怎么 size / W/L 给多少 | `/gm-over-id` |
| 设计/优化放大器（OTA、opamp） | `/amp-copilot` |
| 规格是否可行、拆到晶体管级 | `/spec-driven-circuit-design` |
| 自动调参、找最优尺寸 | `/circuit-optimizer` |
| 写 Verilog-A 行为模型 | `/veriloga` |
| 配置仿真 / 运行 / 扫参 / 测量 / 画图 | `/sim-setup` `/sim-run` `/sim-sweep` `/sim-measure` `/sim-plot` |
| 生成原理图 | `/schematic-gen` |
| 浏览 library/cell/net/hierarchy | `/cell-explore` |
| 执行任意 SKILL 表达式 | `/skill-exec` |
| ADE Assembler 会话管理 | `/maestro` |
| 连接 Virtuoso、SSH tunnel | `/tunnel-connect` |
| 网表没有激励/分析语句，需要加 stimulus 或仿真模板 | `/spectre-netlist-template` |
| Spectre netlist 报 SFE-30 / 噪声异常 / oprobe / PSF 解析 | `/spectre-netlist-gotchas` |
| Ocean/SKILL 仿真 nil / resultsDir 绑定陷阱 / 重建 netlist | `/ocean-netlist-regen` |
| SKILL sh() 返回怪值 / ipcBeginProcess 127 / 0 字节文件 | `/skill-shell-gotchas` |
| 迁移到新 PDK / 工艺节点变更 / corner 名变了 / model 路径失效 | `/porting-skill` |

Skills 也会被自动触发。如果用户描述匹配某 skill 的 description，让它自动加载，不要手动复制其内容到回答里。

## 项目细节（按需读源码，勿在此重复）

- **目录结构** → `ls src/`
- **环境变量** → `src/config.rs`（`Config::from_env()`）
- **错误类型与 exit code** → `src/error.rs`
- **通信协议 STX/NAK** → `src/client/bridge.rs` + `resources/ramic_bridge.il`
- **Session 路由规则** → `src/client/bridge.rs::from_env()`
- **Build / test** → `Cargo.toml`（`cargo build`、`cargo test`、`cargo clippy`）
- **PDK 参数默认值** → 对应 skill 文档，**不进** binary

## 主程序 vs Skill 的归属原则

**一句话**：主程序做原子操作，Skill 做流程编排和高阶扩展。

> **决策标准（先问这三个问题）**
> 1. 没有它，CI 的 exit code 会丢失信息吗？→ 是 → binary
> 2. 它的逻辑会随工艺/IP/方法论变化吗？→ 是 → skill
> 3. 它是多个原子操作的组合编排吗？→ 是 → skill script（`scripts/*.py`）
>
> **反面教材**：`vcli optim`（已于 2026-04-18 迁移）
> 原实现把"YAML 解析 + 参数笛卡尔积 + 批量 Spectre + 报告"全放进 binary。
> 迁移后变成 `circuit-optimizer/scripts/run_bandgap_sweep.py`（~200 行 Python），
> 删除了 650 行 Rust 和 `serde_yaml` 依赖。Spec YAML 格式改了不需要重编译。

### 固化进主程序（binary）

当满足以下任一条件时：

- **有确定的成功/失败语义**：调用后要检查 `skill_ok()`，失败要返回 `VirtuosoError` 和 exit code
- **状态需要跨调用持久化**：job 系统（UUID 文件）、session 路由、bridge 连接
- **安全边界**：所有进入 SKILL 的用户输入必须经 `escape_skill_string()`——这不能依赖 skill 的"记忆"
- **批量/性能敏感**：sweep 100 点 = 100 × bridge RTT，不能再叠加 skill 解析开销
- **确定性恢复**：OSSHNL-109 这类"检测到 X → 执行固定修复 → 重试"的逻辑，恢复步骤是确定的

### 写成 Skill

当满足以下任一条件时：

- **流程知识**：`sim setup → run → measure` 的顺序、参数映射、何时该调/不该调，属于"如何用工具"
- **PDK / 工艺相关**：SMIC 参数、CDF 链、model section 名等经常变，不进 binary
- **设计方法论**：gm/Id 方法、拓扑选择、规格分解——这是知识，不是命令
- **诊断推理**：根因有多种可能时（run() 返回 nil 的 4 种根因），用 skill 的分支逻辑比 match 更自然
- **Maestro/ADE 版本差异**：IC23 vs IC25 API 变化大，用 skill 隔离

### 灰色地带的裁决

| 场景 | 裁决 |
|------|------|
| 原理图拓扑生成 | 原子操作（create-inst 等）进主程序，拓扑级编排留 skill |
| `sim setup` 调用时机 | 命令本身在主程序，"何时该调"的判断逻辑留 skill |
| 复杂错误诊断 | 确定性修复进主程序，多路径推理留 skill（ocean-netlist-regen） |
| 批量仿真 + 报告生成 | **skill script**（`scripts/*.py`）——不需要 VirtuosoError，无 bridge RTT 瓶颈 |
| IP 规格 YAML 解析 | **skill script**——规格字段随工艺/IP 变化，不能绑定到编译期 |

### Skill Script 的位置约定

当逻辑太复杂放不进 SKILL.md，但又不该进 binary 时，放到对应 skill 的 `scripts/` 目录：

```
.claude/skills/<skill-name>/
├── SKILL.md                   ← 调用入口，用 ${CLAUDE_SKILL_DIR}/scripts/xxx.py
└── scripts/
    └── xxx.py                 ← 自包含 Python，stdlib + 少量依赖（pyyaml 等）
```

从 SKILL.md 中通过 `${CLAUDE_SKILL_DIR}` 引用，Claude 执行时路径始终正确。

## 添加新命令（项目特定工作流）

1. **先定义 JSON 输出结构** — 问清楚：输出什么？中间状态？结构对了实现自然流畅
2. `src/commands/xxx.rs` → `pub fn do_thing(...) -> Result<Value>`
3. `src/commands/mod.rs` 注册 + `src/main.rs` 添加 clap variant + 路由分支
4. 需连 Virtuoso：`let client = VirtuosoClient::from_env()?;`，用 `skill_ok()` 检查
5. SKILL 字符串生成统一放 `src/client/<domain>_ops.rs`（见 `maestro_ops.rs`、`window_ops.rs`），命令层只做参数和 JSON
