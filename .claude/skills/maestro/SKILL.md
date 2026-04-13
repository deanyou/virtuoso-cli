---
name: maestro
description: Maestro (ADE Assembler) session management and simulation. Use when: running simulations via Maestro, configuring tests/analyses/outputs, reading results. Detailed API docs see `references/maestro-skill-api.md`.
allowed-tools: Bash(*/virtuoso *)
---

# Maestro (ADE Assembler) — Quick Reference

## 关键模式区别

| 窗口标题 | 模式 | 能否修改/运行仿真 |
|---------|------|------------------|
| `ADE Explorer Reading: ...` | 只读 | ❌ |
| `ADE Assembler Editing: ...` | 编辑 | ✅ |

## 快速流程

```bash
# 1. 确认窗口模式
virtuoso skill exec 'hiGetWindowName(hiGetCurrentWindow())'

# 2. 获取 session
virtuoso skill exec 'axlGetWindowSession(hiGetCurrentWindow())'

# 3. 添加输出
virtuoso skill exec 'maeAddOutput("VOUT" "TEST" ?outputType "net" ?signalName "/VOUT" ?session "fnxSessionX")'

# 4. 保存
virtuoso skill exec 'maeSaveSetup(?lib "LIB" ?cell "CELL" ?view "maestro" ?session "fnxSessionX")'

# 5. 运行
virtuoso skill exec 'maeRunSimulation(?session "fnxSessionX")'

# 6. 读结果
virtuoso skill exec 'maeOpenResults(?history "HISTORY_NAME")'
virtuoso skill exec 'maeGetOverallSpecStatus()'
```

## 常见问题

详见 `references/troubleshooting.md` 和 `references/maestro-skill-api.md`

### 锁文件导致打不开编辑模式

```bash
# 删除锁文件
virtuoso skill exec 'system("rm -f /path/to/library/cell/maestro/maestro.sdb.cdslck")'
```

### 窗口是 Reading 模式

```bash
# 关闭后用 "a" 参数重新打开
virtuoso skill exec 'foreach(w hiGetWindowList() when(rexMatchp("ADE" hiGetWindowName(w)) hiCloseWindow(w)))'
virtuoso skill exec 'deOpenCellView("LIB" "CELL" "maestro" "maestro" nil "a")'
```

### maeAddOutput 成功但 maeGetResultOutputs 返回 nil

"Save" 复选框无法通过 SKILL 启用。需要手动在 GUI 中勾选，或使用标量表达式输出。
