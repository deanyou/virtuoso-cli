# Maestro Netlist → Standalone Spectre Workflow（标准 CMOS n12/p12）

## Source
Session 2026-04-20：FT0001A_SH/5T_OTA_D_TO_S_sim 仿真验证。

## Summary
对于使用标准 CMOS（n12/p12）器件的电路，Maestro 生成的 netlist 已包含完整端口连接，
无需 SKILL 拓扑提取；只需修 model 路径、注入参数值、加分析语句，即可直接运行 spectre。

## Content

### 适用条件

- 器件为标准 CMOS（n12/p12、p33/n33 **不适用** → 见 [[spectre-cmi-2116-ade-netlist]]）
- Maestro 已做过至少一次 run（netlist 目录存在）
- 目标是独立 spectre 运行（不依赖 ADE session token）

### Step 0 — 获取 netlist 路径

```bash
vcli maestro session-info --format json | jq -r '.run_dir'
# → /home/meow/projects/ft0001/simulation/FT0001A_SH/5T_OTA_D_TO_S_sim/maestro/results/
#   maestro/.tmpADEDir_meow/.../netlist
```

### Step 1 — 检查 netlist 时效性（必须）

```bash
grep "Generated on" <netlist_dir>/input.scs
stat <schematic_oa_path>/sch.oa
# netlist 生成时间必须晚于 schematic mtime，否则 netlist 过期
```

### Step 2 — 检查 model 路径是否含 `oa/.../../`

```bash
grep "oa/smic" <netlist_dir>/input.scs | head -1
```

若含有 `oa/smic13mmrf_1233//../models/`，需替换为绝对路径：

```python
bad  = ".../oa/smic13mmrf_1233//../models/spectre"
good = ".../models/spectre"
src = open("input.scs").read().replace(bad, good)
```

详见 [[spectre-ade-model-path]]。

### Step 3 — 注入参数值和分析语句

Maestro netlist 的 `parameters` 行无值：
```spectre
parameters W34 vdc v1 v2   ← 需要替换
```

查询 Maestro 中设置的值：
```bash
vcli maestro list-vars --session <sess> --format json
```

替换并追加分析：
```python
src = src.replace("parameters W34 vdc v1 v2",
                  "parameters W34=8u vdc=0.6 v1=0.4 v2=0.8")

analyses = """
dcop  dc   oppoint=rawfile
ac1   ac   start=1 stop=1G dec=20 annotate=status
"""
src = src.replace("saveOptions options save=allpub",
                  analyses + "saveOptions options save=allpub")
```

### Step 4 — 运行 spectre

```bash
mkdir -p /tmp/ota_sim/psf
cd /tmp/ota_sim && spectre input.scs \
  +escchars \
  +log psf/spectre.out \
  -format psfxl -raw psf \
  +mt -maxw 5 -maxn 5
tail -2 psf/spectre.out   # 确认 0 errors
```

### Step 5 — 读取结果

DC：
```bash
psf -t "net1" -i psf/dcop.dc | grep VALUE -A 20 | grep '"V"'
```

AC（复数值解析）：
```python
# 181点 1Hz-1GHz logspace
import numpy as np, math
freqs = np.logspace(0, 9, 181)
# 解析 psf -t "net1" -i psf/ac1.ac VALUE 段 → (re, im) 对
# mag = sqrt(re^2+im^2)；GBW 在 mag 跨越 1 处；PM = 180 + atan2(im,re) - 180
```

### 已验证结果（5T OTA，W34=8u，Iref=100µA，VDD=1.2V）

| 指标 | 值 |
|------|---|
| 开环 DC 增益 | 131 V/V (42.3 dB) |
| Voffset（unity-gain） | 0.6 mV |
| GBW | 12.59 MHz |
| 相位裕度 | 83.9° |

## When to Use
- 需要在不触发 ADE GUI 的情况下运行仿真（CI / 批量 / 自动化）
- Maestro 已有现成 netlist（`session-info` 可查 run_dir）
- 器件全部为 n12/p12（非 SMIC mmRF n33/p33）

## Context Links
- Based on: [[spectre-ade-model-path]] — model 路径修复
- Based on: [[2026-04-20-maestro-api-run-blocks-ic23]] — 为什么不能用 maeRunSimulation
- Related: [[spectre-cmi-2116-ade-netlist]] — n33/p33 需要 SKILL 拓扑提取（不同路径）
