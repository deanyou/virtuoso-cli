# pstack 在 vcli 中的应用

结论：适合用于 vcli 的开发代理工作流，需要项目适配。已安装独立 `pstack-vcli`，不改变 vcli 的 CLI 功能。

分析来源为 [backnotprop/pstack](https://github.com/backnotprop/pstack)，固定上游版本
`157aae39a733135e93d8b5b19ff62c6a84b0ad56`。该仓库是 Cursor pstack 的 standalone mirror，以 Agent Skills 提供工作流及工程原则；MIT 许可证已随本技能保留。

## 适用性

| 上游能力 | vcli 应用 | 本地取舍 |
|---|---|---|
| how / investigation | 追踪 clap → command → client → transport/解析器 | 从现有源码与测试取证；分析任务保持只读。 |
| architect / model-the-domain | 定义 JSON、错误、状态和模块边界 | 遵守 Rust/Python/SKILL 分工；只在真实取舍时比较候选。 |
| bug-fix / tdd | 将故障复现和修复串成可验证步骤 | 复用现有便宜 fixture；实机不可用明确记录。 |
| interrogate / swarm | 独立检查安全边界、共享状态和 CLI 契约 | 用原生子代理，继承当前模型，文件与实机写入范围互斥。 |
| verification / principles | 真实用户路径、失败停止、证据保留 | 新增可重跑 verifier；fake 和 live 证据分开。 |

原样安装存在问题：`Poteto Mode` 等名称和 Cursor 扩展 frontmatter 非通用；默认模型 slug 与 Task 参数依赖特定 harness；一些流程默认自动开 PR、更新外部消息；缺少 `cursor-team-kit` 控制技能；部分脚本需要 Bun 并会自行安装依赖。全量复制还会与已有 `tdd`、`review` 等技能重叠。

本地版移植核心工作流及 23 条原则的任务索引，替换这些依赖并补充 vcli 约束。详细映射见
[upstream.md](../.claude/skills/pstack-vcli/references/upstream.md)。不安装 PR 自动合并、全局模型设置、Slack automations、bot UI 和 TypeScript 专项流程。

## 项目安装与使用

主体目录为 `.claude/skills/pstack-vcli/`，兼容现有 Python skill CI。
`.agents/skills/pstack-vcli` 为指向同一目录的相对符号链接，使 Codex 发现同一版本，避免双副本漂移。此入口已在本次 macOS 环境安装并读取验证；其它平台的 symlink checkout 支持未实机验证。

下一轮可输入：

```text
$pstack-vcli 分析 vcli tunnel 的会话生命周期，指出共享状态风险。
$pstack-vcli 修复这个 CLI 问题，先复现，再验证实际行为。
$pstack-vcli 审查当前 diff，重点检查 SKILL 结果、转义和重试。
```

固定验证入口：

```bash
python3 .claude/skills/pstack-vcli/scripts/verify.py --scope all
```

可分别选择 `rust`、`skills`、`smoke`，用 `--plan` 预览命令。
每次运行保留 `report.json`、每步 stdout/stderr、退出码、时间与 SHA-256。
证据目录必须为空；skill suites 在临时快照运行，输出目录不能位于技能源目录内。

smoke 从本 checkout 构建 vcli，执行 version、schema 和 session list，检查 JSON 契约；清除继承的 VB/VCLI 配置，使用独立运行目录和显式 profile。它不操作现有 Virtuoso 会话。`config check` 暂未纳入 smoke，因为它仍会读取不能用 VB 变量隔离的 legacy `~/.vcli/.env`。Cargo 正常读取工具链和构建缓存。

## 验证证据

静态格式、脚本行为、真实离线 CLI 和独立代理试用分别验证，不能互相替代。

| 检查 | 结果 |
|---|---|
| 官方 `quick_validate.py` | `.claude` 主体和 `.agents` 入口通过；全部 7 个本地 Markdown 引用存在。 |
| 新 helper 的 unittest | Python 3.9 下 17 项通过；Python 3.13 下 17 项通过；包含失败停止、超时、计划无写入、隔离及递归复制回归。 |
| 现有 GUI skill unittest | Python 3.9 下 321 项通过，在快照执行。 |
| 既有 router pytest | 单独运行 15 项，14 通过、1 失败；不由现有 unittest CI 收集。 |
| 真实 CLI smoke | 本 checkout 构建、version、schema、空 session registry 四步通过。 |
| `cargo clippy -- -D warnings` 与 `cargo fmt --check` | 通过。 |
| 完整 `cargo test` | 普通环境与沙箱外均卡住两个既有 macOS stand-in 用例；沙箱外运行 180 秒后终止，未记为通过。 |
| 排除两个用例后的 Rust 测试 | `cargo test -- --skip macos_verifies_a_real_process_by_its_executable --skip classify_agrees_with_verify_for_ssh_named_process` 退出 0，其余测试通过。 |

两个未完成的用例是 `macos_verifies_a_real_process_by_its_executable` 和
`classify_agrees_with_verify_for_ssh_named_process`，均复制 `/bin/sleep` 为 ssh stand-in 后检查进程。超时根因尚未确认。其余 Rust 检查另用明确 `--skip` 执行，不能把它作为完整测试通过。

测试过程中还处理了两处既有验证阻塞：`src/skill_finder/mod.rs` 的布尔表达式按 Clippy 建议等价简化；skill CI 声明 `pytest>=8,<9`，解决现有 `test_router.py` 的顶层导入。Python 3.13 的完整既有 suite 未在本机重跑，保留 CI matrix 验证。

额外执行 pytest 后发现既有 `test_vcli_down_rejects_skill_ops` 失败：VCLI 不可用时，`VCLI_LOAD` 的路由结果为 `local_x11`，测试期望 `rejected`。该问题在源码快照中可复现，不由本次技能移植引入，已记录但未扩大为 GUI router 产品修改。该检查的日志为 `/private/tmp/pstack-vcli-router-pytest.log`；它使本次额外回归不全绿。

本机证据：`/private/tmp/pstack-vcli-smoke-final/report.json`、
`/private/tmp/pstack-vcli-skills-py39/report.json`、
`/private/tmp/pstack-vcli-rust-unsandboxed/report.json`、
`/private/tmp/pstack-vcli-rust-filtered.log`、
`/private/tmp/pstack-vcli-clippy-final.log`。失败记录保留，不用后续成功覆盖。

已有 tracked SQLite 的 SHA-256 在测试前后均为
`59cb93dfc84263f52065dd367373e55f2959a82d53bf95f234ce402dc9928745`。

独立代理试用的是只读故障调查请求：bridge 收到成功响应，但 cell 打开失败。代理选择调查工作流，沿 `ramic_bridge.il`、`bridge.rs`、`models.rs`、`commands/cell.rs` 和 RPC dispatch 追踪；运行已有结果检查和 SKILL 构造测试，没有修改产品代码或连接实机。它正确区分源码结论、本地测试与不可用的 loopback 检查。

该调查指出现有 CLI `cell open` 仍使用 `result.ok()`，STX + `nil` 可能标为 success；普通 `Ok(JSON)` 的 error payload 也可能退出 0。RPC cell write 通过 `ok_or_exec()` 拒绝 SKILL nil。此为技能试用的源码发现，不作为本次技能安装的一部分修复，亦未声称已复现用户实机。

本次未验证真实 Virtuoso、X11、Spectre 或 PDK 能力。离线成功只覆盖所执行的边界，不能代表电路或 GUI 行为成功。
