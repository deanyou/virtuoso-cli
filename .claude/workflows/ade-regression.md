---
name: ade-regression
description: ADE 批量回归 — maestro 创建 session → 跑 corner sweep → 汇总 pass/fail
trigger: manual
---
## 步骤

1. `/maestro` — 打开/创建 ADE session，确保处于 Editing 模式
2. 配置 `corners.json`（corner 名、section、temp、vdd）和 `measures[]`（Ocean 表达式）
3. `vcli sim corner --file corners.json --timeout 600 --format json` — 批量跑 corner
4. `/maestro-read-results` — 解析 PSF，提取各 corner 的 measure 值
5. 汇总 JSON → `/sim-plot` 画 grouped bar chart，对比各 corner 差异

## Corner 配置示例

```json
{
  "simulator": "spectre",
  "design": {"lib": "FT0001A_SH", "cell": "OTA_5T", "view": "schematic"},
  "model_file": "/foundry/smic/models/spectre/ms013.lib",
  "analysis": {"type": "ac", "saveOppoint": "t"},
  "corners": [
    {"name": "tt",      "section": "tt",  "temp": 27,   "vdd": 1.2},
    {"name": "ss_hot",  "section": "ss",  "temp": 125,  "vdd": 1.08},
    {"name": "ff_cold", "section": "ff",  "temp": -40,  "vdd": 1.32}
  ],
  "measures": [
    {"name": "gain_dc", "expr": "dB20(VF(\"/OUT\"))"},
    {"name": "gbw",     "expr": "cross(dB20(VF(\"/OUT\")) 0 1 \"falling\")"},
    {"name": "pm",      "expr": "value(phase(VF(\"/OUT\")) + 180)"}
  ]
}
```

## 输出

- `corner_results.json` — 每个 corner 的 measure 值
- `corner_bar.png` — 分组柱状图可视化
- Pass/Fail 表（用户定义阈值，超限标记 fail）
