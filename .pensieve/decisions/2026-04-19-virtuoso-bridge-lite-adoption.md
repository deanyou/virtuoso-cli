# virtuoso-bridge-lite 借鉴范围决策

## One-line Conclusion
> 采纳 Callback File IPC 和 `session-info` 命令；变量 sweep 和 SanitizingClient 暂缓。

## Context Links
- Based on: [[ramic-bridge-callback-file-ipc]] — IPC 协议变更背景
- Related: [[vcli-bridge-cli-name]] — daemon 路径混淆的起源

## Context

参考 Arcadia-1/virtuoso-bridge-lite 近期更新，评估哪些值得移植到本项目。
按 binary vs skill 原则（CLAUDE.md）筛选：原子操作/exit code 有意义 → binary；
方法论/PDK 相关 → skill；无立即需求 → 暂缓。

## Problem

bridge-lite 包含 5 项更新：
1. Callback File IPC（修复 IC23.1 平台 bug）
2. `maestro session-info` 命令（读取当前 ADE 会话状态）
3. 变量 scope/sweep 支持
4. SanitizingClient（输入净化层）
5. 其他杂项

需要决定全部采纳还是选择性移植。

## Alternatives Considered

- **全部采纳**：增加维护负担，其中变量 sweep 目前无使用场景
- **仅采纳 Callback IPC**：错过 `session-info` 这个有实用价值的命令
- **按原则筛选（采纳）**：选择性移植，零冗余

## Decision

### 采纳（进 binary）

| 项目 | 实现位置 | 理由 |
|------|----------|------|
| Callback File IPC | `ramic_bridge.il` + `src/daemon/main.rs` | 协议层修复，影响所有调用的正确性 |
| `vcli maestro session-info` | `maestro_ops.rs` + `maestro.rs` + `main.rs` | 原子 SKILL 调用 + 结构化 JSON，exit code 有意义 |

### 暂缓

| 项目 | 理由 |
|------|------|
| 变量 scope/sweep | 当前无需求，`maeSetVar` 已够用 |
| SanitizingClient | 已有 `escape_skill_string()`，无安全漏洞报告 |

## Consequence

- Callback IPC 修复了 IC23.1/RHEL8 第二次调用挂起的 bug
- `session-info` 使脚本能自动检测当前 ADE 设计，无需手动传 lib/cell/view
- Python daemon 文件（`resources/daemons/*.py`）确认为遗留，不随此变更修改
- 暂缓项可在有具体需求时重新评估

## Exploration Reduction
- What to ask less next time: "bridge-lite 里还有什么没有采纳的？" → 本文件
- What to look up less next time: Callback IPC 的临时文件路径约定 → [[ramic-bridge-callback-file-ipc]]
- Invalidation condition: IC23.1 平台 bug 被 Cadence 修复，`ipcWriteProcess` 恢复正常
