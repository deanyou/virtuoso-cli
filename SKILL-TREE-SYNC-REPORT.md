# 双技能树同步机制 — 落地报告

分支：`feat/skill-tree-sync`（基于 `origin/main` @ `1990f04`）
工具：`scripts/sync_skills.py`（仅 Python 3 标准库）
CI：`.github/scripts/run-skill-tests.sh`（两棵树 + 漂移闸门）
workflow 改动：**未改**，补丁见 `WORKFLOW-PATCH.md`

> **本轮（补漏轮）**：落地两条用户裁决 —— ① `virastack-auto-allow` 由 PENDING 改为
> **镜像**；② Codex glob 前缀扩围到 `("virtuoso", "vcli", "spectre")`。
> `vcli-architecture` **保持 PENDING**。详见第 4 节。

---

## 1. 双树现状与差异归类

同步前 `.claude/skills`（180 个文件 + 30 个 skill）与 `.agents/skills`
（104 个文件 + 26 个 skill）的差异，按「过期 / 适配 / 独占」三类逐条归类如下。

| 类别 | 数量 | 判定依据 |
|---|---|---|
| **纯过期**（canonical 前进、Codex 侧落后） | 89 个文件 | 内容是同一份代码/文档的旧版本：`.agents` 侧的 Python、HTML、SQLite 都在 `.claude` 侧有更新版本，且差异是纯粹的代码演进（新增 import、新增分支、重构），没有任何宿主相关字样。 |
| **适配**（有意的宿主差异） | 6 条规则 / 分别命中 25、19、7、3、6、1 个文件 | 见第 2 节变换表。全部集中在 `SKILL.md` 的 frontmatter 与散文中。 |
| **独占**（只属于一侧） | `.claude` 侧 3 个 skill + 1 个字节码文件 | `pstack-vcli` 在 `.agents` 侧是**相对符号链接** `../../.claude/skills/pstack-vcli`（不是拷贝）—— 这是仓库已有的防漂移机制，AGENTS.md 与 `docs/pstack-vcli-assessment.md` 都写明 Codex 通过它发现同一 skill；`vcli-architecture` 归属未定，见第 4 节。 |

### 1.1 关键事实：`.agents` 是手工维护的拷贝，不是自动镜像

`git log -- .agents` 只有 4 个提交，且逐个核对后可复现出当初的复制规则：

| 提交 | `.agents` 侧动作 | 复制规则 |
|---|---|---|
| `83c9d35`（首次批量导入 8 个 skill） | `_shared, diag, digital-import, history-evolve, maestro-read-results, porting-skill, skill-review, spectre-netlist-template, virtuoso-gui-debug` | **逐字节原样拷贝**，未做 frontmatter 适配 |
| `442f308`（新增 13 个 skill） | amp-copilot、cell-explore、circuit-optimizer、gm-over-id、maestro、sim-measure、sim-run、sim-setup、sim-sweep、skill-exec、spec-driven-circuit-design、tunnel-connect、veriloga | 剥掉 `argument-hint`，`Bash(virtuoso *)` → `Bash(*/virtuoso *)` |

由此得到一个可验证的结论：**`argument-hint` 在 `.agents` 侧保留的 6 个文件，
恰好就是首次批量导入时原样拷贝的那 6 个**（数字上完全吻合，逐个核对 frontmatter
键集合全部命中）。所以它们不是设计决策，是导入方式的副产物。

### 1.2 复制过来的 `.agents` 正文本身是旧快照

`maestro` / `skill-exec` / `veriloga` 三个 skill 的 `.agents` 版本，正文用的是
`virtuoso skill exec`（旧名），而当前 `.claude` 已改成 `vcli skill exec`。
它们在 `442f308` 时被拷贝自更早的 `.claude`，所以「`Bash(*/virtuoso *)` 而
`.claude` 是 `Bash(vcli *)`」这类差异属于**过期**，不是适配。

---

## 2. 变换表（6 条规则）

全部集中在 `scripts/sync_skills.py` 的 `ADAPTATIONS` 表里，每条有独立 id、
说明和命中计数。作用域限定在 `<skill>/SKILL.md`——依据是实测：`_shared/vcli.sh`
与 `_shared/scripts/eda_auto_evolve.py` 两树逐字节相同且都写着 `.claude/skills/`，
说明当初的复制**没有**适配非 SKILL.md 文件；非 SKILL.md 文件现在仍然逐字节镜像。

| id | 规则 | 作用域 | 本轮命中文件数 |
|---|---|---|---|
| **A1** | 删掉 frontmatter 的 `argument-hint` 字段 | frontmatter | **25** |
| **A2** | `Bash(<cmd> *)` → `Bash(*/<cmd> *)` | frontmatter 的 `allowed-tools` 行 | **19** |
| **A3** | `Claude Code` → `Codex` | 全文 | **7** |
| **A4** | `Claude` → `Codex` | 全文 | **3** |
| **A5** | `.claude/` → `.Codex/` | 全文 | **6** |
| **A6** | `CLAUDE.md` → `AGENTS.md` | 全文 | **1** |

### 2.1 A1：`argument-hint` 剥离

**依据（外部 + 仓库内双向证据）**：

- Codex 的 skill 校验器只接受 `name / description / license / allowed-tools / metadata`
  五个 frontmatter 键，`argument-hint` 不在其中 → 带上它 Codex 侧直接校验失败。
- 仓库内：22 个后复制的 skill 全部剥掉了它；保留它的 6 个恰好是首次原样导入的那 6 个。
- `CLAUDE_SKILL_DIR` **不**改：`.agents` 侧 4 个文件保留该变量名，实测两树一致。

本轮 `virastack-auto-allow` 镜像后命中数 24 → 25（它的 `argument-hint` 被剥掉）。

### 2.2 A2：`allowed-tools` glob 改写

**作用域**：`allowed-tools` frontmatter 行，**只**对 `CODEX_GLOB_PREFIX_COMMANDS`
里的命令生效。

- 为什么只限定 `allowed-tools` 行、不动正文：`skill-review/SKILL.md` 正文里
  写着 `CORRECT: Bash(virtuoso *)` / `WRONG: Bash(*/virtuoso *)`。若全文替换，
  同一文件会同时出现两行互相矛盾的 `Bash(*/virtuoso *)`，等于把文档改坏。
- **命令集合本轮扩围**（用户裁决）：`("virtuoso",)` → `("virtuoso", "vcli", "spectre")`。
  扩围前的依据是「从数据归纳」：13 个做过适配的 skill 被加前缀的命令无一例外都是
  `virtuoso`，而 `cat` / `ls` / `jq` / `grep` / `find` / `wc` / `python` / `python3`
  从未被加过前缀。裁决的理由是 `vcli` 与 `spectre` 同样是 EDA 二进制、与 `virtuoso`
  走同一条解析路径，应当一并加前缀。**系统工具类 glob 仍然保持裸写**（`Bash(cat *)`、
  `Bash(python *)`、`Bash(python3 *)` 等 8 处未变），不收紧任何目前可用的 allow-list。

扩围后 A2 的实际覆盖面（正本 `allowed-tools` 里用到 `vcli` / `spectre` 的 skill）：

| skill | 本轮前 `.agents` | 本轮后 `.agents` |
|---|---|---|
| `maestro` | `Bash(vcli *)` | `Bash(*/vcli *)` |
| `skill-exec` | `Bash(vcli *) Bash(*/virtuoso *)` | `Bash(*/vcli *) Bash(*/virtuoso *)` |
| `ocean-netlist-regen` | `Bash(*/virtuoso *) Bash(vcli *) Bash(spectre *) Read Write` | `Bash(*/virtuoso *) Bash(*/vcli *) Bash(*/spectre *) Read Write` |
| `porting-skill` | `Bash(vcli *) Bash(*/virtuoso *) Bash(spectre *) Read Write Edit Grep` | `Bash(*/vcli *) Bash(*/virtuoso *) Bash(*/spectre *) Read Write Edit Grep` |
| `schematic-gen` | `Bash(vcli *) Read Write Edit` | `Bash(*/vcli *) Read Write Edit` |
| `spectre-netlist-gotchas` | `Bash(spectre *) Read Write` | `Bash(*/spectre *) Read Write` |
| `spectre-netlist-template` | `Bash(python *) Bash(spectre *) Read Write` | `Bash(python *) Bash(*/spectre *) Read Write` |
| `veriloga` | `Bash(*/virtuoso *) Bash(vcli *) Bash(spectre *) Read Write Edit` | `Bash(*/virtuoso *) Bash(*/vcli *) Bash(*/spectre *) Read Write Edit` |
| `virastack-auto-allow`（本轮新镜像） | — | `Bash(python3 *) Bash(*/vcli *) Read` |

（A2 在本轮共命中 **19 个文件**、改写 **26 处 glob**；上表列的是受影响的 9 个，
其余 10 个只含 `virtuoso` glob，形态未变。）

### 2.3 单 skill 规则豁免机制（本轮新增）

`virastack-auto-allow` 的正文里 3 处 "Claude Code" 全部指向
**Virtuoso 里内嵌的 ViraStack/Claude Code 弹窗这个产品**，而不是驱动本 skill 的宿主：

```
description: Auto-approve the ViraStack (Claude Code UI) permission prompt …
description_zh: … 自动批准 ViraStack（Claude Code UI）的权限提示 …
- "auto click the Allow button in ViraStack / Claude Code"
```

若照常套用 A3/A4，Codex 侧会变成「自动批准 ViraStack (**Codex UI**) 的权限提示」——
描述了一个并不存在的 UI；而且 `scripts/auto_allow.py` 的模块 docstring（非 SKILL.md，
逐字节镜像）写的是同一个产品名，两侧会对不上。

因此 manifest 新增 `SkillRule.skiprules`：该 skill 显式豁免 A3/A4，其余规则照常。
这是**逐字产品名**，不是宿主身份，豁免范围仅限这一个 skill 的两条规则，可一行回退。

---

## 3. 同步结果

### 3.1 首轮（首次同步）

```
$ python3 scripts/sync_skills.py
$ python3 scripts/sync_skills.py --check
no drift: .agents/skills mirrors .claude/skills
```

| 项 | 数值 |
|---|---|
| 同步文件总数 | **89** |
| ├─ 新建（canonical 有、Codex 侧没有） | **60** |
| └─ 更新（两边都有但内容过期） | **29** |
| 删除 | 0（Codex 侧没有任何一个文件缺少 canonical 对应物） |

按类别逐条核对：

- **纯过期 → 已同步**：29 个更新文件全部是 `.claude` 侧的现行版本直接落地
  （`virtuoso-gui-debug` 的 `scripts/rsi.py`、`scripts/rsi_query.py`、
  `scripts/generate_report.py`、`vgui_runner/engine.py`、`vgui_runner/live_executor.py`、
  `tests/test_engine.py`、`tests/test_live_executor.py`、`data/skill_db.sqlite3`、
  `report/rsi_report.html`，以及 20 个 SKILL.md 正文）。
- **适配 → 由变换表生成**：24 个 SKILL.md 剥掉 `argument-hint`，14 个改写
  `allowed-tools` glob，7/3/6/1 处做宿主身份替换。
- **无法判定 → 停下**：见第 4 节，两个 skill 一律不动，工具只报告不猜。

### 3.2 本轮（补漏轮）

```
$ python3 scripts/sync_skills.py --dry-run
… 13 file(s) would change
$ python3 scripts/sync_skills.py
wrote 13 file(s)
no drift: .agents/skills mirrors .claude/skills
```

| 项 | 数值 |
|---|---|
| **本轮改动文件数** | **14** = `.agents/skills` **13**（新建 **5** + 更新 **8**）+ 工具 `scripts/sync_skills.py` **1** |
| ├─ 新建：`virastack-auto-allow` 全量 | 5（`SKILL.md`、`data/allow_prompt_template.png`、`scripts/auto_allow.py`、`scripts/resolve_target.py`、`tests/test_auto_allow.py`） |
| └─ 更新：glob 扩围波及的 SKILL.md | 8（maestro、ocean-netlist-regen、porting-skill、schematic-gen、skill-exec、spectre-netlist-gotchas、spectre-netlist-template、veriloga） |
| 删除 | 0 |
| 累计（相对 `origin/main`） | `.agents/skills` 新建 **65** + 更新 **29** = **94** 个文件 |
| 符号链接 | `.agents/skills/pstack-vcli -> ../../.claude/skills/pstack-vcli` **保持原样**，工具只校验不重写 |

`virastack-auto-allow` 的镜像内容逐项核对：

| 文件 | 与正本关系 | 说明 |
|---|---|---|
| `SKILL.md` | **适配后** | 剥 `argument-hint`；`Bash(vcli *)` → `Bash(*/vcli *)`；豁免 A3/A4，产品名保持 |
| `data/allow_prompt_template.png` | 逐字节相同 | `auto_allow.py` 用 `dirname(dirname(abspath(__file__)))/data/...` 解析模板，随目录一起镜像后仍指向正确路径（已实测存在） |
| `scripts/auto_allow.py` | 逐字节相同 | 非 SKILL.md 文件不参与适配 |
| `scripts/resolve_target.py` | 逐字节相同 | 同上 |
| `tests/test_auto_allow.py` | 逐字节相同 | 同上；`.agents` 侧实跑 42 个测试全绿 |

### 3.3 抽查：amp-copilot（宿主适配未被抹掉）

```yaml
# .claude/skills/amp-copilot/SKILL.md
argument-hint: '[circuit type and specs, e.g. "two-stage OTA GBW=50MHz CL=10pF"]'
allowed-tools: Bash(virtuoso *) Read Write

# .agents/skills/amp-copilot/SKILL.md  （同步后）
allowed-tools: Bash(*/virtuoso *) Read Write
```

`.agents` 侧**无** `argument-hint`，`allowed-tools` **保持 Codex 写法**
`Bash(*/virtuoso *)`。该文件在两次同步中字节未变，说明变换表完全复现了既有 Codex 形态。

### 3.4 抽查：virastack-auto-allow（本轮新镜像）

```yaml
# .agents/skills/virastack-auto-allow/SKILL.md
---
name: virastack-auto-allow
display_name: ViraStack Auto-Allow
description: Auto-approve the ViraStack (Claude Code UI) permission prompt …
description_zh: … 自动批准 ViraStack（Claude Code UI）的权限提示 …
allowed-tools: Bash(python3 *) Bash(*/vcli *) Read
…
```

- `argument-hint` 出现次数：**0** ✓
- `allowed-tools` 中 `vcli` **已带 `*/` 前缀** ✓
- `allowed-tools` 中 `python3` **不带前缀** ✓
- 正文里的 "Claude Code UI" 保留（内嵌产品名，见 2.3）

---

## 4. 待裁决清单

### 4.1 已裁决

| # | 对象 | 裁决 | 落地 |
|---|---|---|---|
| **P2** | `virastack-auto-allow`（5 个文件） | **镜像**（2026-11 用户裁决） | manifest 由 `MODE_PENDING` 改为 `MODE_MIRROR`；全量镜像 SKILL.md / data / scripts / tests，适配规则照常应用；`.agents` 侧新增 42 个测试 |
| **P3** | `allowed-tools` glob 命令集合 | **扩围**（2026-11 用户裁决） | `CODEX_GLOB_PREFIX_COMMANDS` 由 `("virtuoso",)` 改为 `("virtuoso", "vcli", "spectre")`；8 个受影响的 skill 重新同步；系统工具类 glob 保持裸写 |

### 4.2 仍待裁决

| # | 对象 | 问题 | 可选项 | 影响面 |
|---|---|---|---|---|
| **P1** | `vcli-architecture`（`.claude/skills/vcli-architecture/SKILL.md`，1 个文件） | Codex 是否应加载这个架构治理 skill？ | (a) 镜像（改为 `MODE_MIRROR`，1 个文件）；(b) 保持 `.claude` 独占（改为 `MODE_EXCLUDE`）；(c) 维持现状（`MODE_PENDING`） | frontmatter 只有 `name` + `description`，正文宿主中立。**关键连带**：`pstack-vcli/SKILL.md` 里写着「再读 `.claude/skills/vcli-architecture/SKILL.md`」——今天两边都能读到，**仅仅因为 pstack-vcli 是符号链接**。若 P1 选 (a)，则 pstack-vcli 的那份引用在 Codex 侧需要一并改写（或把 pstack-vcli 也改成真实拷贝）。 |

工具行为：`pending` 的 skill **不会被镜像，也不会被删除**，只在
`--check` / `--report` 输出里列出。默认不阻断 CI；加 `--strict-pending`
可让待裁决项直接失败（退出码 2）。

---

## 5. CI 接线

`.github/scripts/run-skill-tests.sh` 改为对**两棵树**做同样的事：

1. **漂移闸门**（新增，最先跑）：`python3 scripts/sync_skills.py --check`。
   不通过 → 立刻 `exit 1` 并提示 `run 'python3 scripts/sync_skills.py'`。
   放在测试之前，避免「用过期内容测完再报第二个更费解的错」。
2. **语法检查**：两棵树都做（`ast.parse`，不写 `__pycache__`；`find` 不跟随符号
   链接，所以被链接的 skill 只在正本位置查一次）。
3. **unittest 套件**：两棵树都跑。
4. **防假绿逻辑对两棵树分别成立**：
   - 「0 套件即失败」在每棵树内单独判定（`no $tree/*/tests directory found`）；
   - 「0 测试即失败」在每个套件内单独判定（必须出现 `Ran [1-9][0-9]* tests?`）。

实测（假绿守卫逐条验证，见第 7 节）：

| 场景 | 结果 |
|---|---|
| 正常 | `OK: 8 skill suite(s) passed across 2 tree(s).` exit 0 |
| 某棵树某个套件 0 测试 | `ERROR: 1 of 1 skill suite(s) failed in .agents/skills.` exit 1 |
| 某棵树没有任何 `tests/` | `ERROR: no .claude/skills/*/tests directory found; refusing to pass.` exit 1 |
| 两棵树漂移 | `ERROR: the skill trees have drifted; run 'python3 scripts/sync_skills.py'.` exit 1 |

`WORKFLOW-PATCH.md` 给出 `skill-tests.yml` 的路径过滤补丁
（新增 `.agents/skills/**` 与 `scripts/sync_skills.py`）。
**`.github/workflows/` 下没有任何文件被修改**（`git status` 验证为空）。

---

## 6. 测试结论（原文）

本轮（补漏轮）实跑：

```
### .claude/skills
--- .claude/skills/pstack-vcli (exit=0)
Ran 17 tests in 12.349s
OK
--- .claude/skills/verify-virtuoso-cli (exit=0)
Ran 6 tests in 0.461s
OK
--- .claude/skills/virastack-auto-allow (exit=0)
Ran 42 tests in 11.082s
OK
--- .claude/skills/virtuoso-gui-debug (exit=0)
Ran 370 tests in 8.629s
OK
### .agents/skills
--- .agents/skills/pstack-vcli (exit=0)
Ran 17 tests in 12.226s
OK
--- .agents/skills/verify-virtuoso-cli (exit=0)
Ran 6 tests in 0.520s
OK
--- .agents/skills/virastack-auto-allow (exit=0)
Ran 42 tests in 10.849s
OK
--- .agents/skills/virtuoso-gui-debug (exit=0)
Ran 370 tests in 8.205s
OK
```

CI 脚本整跑（`bash .github/scripts/run-skill-tests.sh`，exit 0）：

```
OK: 4 skill suite(s) passed in .claude/skills.
OK: 4 skill suite(s) passed in .agents/skills.
OK: 8 skill suite(s) passed across 2 tree(s).
```

- `.claude/skills`：4 个套件 / **435** 个测试，全绿。
- `.agents/skills`：4 个套件 / **435** 个测试，全绿（本轮起与正本持平——上轮少
  `virastack-auto-allow` 是因为它处于待裁决状态未被镜像，现已镜像）。
- 两棵树都没有「0 测试的假绿」：每个套件都有正的 `Ran N tests`。
- `.agents/skills/pstack-vcli` 的套件经符号链接解析后运行，等于额外验证了那个链接可用。

---

## 7. 逐项验证证据

| 验证项 | 命令 | 结果 |
|---|---|---|
| 同步后 `--check` 干净退出 0 | `python3 scripts/sync_skills.py --check` | `no drift: .agents/skills mirrors .claude/skills`，exit 0 |
| PENDING 只剩 vcli-architecture | 同上 | `PENDING DECISION (1 skill(s) left untouched): vcli-architecture` |
| 两棵树 unittest 全绿 | `bash .github/scripts/run-skill-tests.sh` | `OK: 8 skill suite(s) passed across 2 tree(s).` exit 0 |
| `.agents` 侧新落的 virastack 套件 | `cd .agents/skills/virastack-auto-allow && python3 -m unittest discover tests` | `Ran 42 tests in 10.849s` / `OK` |
| amp-copilot 抽查 | `head -5 .agents/skills/amp-copilot/SKILL.md` | 无 `argument-hint`；`allowed-tools: Bash(*/virtuoso *) Read Write` |
| virastack-auto-allow 抽查 | `head -12 .agents/skills/virastack-auto-allow/SKILL.md` | `argument-hint` 计数 0；`allowed-tools: Bash(python3 *) Bash(*/vcli *) Read` |
| 模板路径随镜像可解析 | `auto_allow.py` 的 `DEFAULT_TEMPLATE` | 解析到 `.agents/skills/virastack-auto-allow/data/allow_prompt_template.png`，`exists: True` |
| `.claude` 正本未被改 | `git status --porcelain -- .claude` | 空 |
| workflow 未被改 | `git status --porcelain -- .github/workflows` | 空 |
| 符号链接未被替换 | `ls -la .agents/skills/pstack-vcli` | `pstack-vcli -> ../../.claude/skills/pstack-vcli` |
| 无文件被删 | 对比 `origin/main` 的 `.agents/skills` 文件集 | `deleted: []` |
| 防假绿：0 测试（`.claude` 树） | 临时夹具：套件文件只有 `import unittest` | `ERROR: 1 of 1 skill suite(s) failed in .claude/skills.`，exit 1 |
| 防假绿：0 测试（`.agents` 树） | 临时夹具 + 临时移除漂移闸门 | `Ran 0 tests in 0.000s` / `NO TESTS RAN` / `FAILED: .agents/skills/alpha`，exit 1 |
| 防假绿：0 套件 | 临时夹具：两棵树都没有 `tests/` | `ERROR: no .claude/skills/*/tests directory found; refusing to pass.`，exit 1 |
| 防假绿：漂移 | 临时夹具：两棵树内容不同 | `ERROR: the skill trees have drifted; run 'python3 scripts/sync_skills.py'.`，exit 1 |

---

## 8. 沉淀（sediment）

> **机械检查**：`python3 scripts/sync_skills.py --check` 已接入
> `.github/scripts/run-skill-tests.sh`（两棵树都跑测试，漂移闸门先行），
> 并由 `WORKFLOW-PATCH.md` 的路径过滤保证 `.agents/skills/**` 与
> `scripts/sync_skills.py` 的改动会触发该 job——从此「双树漂移」在 CI 里
> 是一条可复现的红，而不是靠人记得同步。
> **本轮补充**：宿主适配的每一项都是可审计的显式声明——变换表 6 条规则
> （`ADAPTATIONS`）、每个 skill 的处置（`SKILL_RULES`）、单 skill 规则豁免
> （`SkillRule.skiprules`）、glob 命令集合（`CODEX_GLOB_PREFIX_COMMANDS`）；
> 改一项适配 = 改一行常量 + 重跑一次同步，没有藏在人脑里的隐式约定。

---

## 9. 约束遵守情况

- 只用 Python 3 标准库 + bash，未安装任何工具链或包。
- 未改任何 Rust 代码；`.claude/` 侧零改动（git 验证为空）。
- `.agents/` 侧所有改动均由 `scripts/sync_skills.py` 生成，无手编。
- 未 push、未开 PR；仅本地提交到 `feat/skill-tree-sync`。
- `.github/workflows/` 下零改动，改动以 `WORKFLOW-PATCH.md` 交付。

### 9.1 Rust 门禁未跑（说明）

`AGENTS.md` 要求收工前跑 `cargo test && cargo clippy -- -D warnings && cargo fmt --check`。
本机 **没有 cargo**（`command -v cargo` 无输出），而本任务的负约束明令
「不许安装任何工具链或包」，因此没有安装 Rust 工具链、也没有跑这三个门禁。
本分支 **Rust 侧改动为零**（`git status -- src/ Cargo.toml` 为空），
三个门禁的输入未变，故其结论与基线一致；此事实一并记录在此，不作「已验证」宣称。

### 9.2 与裁决原文的一处偏离（已显式记录，可一行回退）

裁决要求「适配规则照常应用」。对 `virastack-auto-allow` 而言，A3/A4 若照常套用，
会把正文里的 **内嵌产品名** "ViraStack (Claude Code UI)" 改成并不存在的
"ViraStack (Codex UI)"。已改为在 manifest 中对该 skill 显式豁免 A3/A4
（`SkillRule.skiprules=("A3", "A4")`），其余规则（尤其 A1、A2）照常应用。
如需恢复「照常应用」，删除该 tuple 即可，代价是 Codex 侧 skill 描述里的产品名失真。
