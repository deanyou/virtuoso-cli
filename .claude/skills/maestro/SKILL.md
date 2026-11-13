---
name: maestro
description: "Maestro (ADE Assembler) session management and simulation. Use when: running simulations via Maestro, configuring tests/analyses/outputs, updating design variables, reading results."
argument-hint: '[action, e.g. "run AC on fnxSession0" or "list sessions"]'
allowed-tools: Bash(vcli *)
---

# Maestro (ADE Assembler) — Quick Reference

## 关键模式区别

| 窗口标题 | 模式 | 能否修改/运行仿真 |
|---------|------|------------------|
| `ADE Explorer Reading: ...` | 只读 | ❌ |
| `ADE Explorer Editing: ...` | 编辑 | ✅ (IC23+，使用 mae* API) |
| `ADE Assembler Editing: ...` | 编辑 | ✅ |

## 会话选择：Virtuoso 用 `VB_SESSION`，Maestro 用子命令 `--session`

两套会话命名空间是**不同的东西**，不要混用：

| 要选的东西 | 用什么 | 例子 |
|-----------|--------|------|
| 哪个 Virtuoso 实例（bridge 会话） | `VB_SESSION` 环境变量 | `VB_SESSION=xunipc-userone-38371 vcli maestro ...` |
| 哪个 Maestro 会话（ADE session） | 子命令自己的 `--session` | `vcli maestro run --session fnxSession0` |

- `export VB_SESSION=...` 是**合法用法**，不是缺陷——它就是官方文档里的写法。
- 禁止 `export` 的只有 `VCLI_CAPABILITY=admin`：admin 是提权能力，用**单命令前缀**，
  不要 export 到后续每一条命令的环境里。
- Maestro 子命令的 `--session`（`fnxSession*`）**不会**写入 `VB_SESSION`——它是 Maestro
  会话名，不是 bridge 会话 id。写进去会让客户端去找一个不存在的会话文件，然后回落到
  `VB_PORT=0`，报 `ECONNREFUSED`。
- 全局 `--session`（`vcli --session <id> maestro ...`）会按值的形态自动判定：
  - 像 bridge 会话 id（`<host>-<user>-<port>`，如 `xunipc-userone-38371`）→ 选中该
    Virtuoso 实例（写入 `VB_SESSION`）；
  - 像 `fnxSession*` → 只当作 Maestro 会话，不写 `VB_SESSION`；
  - 两者都看不出来 → vcli **直接报错**（exit 2），要求你用 `VB_SESSION` 选 Virtuoso、
    用子命令的 `--session` 选 `fnxSession`；**不会**默默连到别的实例。

> ⚠️ 全局 `--session` 和子命令 `--session` 同名：子命令位置的值会覆盖全局位置的值。
> 需要同时指定时，用环境变量选 Virtuoso：
> `VB_SESSION=xunipc-userone-38371 vcli maestro run --session fnxSession0`。

## 快速流程

> 下例假设 `VB_SESSION` 已经指向目标 Virtuoso 实例（`export VB_SESSION=...`
> 或单命令前缀 `VB_SESSION=... vcli ...`）。`--session fnxSession0` 是 **Maestro 会话**，
> 不是 Virtuoso 实例——两者别混。

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

> ⚠️ 全新 cell 的 maestro view 目录一开始不存在，直接开 GUI 会弹出
> **"Data file does not exist"** 对话框，阻塞 SKILL channel。
> `vcli maestro open --mode a` 会把 view 建出来并写入磁盘。

**只走这一条命令**（单命令前缀，不要 export admin）：

```bash
# 新建 / 打开 maestro view（后台 setup 会话，可编辑，会持有 OA edit lock）
VB_SESSION=xunipc-userone-38371 VCLI_CAPABILITY=admin VB_TIMEOUT=120 \
  vcli maestro open --lib LIB --cell CELL --mode a
```

- `VB_SESSION=...`：选哪个 Virtuoso 实例。
- `VCLI_CAPABILITY=admin`：**单命令前缀**。`maestro open --mode a` 需要 admin 能力，
  但不要 `export VCLI_CAPABILITY=admin`——那会把提权能力留在后续所有命令里。
- `VB_TIMEOUT=...`：可选。服务端忙时调大（默认 30s）。
- `--mode a` 取 edit lock；只读观察用 `--mode r`。

> ⚠️ `vcli maestro open` 开的是**后台 setup 会话**（底层就是 `maeOpenSetup`）：
> **没有 GUI 窗口**，也无法 attach 到已有窗口。所以它开出来的 "a" 模式会话会
> **占住 edit lock**，而 Virtuoso 那边看不到任何可以点的界面。
> 详见下方「窗口是 Reading 模式」。

### ⚠️ 不要用裸的 Maestro setup bootstrap

后台直接建 Maestro setup、再存盘、再关会话的那套两步 bootstrap（不经 `vcli maestro open`）
是**禁止**的。

理由：它绕过了 `vcli maestro open` 的会话与锁管理，容易留下悬空的 edit lock；
失败时它只返回 nil，没有可读的错误上下文。一切走 `vcli maestro open --mode a`。

### ⚠️ 不要用 shell 删锁

不要用 shell 的 `rm` 去删 `*.cdslck` 锁文件。

见下方「锁归属与当次授权」。

## 执行者预检（发任何 SKILL 之前）

以下三项是**执行者要自己做的预检**，不是 vcli 的自动拦截——本文档本身没有拦截效力，
绕过它直接发 SKILL 依然可能连错会话、连错机器或抢别人的锁。

1. **hosts 正确**：`VB_REMOTE_HOST` 必须指向**跑 Virtuoso 的那台机器**（compute host），
   不是 jump/bastion。检查：

   ```bash
   vcli tunnel status --format json | jq '.daemon.hostname_check'
   # {"configured": "jump-bastion-01", "actual": "compute-eda-42", "mismatch": true}
   ```

   `mismatch: true` → **停下**，先修 `VB_REMOTE_HOST`（不是 `VB_JUMP_HOST`）。
   hosts 不对时发 SKILL 只会连到错误的机器（或根本连不上）。

2. **JobManager 可达**：`vcli maestro run` 靠 JobManager 提交仿真。JobManager 不可达时
   提交会失败或挂住。**停下并报告**，不要用 SKILL 硬闯——硬闯的结果通常是一个
   看起来成功、实际没有任何结果的 run。

3. **会话归属**：确认你选中的是本次任务授权的那个 Virtuoso 实例（`VB_SESSION`）、
   那个 cellview。

任一项不通过 → **停下并报告**，不要继续发 SKILL。

## 锁归属与当次授权

- OA edit lock（`*.cdslck`）属于**当前持有者**。
- **禁止擅清锁**：不要用 shell 删 `.cdslck`，也不要为了绕开锁而换 session/换机器。
  锁在，很可能是**另一个人或另一个进程**正在编辑这个 cellview。
- 只有在**你就是该 cellview 的持有者**、对方进程已经退出、**且本次授权明确包含
  "清理锁"** 这三个条件同时成立时，才可以处理锁。
- 拿不准就**停下报告**，等授权，不要自己决定。

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

**不要用 shell 删掉 `.cdslck`。** 锁属于当前持有者；见上方「锁归属与当次授权」。
正确做法：先确认持有者与本次授权，再决定是换用只读模式
（`vcli maestro open --mode r`）、联系持有者，还是在授权明确时清理。

```bash
# 只读观察：不取 edit lock，也不影响持有者
VB_SESSION=xunipc-userone-38371 VCLI_CAPABILITY=admin \
  vcli maestro open --lib LIB --cell CELL --mode r
```

### 窗口是 Reading 模式

> ⚠️ **`vcli maestro open --mode a` 不能把已有 GUI Reading 窗口改成可编辑。**
> 它走的是 `maeOpenSetup`，建的是**后台 setup 会话**：没有窗口、无法 attach 到
> 已有窗口，而且 "a" 模式会**占住 edit lock**——原来的 Reading 窗口照样只读，
> 新后台会话的锁还可能挡住后面在 GUI 里重开。

所以分情况处理：

**1. 只想读 setup / 看分析，不碰锁** —— 用只读后台会话（不取 edit lock，
不影响持有者，也不会挡住 GUI）：

```bash
VB_SESSION=xunipc-userone-38371 VCLI_CAPABILITY=admin \
  vcli maestro open --lib LIB --cell CELL --mode r
```

**2. 要让 GUI 里那个窗口变成可编辑** —— 只能走 **GUI 打开路径**
（`deOpenCellView`），即关掉旧的 ADE 窗口后重新打开：

```bash
vcli skill exec 'foreach(w hiGetWindowList() when(rexMatchp("ADE" hiGetWindowName(w)) hiCloseWindow(w)))'
vcli skill exec 'deOpenCellView("LIB" "CELL" "maestro" "maestro" nil "a")'
```

这一步的前提是**锁是空的**。如果之前已经用 `--mode a` 开过后台会话，
先把它关掉、把锁还回去，否则 GUI 重开会被自己的后台锁挡住：

```bash
vcli maestro list-sessions                     # 找到那个 fnxSessionN
vcli maestro close --session fnxSessionN       # 释放 edit lock
```

**3. 只在自己开的后台会话上就地切锁** —— `vcli maestro set-mode` 走的是
`maeMakeEditable` / `maeMakeReadonly`，在**同一个会话**上就地取锁/还锁，不用重开：

```bash
vcli maestro set-mode --session fnxSessionN --mode a   # 取 edit lock
# ... 只读/写 setup 的那几段 ...
vcli maestro set-mode --session fnxSessionN --mode r   # 还回 edit lock
```

> 这是针对**后台会话**的锁卫生手段，**不是**"把 GUI Reading 窗口变可编辑"——
> 后台会话没有窗口，也不会 attach 到已有窗口。

> 本节不给"一步把 Reading 变可编辑"的承诺：没有在活 Virtuoso 上验证过的
> 路径，就不要写成能修。遇到读不到 / 改不了的情况，按上方「锁归属与当次授权」
> 停下报告。

### maeAddOutput 成功但 maeGetResultOutputs 返回 nil

"Save" 复选框无法通过 SKILL 启用。需要手动在 GUI 中勾选，或使用标量表达式输出。
