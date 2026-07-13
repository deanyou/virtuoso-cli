---
name: analog-design-flow
description: 模拟芯片设计闭环 — gm/Id 选点 → schematic 生成 → 仿真配置/运行 → 量测 → 绘图
trigger: manual
---
## 步骤

1. `/gm-over-id` — 根据 GBW/CL 推导 gm，扫 L 建立 lookup table，输出 W/L
2. `/schematic-gen` — 读取尺寸，生成原理图 cellview
3. `/sim-setup` — 配置 Ocean（model file、desVar、resultsDir）
4. `/sim-run` — DC/AC/Tran 分析，写 PSF
5. `/sim-measure` — Ocean 表达式提取 gm、gain、GBW、PM
6. `/sim-plot` — matplotlib 可视化

## 迭代回路

若 measure 结果不满足规格 → 回到 step 1 重新调整 W/L
