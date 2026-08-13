---
name: gm-over-id
description: gm/Id methodology for analog IC design — transistor sizing via lookup table approach. Use when designing amplifiers, current mirrors, OTAs, or any analog circuit where you need to determine W/L from specs (GBW, gain, noise). Also use when the user mentions gm/id, transistor sizing, Vov, current density, or design space exploration.
allowed-tools: Bash(*/virtuoso *) Read Write
---

# gm/Id 设计方法论

基于仿真的查表法，用 gm/Id 作为设计自由度，替代传统的 Vov 手算。适用于所有工艺节点，尤其是短沟道器件（经验公式失效时）。

## 核心思想

```
gm/Id = 2/Vov  (长沟道近似)

gm/Id 大 → 弱反型 → 高增益、低速度、大面积
gm/Id 小 → 强反型 → 低增益、高速度、小面积
```

**gm/Id 是设计空间的统一坐标轴**，所有关键参数都可以表示为它的函数：

| 参数 | 与 gm/Id 的关系 | 设计含义 |
|------|-----------------|---------|
| gain = gm/gds | gm/Id ↑ → gain ↑ | 增益需求高 → 选大 gm/Id |
| fT | gm/Id ↑ → fT ↓ | 速度需求高 → 选小 gm/Id |
| Id/W (电流密度) | gm/Id ↑ → Id/W ↓ | 功耗约束 → 查表得 W |
| Vov | gm/Id ↑ → Vov ↓ | 输出摆幅 → 限制 Vov |
| 噪声 Vn² | ∝ 1/gm | 低噪声 → gm 大 → gm/Id 大 |

## 设计流程

### Step 1: 从规格推导 gm

```
GBW = gm₁ / (2π · CL)
→ gm₁ = 2π · GBW · CL · 1.2  (1.2倍裕量考虑寄生)
```

### Step 2: 选择 gm/Id（增益-带宽折中）

| gm/Id 范围 | 反型区域 | 典型用途 |
|-----------|---------|---------|
| 5-8 | 强反型 | 高速电路、电流镜 |
| 8-15 | 中等反型 | 通用放大器（最常用） |
| 15-25 | 弱反型 | 低功耗、高增益 |

### Step 3: 查表得 Id → 计算 W

```
Id_need = gm / (gm/Id)
id_sim  = lookup(gm/Id, L)   ← 查找表中 W=1µm 时的绝对电流 (A)

W = Id_need / id_sim * W_tb  (W_tb = 1µm)
  = Id_need / id_sim          (结果单位 µm)

例: gm/Id=14, L=500n → id_sim=5.01µA (at W=1µm)
    Id_need=13.5µA → W = 13.5/5.01 × 1 = 2.7µm
```

**注意**: lookup JSON 中的 `id` 是绝对电流（A），不是电流密度。
`idw` 字段已废弃，直接用 `id / w_testbench` 计算。

### Step 4: 选择 L（增益-速度折中）

- L 大 → 增益高、速度低
- L 小 → 增益低、速度高
- 经验：L_min ~ 2×L_tech 起步

## 用 virtuoso-cli 自动化仿真

### 1. 生成 gm/Id 查找表（单管 DC 仿真）

需要一个单管 testbench（NMOS 或 PMOS），扫描 VGS，提取 oppoint 参数。

```bash
# 设置仿真
virtuoso sim setup --lib <LIB> --cell <GMID_TB> --view schematic
virtuoso skill exec 'resultsDir("/tmp/gmid_lookup")'
virtuoso skill exec 'desVar("L" 200e-9)'

# DC 扫描 VGS
virtuoso sim run --analysis dc --param saveOppoint=t --timeout 120

# 提取关键参数 (在每个 VGS 偏置点)
virtuoso sim measure --analysis dcOp \
  --expr 'value(getData("/NM0:gm" ?result "dcOpInfo"))' \
  --expr 'value(getData("/NM0:ids" ?result "dcOpInfo"))' \
  --expr 'value(getData("/NM0:gds" ?result "dcOpInfo"))' \
  --expr 'value(getData("/NM0:vth" ?result "dcOpInfo"))' \
  --expr 'value(getData("/NM0:cgs" ?result "dcOpInfo"))'
```

### 2. 生成完整曲线（参数扫描 L）

```bash
# 用 SKILL 直接生成 waveVsWave 查找曲线
virtuoso skill exec '
  ;; 确保 DC 结果已加载
  selectResult(quote(dc))
  
  ;; gm/Id vs gain (self_gain = gm/gds)
  waveVsWave(?x OS("/NM0" "gmoverid") ?y OS("/NM0" "self_gain"))
'

virtuoso skill exec '
  ;; gm/Id vs Id/W (电流密度，对数坐标更直观)
  waveVsWave(?x OS("/NM0" "gmoverid") ?y (OS("/NM0" "id") / VAR("W")))
'

virtuoso skill exec '
  ;; gm/Id vs Vov
  waveVsWave(?x OS("/NM0" "gmoverid") ?y (OS("/NM0" "vgs") - OS("/NM0" "vth")))
'

virtuoso skill exec '
  ;; gm/Id vs lambda (沟道长度调制系数)
  waveVsWave(?x OS("/NM0" "gmoverid") ?y (OS("/NM0" "gds") / OS("/NM0" "id")))
'
```

### 3. 从查找表读取设计参数

```bash
# 已知 gm/Id = 10，L = 200n，查 Id/W
virtuoso skill exec '
  selectResult(quote(dc))
  let((gmid_wave idw_wave)
    gmid_wave = OS("/NM0" "gmoverid")
    idw_wave = OS("/NM0" "id") / VAR("W")
    cross(waveVsWave(?x gmid_wave ?y idw_wave) 10 1 "falling")
  )
'

# 已知 gm/Id = 10，L = 200n，查 gain
virtuoso skill exec '
  selectResult(quote(dc))
  let((gmid_wave gain_wave)
    gmid_wave = OS("/NM0" "gmoverid")
    gain_wave = OS("/NM0" "self_gain")
    cross(waveVsWave(?x gmid_wave ?y gain_wave) 10 1 "falling")
  )
'
```

### 4. PMOS 仿真（注意 abs）

```bash
# PMOS 的 Id 和 gds 为负值，需要取绝对值
virtuoso skill exec '
  waveVsWave(?x OS("/PM0" "gmoverid") ?y OS("/PM0" "self_gain"))
'
virtuoso skill exec '
  waveVsWave(?x OS("/PM0" "gmoverid") ?y abs(OS("/PM0" "id") / VAR("W")))
'
```

## 设计实例：二级 OTA

### 规格
- GBW = 10 MHz, CL = 10 pF, Gain > 60 dB

### Step 1: 输入对管 gm
```
gm₁ = 2π × 10M × 12p = 753.6 µS
```

### Step 2: 选 gm/Id = 12 (中等反型，增益-速度平衡)
```
Id₁ = gm₁ / (gm/Id) = 753.6µ / 12 = 62.8 µA
```

### Step 3: 查表 Id/W (L=500n 时)
```bash
# 假设查得 Id/W = 2.5 µA/µm
W₁ = Id₁ / (Id/W) = 62.8 / 2.5 = 25.1 µm
```

### Step 4: 验证增益
```bash
# 查得 gain(gm/Id=12, L=500n) ≈ 35
# 两级总增益 ≈ 35 × 35 = 1225 ≈ 62 dB ✓
```

### 设计指导原则

| 管子角色 | gm/Id 选择 | 原因 |
|---------|-----------|------|
| 输入差分对 | 10-15 | 平衡增益和带宽，gm/Id 大有利于噪声 |
| 电流镜负载 | 5-10 | gm 小 → 噪声贡献小 |
| 尾电流源 | 5-8 | 不需要高 gm，匹配重要 |
| 输出级 | 8-12 | 平衡摆幅和驱动能力 |
| 需要大摆幅的管子 | 12-20 | Vov 小 → 留更多输出摆幅 |

## Ocean SKILL 代码参考

```skill
;; 完整的 gm/Id 查找表仿真设置 (NMOS)
simulator('spectre)
design("LIB" "gmid_nmos_tb" "schematic")
resultsDir("/tmp/gmid_nmos")
analysis('dc ?saveOppoint t)
save('all)

;; 仿真输出定义
ocnxlOutputSignal("gmoverid" ?plot t
  ?expr "OS(\"/NM0\" \"gmoverid\")")
ocnxlOutputSignal("self_gain" ?plot t  
  ?expr "OS(\"/NM0\" \"self_gain\")")
ocnxlOutputSignal("id_over_w" ?plot t
  ?expr "OS(\"/NM0\" \"id\") / VAR(\"W\")")
ocnxlOutputSignal("fT" ?plot t
  ?expr "OS(\"/NM0\" \"ft\")")

;; 参数扫描 L
paramAnalysis("L" ?values '(200n 300n 500n 1u 2u))

run()
```

## 关键陷阱

- **gm/Id 查找表与工艺强相关** — 换工艺必须重新仿真
- **L 对 gain 影响巨大** — 必须同时扫描 L
- **PMOS Id/gds 为负** — 用 `abs()` 取绝对值
- **短沟道 gm/Id 曲线偏离理想** — 这正是查表法的优势
- **W 初始值影响结果** — 仿真时 W 取中间值，最后根据计算结果微调后重仿
- **体效应** — 仿真 testbench 中 B 端接法要与实际电路一致
- **self_gain 不可用时** — 手动用 `gm/gds` 替代：`OS("/NM0","gm")/OS("/NM0","gds")`
