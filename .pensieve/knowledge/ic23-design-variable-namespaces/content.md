# IC23 Design Variable Namespaces：两层架构

## Source
Session 2026-04-25：FT0001A_SH/5T_OTA_D_TO_S_sim，调试 W34 参数不写入 netlist。

## Summary
IC23 中存在两层独立的变量命名空间；`maeSetVar` 返回 `t` 但不影响 netlist，只有
`asiSetDesignVarList` 才能写入 Spectre 实际看到的 `parameters` 行。

## Content

### 两层命名空间对照表

| API | 写入位置 | 流入 input.scs | 典型用途 |
|-----|---------|---------------|---------|
| `maeSetVar(name val)` | Maestro 内部 varList | ❌ | GUI 显示值、扫参范围 |
| `maeGetVar(name)` | 同上读取 | — | |
| `asiGetDesignVarList(sess)` | asi session 层 | ✅ 读 | 返回 `(("name" "val") ...)` |
| `asiSetDesignVarList(sess list)` | asi session 层 | ✅ 写 | 真正改变 netlist 参数 |

### 症状

```
maeSetVar("W34" "16u")   → t         ← 看起来成功
maeGetVar("W34")         → "16u"     ← 读回也是新值
input.scs parameters 行  → W34=1     ← 但 netlist 仍是旧值！
```

### 正确 Pattern（替换单个变量）

```skill
vcli skill exec 'let((sess vl)
  sess=asiGetCurrentSession()
  vl=asiGetDesignVarList(sess)
  vl=cons(list("W34" "16u") remove(assoc("W34" vl) vl))
  asiSetDesignVarList(sess vl))'
```

- `assoc("W34" vl)` — 在 alist 中定位旧项
- `remove(old vl)` — 删除旧项
- `cons(newEntry cleaned)` — 在头部插入新项（顺序不影响 netlist）

替换多个变量时链式调用即可：

```skill
let((sess vl)
  sess=asiGetCurrentSession()
  vl=asiGetDesignVarList(sess)
  vl=cons(list("W34" "16u") remove(assoc("W34" vl) vl))
  vl=cons(list("vdc" "0.6") remove(assoc("vdc" vl) vl))
  asiSetDesignVarList(sess vl))
```

### IC23.1 下以下函数未定义，勿用

| 函数 | 结果 |
|------|------|
| `asiSetDesVar(sess name val)` | `*Error* eval: undefined function asiSetDesVar` |
| `asiSetDesignVar(sess name val)` | 同上 |
| `desVar("name" val)` | 通过 bridge 返回 `nil`（缺少 ADE session 上下文） |

### 验证方法

```bash
# 用 maeGetVar 验证是误导性的 — 它只读 Maestro 内部层
# 唯一可靠的验证是检查 netlist
grep "^parameters" <run_dir>/netlist/input.scs
# → parameters temperature=27 W34=16u vdc=0.6 ...
```

### 关联故障：CMI-2441（PMOS 宽度越界）

`W34` 未加单位后缀（如 `W34=1` 而非 `W34=16u`）时，Spectre 将 `1` 解释为 1 米，
触发 CMI-2441：

```
WARNING (CMI-2441): p12 '...PM1': w = 1 is not in range [280n, 100u].
```

根因是 `maeSetVar` 写入了带单位的值，但 `asiSetDesignVarList` 从未被调用，
所以 netlist 仍含原始无单位值 `W34=1`。

修复：改用 `asiSetDesignVarList` 设置 `"W34" "16u"`（带单位字符串）。

## When to Use
- 通过 vcli/SKILL bridge 更新仿真参数后，netlist 中的值没有变化
- 出现 CMI-2441（器件尺寸越界），但 GUI 显示的值是正常的
- 需要脚本化批量修改仿真参数（CI / sweep 场景）

## Context Links
- Leads to: [[maestro]] skill — `asiSetDesignVarList` pattern 已写入 maestro 技能
- Related: [[maestro-netlist-standalone-workflow]] — standalone 模式下参数注入方式不同（直接编辑 input.scs）
