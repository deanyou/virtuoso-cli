---
name: pstack-vcli
description: Evidence-driven investigation, design, bug fixing, refactoring, and review of the virtuoso-cli repository, adapted from pstack. Use for vcli engineering workflows or explicit pstack requests; circuit sizing and simulation methodology belong to the existing EDA skills.
---

# Pstack for vcli

这是 pstack 的 vcli 项目移植版。目标是通过可复现的证据选择最小改动，并验证用户实际看到的行为。它为开发代理提供工作流，不向 `vcli` 增加命令。

## 开始任务

先读项目根目录 `AGENTS.md`，查看工作区状态，保留已有修改。涉及 runtime、router、executor、verifier 或 recovery 时，再读 `.claude/skills/vcli-architecture/SKILL.md`。从相关入口追踪调用、数据类型、状态和错误传播；区分源码推断与实际运行证据。

按下面的任务类型读取 [references/workflows.md](references/workflows.md) 中对应段落。只读分析保持只读；用户已经要求实现时，分析完接着实现，不把中间结论当作交付终点。

| 请求 | 工作流 |
|---|---|
| 怎么工作、是否合适、回归原因 | 调查 |
| 设计新命令、跨函数接口修改 | 设计与功能 |
| 报错、错误结果、卡住 | 修复 |
| 重命名、去重、迁移接口 | 重构 |
| 延迟、吞吐、资源消耗 | 性能 |
| 审查 diff 或 PR | 审查 |
| 创建、安装、调整技能 | 技能适配 |

有结构选择或故障分析时，按需读 [references/principles.md](references/principles.md)。这些原则帮助做决定，不要求每次建立多套原型或完整评审团。

## vcli 约束

- `VirtuosoResult::ok()` 仅说明 transport 成功。需要 SKILL 执行成功的操作用 `skill_ok()`，通过 `VirtuosoError` 传播失败；不要引入 `anyhow`。
- 用户输入进入 SKILL 字符串前使用 `bridge::escape_skill_string()`。外部命令使用固定程序及分离参数，Rust 用 `Command::new()`，Python 用 argv 列表。
- 命令层负责参数和 JSON；SKILL 构造在 `src/client/*_ops.rs`。Rust 管执行与安全边界，Python 管工作流，SKILL 管 Virtuoso API。Router 只返回决策。
- 并发与重试涉及会话缓存、端口、SSH 和窗口时，明确共享写入范围。非幂等操作在超时或回复丢失后不能自动重放；先查询实际状态。
- `unavailable`、`skipped` 和推断不能记为 `passed`。离线 fixture/fake 证明协议或流程，不证明真实 Virtuoso GUI、PDK 或仿真行为。
- 实机诊断使用已有 `diag`、`tunnel-connect`；GUI 使用 `virtuoso-gui-debug`；电路和仿真使用已有 EDA 技能。需要时从项目 `.claude/skills/<name>/SKILL.md` 读取，不假设 Cursor 插件存在。
- `VB_REMOTE_HOST` 指运行 Virtuoso 的 compute host；`VB_JUMP_HOST` 指 SSH 跳板。不要从旧文档复制 session ID、端口、DISPLAY、库名或 PDK 路径。

## 委派与范围

有独立调查、设计或审查工作时，使用当前环境的原生子代理工具。在 Codex 中用 `collaboration.spawn_agent`，继承当前模型，按可用并发槽位分批。传文件位置、目标、可修改范围和验收条件。多个写入者不修改同一组文件，也不同时驱动共享 Virtuoso 会话；只有读源码的审查可并行。

主代理审阅实际 diff 和证据，再合并结论。没有子代理工具时顺序完成并说明审查局限。独立审查不等于不同模型审查，不宣称不存在的模型覆盖。

此技能沿用当前用户授权。创建 PR、合并、部署、对外消息和清理遵守当前任务范围与环境权限，不由工作流自动授权。普通项目内编辑和测试按已有授权继续；不为流程本身添加确认环节。

## 验证与交付

每个改动选择直接覆盖其行为的检查。最终执行项目要求的 `cargo test`、`cargo clippy -- -D warnings`、`cargo fmt --check`；技能改动还执行 Python skill suites。使用随附脚本记录命令、退出码、输出和未覆盖范围：

```bash
python3 .claude/skills/pstack-vcli/scripts/verify.py --scope all
```

脚本仅依赖 Python 标准库与项目现有工具，默认将证据保留在临时目录。`--scope rust` 执行三项 Rust 检查；`skills` 在临时副本运行现有 skill CI；`smoke` 构建本 checkout 的 vcli 并检查离线 version、schema 和空 session registry。`--output-dir DIR` 指定证据目录，`--plan` 只打印计划，不能作为验证成功。详细覆盖范围见 [references/verification.md](references/verification.md)。

报告行为变化、选择原因、验证结果及尚未验证的真实环境能力。必要时附日志路径和可复现命令。只读调查无需为交付创建 PR。

移植来源、许可证及取舍见 [references/upstream.md](references/upstream.md)。
