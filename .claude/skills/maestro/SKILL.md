---
name: maestro
description: Maestro (ADE Assembler) session management and simulation. Use when: running simulations via Maestro, configuring tests/analyses/outputs, updating design variables, reading results.
argument-hint: [action, e.g. "run AC on fnxSession0" or "list sessions"]
allowed-tools: Bash(virtuoso *)
---

# Maestro (ADE Assembler) — Quick Reference

## 关键模式区别

| 窗口标题 | 模式 | 能否修改/运行仿真 |
|---------|------|------------------|
| `ADE Explorer Reading: ...` | 只读 | ❌ |
| `ADE Explorer Editing: ...` | 编辑 | ✅ (IC23+，使用 mae* API) |
| `ADE Assembler Editing: ...` | 编辑 | ✅ |

## 快速流程

```bash
# 1. 确认窗口模式（新：直接用 vcli window list）
vcli window list
# → [{"name":"Virtuoso® ADE Explorer Editing: FT0001A_SH CMOP_TB maestro ...","mode":"ade-editing"}, ...]

# 或旧方式
vcli skill exec 'hiGetWindowName(hiGetCurrentWindow())'

# 2. 获取 ADE session 名
vcli skill exec 'axlGetWindowSession(hiGetCurrentWindow())'
# → "fnxSession3"

# 3. 获取 setup 名（analysis 操作需要它）
vcli skill exec 'maeGetSetup(?session "fnxSession0")'
# → ("FT0001A_SH_CMOP_TB_1")

# 4. 启用 analysis（IC25 实测签名：positional，无 ?session）
vcli skill exec 'maeSetAnalysis("FT0001A_SH_CMOP_TB_1" "ac")'
# 支持类型: "ac" | "dc" | "tran" | "noise" | "dcOp"

# 4b. 也可用 vcli 子命令
vcli --session <bridge> maestro set-analysis --session fnxSession0 --analysis ac

# 5. 确认 analysis 已添加
vcli skill exec 'maeGetEnabledAnalysis("FT0001A_SH_CMOP_TB_1")'
# → ("ac")

# 6. 添加输出
vcli skill exec 'maeAddOutput("VOUT" "FT0001A_SH_CMOP_TB_1" ?expr "getData(\"/VOUT\")")'

# 7. 保存
vcli skill exec 'maeSaveSetup(?session "fnxSession0")'

# 8. 运行（需要 Xvfb 已安装）
vcli skill exec 'maeRunSimulation(?session "fnxSession0")'

# 9. 查看仿真消息/错误
vcli skill exec 'maeGetSimulationMessages(?session "fnxSession0")'

# 10. 等待完成后导出结果
vcli skill exec 'maeExportOutputView(?session "fnxSession0" ?fileName "/tmp/results.csv" ?view "Detail")'
```

## IC23.1 实测函数签名

> 以下签名在 IC23.1-64b.500 环境下实测验证。IC25 可能有差异。

| 函数 | IC23.1 实测签名 | 注意 |
|------|----------------|------|
| `maeGetSessions` | `()` | 无参 |
| `maeIsValidMaestroSession` | `(sessionName)` | positional |
| `maeGetSetup` | `(?session sessionName)` | keyword，返回 list `("setupName")` |
| `maeSetAnalysis` | `(setupName analysisType)` | positional，arg2 是 type 字符串，返回 `t` 成功 |
| `maeGetEnabledAnalysis` | `(setupName)` | positional，**不接受** `?session` keyword |
| `maeGetAnalysis` | `(setupName sessionName)` | 两个 positional |
| `maeRunSimulation` | `(?session sessionName)` | keyword，异步，返回 run 名称如 `"ExplorerRun.0"` |
| `maeGetSimulationMessages` | `(?session sessionName)` | keyword |
| `maeGetAllExplorerHistoryNames` | `(sessionName)` | positional，**不接受** `?session` |
| `maeOpenResults` | `(?history historyName)` | keyword |
| `maeSaveSetup` | `(?session sessionName)` | keyword |
| `maeExportOutputView` | `(?session s ?fileName f ?view v)` | keyword |
| `maeAddOutput` | `(outputName testName ?expr e)` | mixed |
| `maeSetVar` | `(name value)` | positional，无 session 参数 |
| `maeGetVar` | `(name)` | positional，无 session 参数 |
| `maeSetDesign` | `(?session s ?libName l ?cellName c ?viewName v)` | keyword |

## 版本检测与自动适配

vcli 自动检测 Virtuoso IC 版本（IC23 vs IC25），并使用对应的 SKILL API 签名。检测通过 `getVersionString()` 实现，结果缓存在内存中。

**版本差异关键点：**

| 函数 | IC23 | IC25 | 差异 |
|------|------|------|------|
| `maeGetSetup` | 返回 list `("setupName")` → 需要 `car()` | 返回 string `"setupName"` | IC25 下 `car()` 返回 nil |
| `maeSetAnalysis` | `(setupName type)` positional | `(type ?session s ?enable t ?options \`(...))` keyword | IC25 不接受 setup name 作为第一个参数 |
| `maeGetEnabledAnalysis` | `(setupName)` positional | `(?session s)` keyword | IC25 用 `?session` 取代 setup name |
| `maeSetAnalysis ?options` | 不支持通过 CLI 配置 | 支持 JSON → backtick alist | IC25 需要 `?options` 配置 sweep 参数 |

CLI 的 `set-analysis` 命令在 IC25 下支持 `--options` 参数：
```bash
vcli maestro set-analysis --session fnxSession0 --analysis ac --options '{"start":"1","stop":"10G","dec":"20"}'
```

## 设计变量更新（IC23 关键陷阱）

IC23 中存在**两层变量命名空间**，必须分清：

| API | 写入位置 | 是否流入 netlist |
|-----|---------|----------------|
| `maeSetVar("W34" "16u")` | Maestro 内部 varList | ❌ 不影响 input.scs |
| `asiSetDesignVarList(sess newList)` | asi session 层 | ✅ 写入 `parameters ...` |

`maeSetVar` 会返回 `t`，`maeGetVar` 也能读回新值——但仿真仍然用旧值。只有 `asiSetDesignVarList` 才真正改变 netlist。

**IC23 正确 pattern：**

```skill
vcli skill exec 'let((sess vl)
  sess=asiGetCurrentSession()
  vl=asiGetDesignVarList(sess)
  vl=cons(list("W34" "16u") remove(assoc("W34" vl) vl))
  asiSetDesignVarList(sess vl))'
```

- `cons(newEntry removeOldEntry)` — 替换已有变量（assoc 定位旧项，remove 删除，cons 前插新项）
- 更新多个变量时重复 `cons(...)` 链即可
- 更新后用 `vcli maestro save --session <name>` 持久化

**IC23.1 下以下函数未定义，勿用：**
- `asiSetDesVar` → `*Error* eval: undefined function`
- `asiSetDesignVar` → 同上
- `desVar("name" val)` → 通过 bridge 调用返回 nil（缺少 ADE session 上下文）

验证变量已写入 netlist：检查最新 `input.scs` 的 `parameters ...` 行，而非相信 `maeGetVar` 的返回值。

## 常见问题

### maeGetEnabledAnalysis 在 IC23.1 下签名与 IC25 文档不同

IC23.1 实际只接受 positional `(setupName)`，IC25 使用 `?session` keyword。
vcli 自动按版本选择正确的签名，无需手动干预。

### 没有 analysis (EXPLORER-9059)

```bash
# 先获取 setup 名
vcli skill exec 'maeGetSetup(?session "fnxSession0")'
# 再启用 analysis
vcli skill exec 'maeSetAnalysis("YOUR_SETUP_NAME" "ac")'
```

### 锁文件导致打不开编辑模式

```bash
vcli skill exec 'system("rm -f /path/to/library/cell/maestro/maestro.sdb.cdslck")'
```

### 窗口是 Reading 模式

```bash
vcli skill exec 'foreach(w hiGetWindowList() when(rexMatchp("ADE" hiGetWindowName(w)) hiCloseWindow(w)))'
vcli skill exec 'deOpenCellView("LIB" "CELL" "maestro" "maestro" nil "a")'
```

### maeAddOutput 成功但 maeGetResultOutputs 返回 nil

"Save" 复选框无法通过 SKILL 启用。需要手动在 GUI 中勾选，或使用标量表达式输出。
