---
name: maestro
description: Maestro (ADE Assembler) session management and simulation. Use when: running simulations via Maestro, configuring tests/analyses/outputs, updating design variables, reading results.
argument-hint: [action, e.g. "run AC on fnxSession0" or "list sessions"]
allowed-tools: Bash(vcli *)
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

# 2. 获取 ADE session 名
vcli maestro list-sessions
# → ["fnxSession0"]

# 3. 确认当前有哪些 analyses
vcli maestro get-analyses --session fnxSession0
# → {"analyses": ["tran", "ac"], ...}

# 4. 设置 AC sweep 参数（IC25 支持，IC23 不支持）
vcli maestro set-analysis --session fnxSession0 --analysis ac \
  --options '{"start":"1","stop":"1e10"}'

# 5. 保存 setup
vcli maestro save --session fnxSession0

# 6. 运行
vcli maestro run --session fnxSession0

# 7. 查看仿真消息
vcli skill exec 'maeGetSimulationMessages(?session "fnxSession0")'
```

## Maestro API 实测签名

> IC23.1-64b.500 和 IC25.1 ISR7 均实测验证。签名基本一致，主要差异在 `maeSetAnalysis` 的 `?options` 参数。

### 通用的 mae* 函数（IC23 / IC25 签名相同）

| 函数 | 实测签名 | 返回值 |
|------|---------|--------|
| `maeGetSessions` | `()` | `("fnxSession0" ...)` |
| `maeGetSetup` | `(?session sessionName)` | `("setupName")` — **返回 list**，需 `car()` 取 setup 名 |
| `maeSetAnalysis` | `(setupName analysisType)` | `t` = 成功 |
| `maeGetEnabledAnalysis` | `(setupName)` | `("ac" "tran" ...)` — **positional**，不支持 `?session` |
| `maeGetAnalysis` | `(setupName sessionName)` | analysis 配置信息 |
| `maeRunSimulation` | `(?session sessionName)` | 异步，返回 run 名称 |
| `maeGetSimulationMessages` | `(?session sessionName)` | 仿真日志字符串 |
| `maeSaveSetup` | `(?session sessionName)` | `t` |
| `maeAddOutput` | `(outputName testName ?expr expr)` | `t` |
| `maeOpenResults` | `(?history historyName)` | `t` |
| `maeExportOutputView` | `(?session s ?fileName f ?view v)` | 导出 CSV |
| `maeGetAllExplorerHistoryNames` | `(sessionName)` | `("ExplorerRun.0" ...)` |

### IC25 独有的 `?options` 参数

IC25 的 `maeSetAnalysis` 支持额外的 `?options` 参数（IC23 不支持）：

```
maeSetAnalysis(setupName analysisType
  ?session sessionName
  ?enable t
  ?options (list (list "start" "1") (list "stop" "1e10")))
```

**`?options` alist 格式：**

```skill
(list (list "key1" "value1") (list "key2" "value2"))
```

对应 vcli 命令的 `--options '{"start":"1","stop":"1e10"}'`。

**已知有效字段（2026-08-06 实测 IC25.1 ISR7）：**

| 字段 | 示例值 | 效果 |
|------|--------|------|
| `start` | `"1"` | sweep 起始频率/时间 ✅ |
| `stop` | `"1e10"` | sweep 截止频率/时间 ✅ |
| `dec` | `"20"` | 每 decade 点数 | ❌ 静默丢弃，需 sed 补 netlist |
| `lin` | `"100"` | 线性 sweep 点数 | ❌ 未验证 |
| `step` | `"1u"` | 步长 | ❌ 未验证 |

> ⚠️ `dec` 不在 `maeSetAnalysis` 的可写字段白名单中，会被静默丢弃。
> workaround：`maeSaveSetup` 后用 sed 替换 netlist 中对应的 analysis 行。

## 版本检测与自动适配

vcli 通过 `getVersion(t)` 查询 daemon 返回的 IC 版本字符串，自动选择 Maestro API 路径。

**`VirtuosoVersion::is_ic25()` 现在正确返回 `true`**（2026-08-06 修复，之前硬编码返回 `false`）。

**IC25 当前与 IC23 的差异：**

| 方面 | IC23 | IC25 |
|------|------|------|
| `maeGetSetup` 返回值 | `("setupName")` — list | 同 IC23，需 `car()` |
| `maeSetAnalysis` | `(setupName type)` | `(setupName type ?session s ?enable t ?options ...)` |
| `?options` 参数 | ❌ 不支持 | ✅ 支持 start/stop 写入 netlist |

## 设计变量更新

IC23/IC25 共享两层变量命名空间陷阱：

| API | 写入位置 | 流入 netlist |
|-----|---------|-------------|
| `maeSetVar("W34" "16u")` | Maestro 内部 varList | ❌ 不影响 input.scs |
| `asiSetDesignVarList(sess newList)` | asi session 层 | ✅ 写入 `parameters ...` |

**正确 pattern：**

```skill
vcli skill exec 'let((sess vl)
  sess=asiGetCurrentSession()
  vl=asiGetDesignVarList(sess)
  vl=cons(list("W34" "16u") remove(assoc("W34" vl) vl))
  asiSetDesignVarList(sess vl))'
```

验证：`grep "^parameters" netlist/input.scs`

**IC23/IC25 下以下函数未定义，勿用：**
- `asiSetDesVar` → undefined
- `asiSetDesignVar` → undefined
- `desVar("name" val)` → 返回 nil

## AC Sweep 完整流程（IC25）

```bash
# 1. 设置 sweep 参数（只支持 start/stop）
vcli maestro set-analysis --session fnxSession0 --analysis ac \
  --options '{"start":"1","stop":"1e10"}'

# 2. 保存（生成新 netlist）
vcli maestro save --session fnxSession0

# 3. 确认 netlist 中有 sweep 参数（若无 dec 需补全）
grep "^ac " netlist/input.scs

# 4. 补 dec（如果缺失）
sed -i 's/^ac ac stop=1e10 annotate=status $/ac ac start=1 stop=1e10 dec=20 annotate=status/' \
  netlist/input.scs

# 5. 直接跑 Spectre 获取 ASCII PSF（绕开 Maestro binary PSF 读取困难问题）
spectre -format psfascii -raw ./psf netlist/input.scs

# 6. 解析 ac.ac 输出 CSV
python3 -c "
import re, math, csv
with open('psf/ac.ac') as f: c = f.read()
freqs = re.findall(r'\"freq\"\s+([-+e\d.]+)', c)
vouts = re.findall(r'\"VOUT\"\s+\(([^)]+)\)', c)
with open('/tmp/vout_ac.csv', 'w') as f:
    w = csv.writer(f)
    w.writerow(['freq_Hz','real','imag','mag_dB','phase_deg'])
    for f, v in zip(freqs, vouts):
        r,i = map(float, v.split())
        mag = 20*math.log10(math.sqrt(r*r+i*i))
        phase = math.degrees(math.atan2(i, r))
        w.writerow([f, r, i, f'{mag:.4f}', f'{phase:.4f}'])
print(f'{len(freqs)} points exported')
"
```

## 全新 cell 的前置步骤（ensure_maestro_view）

> ⚠️ `vcli maestro open` / `deOpenCellView("a")` 假设 maestro view **已经存在磁盘上**。
> 对于从未在 Maestro 中打开过的全新 cell，该目录不存在，`deOpenCellView` 返回 nil 并弹出
> **"Data file does not exist"** GUI dialog，阻塞 SKILL channel。

bootstrap 模式（两步，idempotent）：

```bash
# 步骤 1：创建 maestro view 并写入磁盘
vcli skill exec 'let((sess)
  sess=maeOpenSetup("LIB" "CELL" "maestro")
  maeSaveSetup(?session sess)
  close_session(sess))'

# 步骤 2：正常打开 GUI
vcli maestro open --lib LIB --cell CELL
```

## 常见问题

### 仿真完成但 ac.ac 只有 DCOP 点（freq=0 Hz）

原因：Maestro session 的 netlist 中 AC analysis 没有 sweep 参数，`ac ac annotate=status` 只有 annotate 关键字。

解决：见上方「AC Sweep 完整流程」第 3-4 步。

### `maeGetEnabledAnalysis` 返回 nil

检查 setup 名是否正确获取：
```bash
vcli skill exec 'maeGetSetup(?session "fnxSession0")'
# → 应返回 ("setupName")，用 car() 取 setup 名
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
