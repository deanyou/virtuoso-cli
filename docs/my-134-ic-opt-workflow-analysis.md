# MY-134：HAIDERZz/IC-opt-workflow 集成分析

分析对象：`HAIDERZz/IC-opt-workflow`，分析提交 `48e287b`（2026-08-13）。
本报告只基于该提交和当前 `virtuoso-cli` 工作树代码，不修改外部仓库或生产代码。

## 结论

建议**借鉴任务分发、不可变配置和审计报告模式，暂不深度集成对方完整优化器**。对方的 requirement-driven workflow 与本仓库的 Maestro 原子执行层互补，但它的 OpenBox/TuRBO、项目文件模型和 Spectre/OCEAN 编排层较大，直接移植会重复已有 Rust/skill 能力并引入 Python 运行时、优化器依赖和 Cadence 环境耦合。

优先级最高的是在本仓库增加一个轻量的 Python workflow skill：读取声明式需求，生成带哈希的 execution brief，调用现有 `vcli maestro`/`vcli skill`/`vcli spectre` 能力，最后由 manifest 驱动验证。优化算法、多 testbench 和多 corner 应在需求明确后另立阶段。

## 代码证据

### 1. Agent 任务简报与边界

- `src/hermes_workflow/package.py:84-116` 将已校验的 contract bundle 和 manifest payload 渲染成 `Execution Agent Task`。
- `src/hermes_workflow/package.py:122-124` 明确规定 tool-side action 只能使用 `virtuoso-bridge-lite` skills，并限定为检查/导出 Maestro testbench 和放置 `netlists/exported/input.scs`。
- `src/hermes_workflow/package.py:159-169` 要求保留 Maestro 设置、只模板批准变量、停止等待 supervisor，并将命令退出码排除在验收证据之外。
- `src/hermes_workflow/package.py:171-187` 将 preflight、approval 和 `supervisor_instruction.json` 设为真实 Spectre 执行前的门禁。
- `src/hermes_workflow/optimizer_task_package.py:185-229` 为优化执行包生成命令、审计命令、预算、并行度、后端和所需返回 artifacts。

这部分最值得吸收：简报既是交接文档，也是 policy 和 verifier 的输入，能避免 agent 越权修改仿真 setup。

### 2. SHA-256 manifest

- `src/hermes_workflow/package.py:80-81` 对文件内容计算 SHA-256。
- `src/hermes_workflow/package.py:234-247` 复制配置文件并记录每个 `config/*` 的 hash。
- `src/hermes_workflow/package.py:252-265` 写入 `execution_manifest.json`，再生成 `EXECUTION_TASK.md`。
- `src/hermes_workflow/requirement_intake.py:493-500` 对准备后的 `input.scs` 计算 hash；`584` 和 `659` 记录 constraints 文本 hash。

这比单纯记录路径更可审计，适合本仓库的远程 tunnel、Maestro 导出和 Spectre netlist 交接。

### 3. 声明式 requirement 契约

- `src/hermes_workflow/requirement_intake.py:51-80` 定义 optimize 和 fix-run 所需 section。
- `src/hermes_workflow/requirement_intake.py:81-100` 声明受管配置文件及可选的 testbench、corner、fixed points、waveform、history warm-start 文件。
- `src/hermes_workflow/requirement_intake.py:114-117` 将 backend 限定为 `maestro_exported_spectre_deck`。
- `src/hermes_workflow/requirement_intake.py:584-659` 校验 requirement、语义约束、批准字段并生成 intake report。

该模型把指标、约束、资源和批准状态从 CLI 参数中移出，适合借鉴；但对 `virtuoso-cli` 应先做小型 schema，避免复制整套 Pydantic/YAML 模型。

### 4. optimize 与 fix_run

- `src/hermes_workflow/optimizer_flow.py:134-190` 的 optimize flow 先校验 requirement、real 模式下运行 doctor，再 prepare、validate，后续才进入 execution package 和优化运行。
- `src/hermes_workflow/optimizer_task_package.py:106-156` 根据 optimizer 配置解析 OpenBox/native TuRBO backend，检查预算、continuation 与 warm-start 冲突。
- `src/hermes_workflow/fix_run_flow.py:299-325` 明确 fix_run 必须是真实执行且 requirement mode 必须为 `fix_run`。
- `src/hermes_workflow/fix_run_flow.py:327-375` 先 prepare、doctor、build package、approve，再逐个 fixed point 建 candidate request。

两种模式的生命周期和产物边界清晰。当前 `virtuoso-cli` 已有 Maestro 原子操作，但没有同等级的 requirement intake 和 run supervisor。

### 5. history warm-start、多 testbench、多 corner

- `src/hermes_workflow/history_warm_start.py:89-133` 定义 warm-start audit 和 accepted/rejected observations。
- `src/hermes_workflow/optimizer_task_package.py:144-149` 禁止 continuation 与 history warm-start 同时启用。
- `src/hermes_workflow/multi_testbench_aggregation.py:103-145` 遍历 corner 和 testbench child runs，汇总 metric rows 和失败状态。
- `src/hermes_workflow/multi_testbench_aggregation.py:166-180` 对各 corner 重新计算 objective/constraint 状态，再选择 nominal 或 worst-case 结果。

这些能力成熟度高，但依赖完整的 project/config/run manifest 体系，暂不适合直接搬入 Rust CLI。

## 与 virtuoso-cli 的差异矩阵

| 能力 | 重叠 | IC-opt-workflow 有、当前仓库缺少 | 当前仓库有、IC-opt-workflow 未覆盖 | 判断 |
|---|---|---|---|---|
| Maestro session/变量/analysis/run | `virtuoso-cli` 的 `src/commands/maestro.rs:12-120` 与 `src/client/maestro_ops.rs:25-80` 提供原子 SKILL 调用；对方把这些当成上游导出来源 | 对方增加 requirement 驱动的生命周期和 approval gate | Rust 类型化命令、transport 错误和 skill_ok 语义 | 复用本仓库原子层 |
| Spectre deck | 两者都处理 Maestro 导出的 Spectre deck | 对方有 execution package、prepare 和 manifest 交接 | 本仓库有 Rust Spectre/netlist 模块和相关 skills | 借鉴 brief，不移植 adapter |
| 声明式需求 | 都有 workflow/skill 文档，但没有同等契约 | `opt_requirement.md`、YAML config、Pydantic 严格校验 | 本仓库主要是 clap 参数和 skill 文档 | 值得新增轻量 schema |
| Execution Agent Task | 本仓库 skill 已有操作说明 | 对方生成带资源、变量、禁令、preflight 的任务包 | 本仓库缺少固定任务包和 manifest | 高优先级借鉴 |
| SHA-256 审计 | 本仓库有历史/运行数据，但该流程没有统一 immutable config manifest | 配置、constraints、input.scs hash 与报告关联 | 本仓库已有错误和 session 状态结构 | 高优先级借鉴 |
| OpenBox/TuRBO | 无直接对应 | 完整 optimizer backend、continuation、history warm-start | 本仓库有模拟执行原子能力 | 暂不集成 |
| multi-testbench/corner | Maestro 有 session 概念，但无聚合监督器 | child run、worst-case/nominal policy、aggregate manifest | 本仓库有 PVT/sweep skill，但不是同一 run contract | 后续独立设计 |
| fix_run | 可用现有 simulation 命令拼装 | fixed points、waveform manifest、独立 fix_run report | `maestro export-waveform` 与 CSV parser 可作为底层 | 可先做轻量编排 |

## 按价值排序的建议

### P0：Execution brief + immutable manifest

**范围**：新增 Python skill 或 `.claude/skills/ic-opt-workflow/` 的编排脚本；输入一个小型 `opt_requirement.md`，输出 `execution_package/EXECUTION_TASK.md` 和 `execution_manifest.json`。brief 只允许现有 `vcli`/Virtuoso skills 做 inspect/export，明确禁止改 setup、直接跑 real simulation 和伪造验收报告。

**风险**：如果 brief 允许自由 shell 或直接执行 Spectre，policy 会失效；如果 hash 只覆盖路径不覆盖内容，审计无意义。

**验证**：固定 fixture 生成稳定 schema；修改 requirement 或导出的 input.scs 后 hash 必须改变；缺少批准字段时必须在执行前失败。

### P1：把 Maestro 导出接入可验证 handoff

**范围**：调用已有 Maestro 命令获取 session、analysis、变量和 waveform/CSV；生成 deck 后记录 `input.scs` hash、来源 cellview、session、corner 和导出时间。Rust 继续负责原子 RPC，Python 负责顺序和报告。

**风险**：不同 IC23/IC25 签名和远端路径；Maestro 导出成功不等于 Spectre deck 可运行。

**验证**：fake bridge fixture + `skill_ok()` 失败用例；远端导出后检查文件存在、hash、manifest 引用一致；运行现有 `maestro` 单元测试和 skill tests。

### P2：固定点 `fix_run` 编排

**范围**：先支持单 testbench、单 corner、固定参数点；复用 `maestro export-waveform` 和现有 CSV parser，输出每个 point 的 JSON manifest。暂不引入 OpenBox/TuRBO。

**风险**：参数替换可能绕过 CDF 或污染原始 netlist；波形 CSV 成功不代表指标计算成功。

**验证**：离线 fixture 覆盖成功、Spectre 失败、OCEAN 失败、缺失 CSV、manifest hash mismatch；真实环境只在用户批准后执行。

### P3：多 testbench / 多 corner 聚合

**范围**：定义 child run 目录、metric ownership、nominal/worst-case policy 和父级 aggregate manifest，再接入现有 `sim-sweep`/Maestro 能力。

**风险**：错误地把不同 testbench 的 metric 合并，或在某个 corner 失败时错误报告整体通过。

**验证**：构造矩阵 fixture（1×1、2×1、1×2、2×2），逐项验证失败传播、worst-case 选择和 metric 所属 testbench。

### P4：history warm-start 与优化 backend

**范围**：只有在 P0-P3 的 contract、manifest 和 verifier 稳定后，评估把 OpenBox/TuRBO 作为可选 Python backend；历史 observation 必须按当前变量空间、metric 定义和 run status 重新审核。

**风险**：优化器依赖、许可证和计算资源、历史数据污染、结果不可复现。直接移植会重复对方大量 Python 领域模型。

**验证**：离线 history fixture 的接受/拒绝审计、预算和 backend 冲突测试；再做小预算真实 Spectre smoke test。

## 是否值得深度集成

目前判断：**不值得全盘深度集成；值得吸收 execution brief、immutable manifest、requirement intake 和 verifier 分层。**

原因是双方边界互补：`virtuoso-cli` 的强项是 Rust 原子执行、transport、SKILL 安全和已有 Maestro/Spectre 能力；IC-opt-workflow 的强项是 Python 编排、优化策略和审计闭环。把对方 optimizer backend 直接并入会扩大依赖和运行面，且没有解决本仓库当前最直接的能力缺口。先将 P0 做成独立 skill，保持 Python 编排 / Rust 原子执行边界，再用真实 workflow 证据决定是否进入 P2-P4。

## 当前结论的限制

- 未运行对方的真实 Spectre/Maestro 流程，无法判断其在本地 PDK 和远端 tunnel 上的可用性。
- `skills/ic-opt/SKILL.md` 主要是操作约束和产物阅读规则，算法实现仍在 `src/hermes_workflow/`；本报告没有把所有 optimizer 模块逐一审计。
- 多 corner 的最终 policy 依赖项目配置；集成前必须以具体 requirement fixture 验证，不能只按模块名称推断。
