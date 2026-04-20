# Maestro 仿真触发 API 在 IC23.1 上阻塞 bridge

## One-line Conclusion
> IC23.1 ADE Explorer 的所有仿真触发 API 均阻塞 SKILL bridge；`set-var` 等读写操作安全，`run` 类操作不可用于自动化。

## Context Links
- Based on: [[ramic-bridge-callback-file-ipc]] — bridge 通信机制
- Based on: [[2026-04-19-standalone-spectre-for-one-off-verification]] — 可行的替代路径
- Related: [[maestro-session-types]] — Maestro session 结构

## Context

在 `vcli maestro run --session fnxSession0` 实测中发现，IC23.1 上所有仿真触发 API
均阻塞，包括此前未测试的 netlist 生成 API。

## Problem

以下 API 全部阻塞 bridge（bridge 30s 超时，SKILL evaluator 等待 GUI modal）：

| API | 阻塞原因 |
|-----|---------|
| `maeRunSimulation` | 触发 "ADE Explorer Update and Run" modal |
| `maeCreateNetlist` | 触发 netlisting 进度条 |
| `asiCreateNetlist` | 同上 |

以下 API 安全，不阻塞：

| API | 功能 |
|-----|------|
| `maeSetDesignVar` | 写入设计变量（`vcli maestro set-var`） |
| `maeGetDesignVar` | 读取设计变量（`vcli maestro get-var`） |
| `maeGetSessions` | 列出 session（`vcli maestro list-sessions`） |
| `maeGetAnalysisType` | 读取分析列表（`vcli maestro get-analyses`） |
| `hiGetCurrentWindow` / `cw->davSession` | 读取窗口状态（`vcli maestro session-info`） |
| `asiGetAnalogRunDir` | 读取 run 目录 |

## Alternatives Considered

- **`maeNetlistAndRun`（未测）**：推测同样阻塞，未验证
- **Virtuoso SKILL 后台进程（`ipcBeginProcess`）**：触发 spectre 的方式，绕过 SKILL evaluator；但需要自己管理进程和结果收集

## Decision

Maestro 自动化仿真路径仅限：
1. 用 `set-var` 写入设计变量
2. 用 `session-info` 获取 run_dir / netlist 路径
3. 直接调用 `spectre` 二进制（standalone，见 [[2026-04-19-standalone-spectre-for-one-off-verification]]）

不依赖 `maeRunSimulation` 或任何 netlist 生成 API。

## Consequence

- `vcli maestro run` 命令在 IC23.1 上无法在自动化场景中使用
- 变量赋值 + GUI 手动点 Run 是完整 Maestro run 的最简路径
- 独立 spectre 路径对 n12/p12 标准 CMOS 完全可行（0 error 验证）

## Exploration Reduction

- **What to ask less next time**: "有没有办法从 bridge 触发 Maestro 跑仿真？" → IC23.1 上没有。
- **What to look up less next time**: 哪些 API 阻塞 → 本文档。
- **Invalidation condition**: IC25 或更高版本引入非阻塞仿真 API；或 Cadence 修复 `maeRunSimulation` 不再弹 modal。
