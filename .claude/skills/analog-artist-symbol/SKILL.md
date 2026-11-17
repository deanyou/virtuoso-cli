---
name: analog-artist-symbol
description: |
  按真实接口、网表与拓扑绘制紧凑可读的模拟电路功能 SVG symbol，并回迁为
  Cadence Virtuoso 原生 symbol。用于 symbol 生成、审阅、改造和 SVG→OA 转换。
argument-hint: '[library/cell/view, e.g. "FT0001A_SH/cmp_strongarm/symbol"]'
---

# Analog artist symbol

用功能图、端口组织和原生几何表达模拟电路的作用。输出包含可编辑 SVG、转换脚本和回读证据，让符号在父级原理图里仍能辨认信号来源、控制作用和极性。

本技能面向已有电路视图和真实网表。拓扑实例化、连线和器件摆放使用项目已有的 `schematic-gen`；本技能处理符号外观、接口语义、OA 几何与验收。

## 前置条件

- `vcli` 已连接 live Virtuoso，`skill.eval` 需要 `VCLI_CAPABILITY=admin`。
- 有 SVG 渲染工具，例如 `rsvg-convert`、浏览器或 Inkscape。
- 目标库、cell、view 已知；参考库只读。
- 默认在 scratch library 中生成和验收。写入用户设计视图需要单独授权，且必须先做磁盘级备份。

## 流程

1. 读取真实接口：`vcli rpc call --method symbol.inspect` 取得端口名、方向、端序和 bBox；从 schematic 读实例 `model`（`nch` / `pch`）、网名和连接关系。把这些信息作为器件类型、控制映射和输出极性的依据。
2. 规划功能组：确定主信号路径、差分支路、采样或锁存结构、输出级、电源与控制端。把 `CLK` / `CLKB` / 使能 / 复位的作用点写进 `<desc>`，把端序写进 `<metadata>` 的 `terminal_order`。
3. 绘制 SVG：用 `viewBox` 管理画布，保留分组、线、路径、文字；每个外部端口用唯一的 `data-port`，并标记 `data-side` 与 `data-direction`。端口名、功能字、位宽和短节点名保持接近的字号。详见 [风格与构图](references/style.md)。
4. 渲染核验：`python3 scripts/check_svg.py symbol.svg --json` 检查端口和端序；用 `rsvg-convert` 渲染正常尺寸和父级常用缩放，检查路径接合、文字遮挡、裁切、填充遮线、节点区分和控制线落点。
5. 生成原生视图：`python3 scripts/svg2oa.py symbol.svg --lib LIB --cell CELL --view symbol --pins --pin-json ports.json -o symbol.il`。转换器生成 `dbCreatePath` / `dbCreatePolygon` / `dbCreateLabel`、真实 pin、`pin/label`、动态标签、选择框和端序设置。改写已有 cellview 时用 `--origin-x / --origin-y` 把绘图对齐到原有 pin 框，`--pin-order` 或 `metadata.terminal_order` 固定端序。详见 [原生几何](references/oa-geometry.md)。
6. 在 scratch library 应用：`VCLI_CAPABILITY=admin python3 scripts/apply_skill.py symbol.il`。脚本通过 `vcli rpc call --method skill.eval` 执行生成的 SKILL。
7. 回读验收：打开生成的 cellview，用 `symbol.inspect` 和 `skill.eval` 读回 terminals、pin order、labelType、lpp、bBox 与形状数量；截图检查完整功能组。改动 schematic 时执行 `schematic.check` 并读取检查标记。

## 写入已有设计库的授权流程

1. 回读原 symbol 的 bBox、8 个终端的方向、端序和每个 pin 图形的中心；记下父级原理图的 `schematic.check` 结果作为基线。
2. 把整个 view 目录复制到库目录下的 `.backups/` 作为回滚点，确认 `symbol.oa` 的大小与时间未变。
3. 用那个 pin 框作为画布：`viewBox` 宽高按原 bBox 反推，`--origin-x / --origin-y` 取 bBox 左下角，使每个 `data-port` 落在原 pin 中心。
4. 执行生成的 SKILL，随后读回 terminals、方向、pin 图中心、`schGetPinOrder`、bBox 和形状数；在父级重跑 `schematic.check` 并与基线对比。
5. 报告备份路径、回滚方式和残留状态。完整实测记录见 [评估记录](references/verification.md)。

## 原生对象要求

| 对象 | 要求 |
|---|---|
| 外部端口 | `schCreateSymbolPin` 创建真实 pin，方向来自 `data-direction` |
| 端口名 | `schCreateSymbolLabel` 的 `pin name`，并用 `schGlueLabel` 粘合到 pin |
| 动态标签 | `[@instanceName]` 与 `[@partName]` 使用 `NLPLabel` |
| 选择框 | `instance/drawing` 上的矩形，覆盖 `viewBox` 对应范围 |
| 功能图 | 红色端点映射 `pin/drawing`，其余图形映射 `wire/drawing` |
| 线宽 | 小于 `0.01` 的 path width 提升到 `0.01` |
| 字体 | `stick`，高度 `0.0625`，与项目已有符号一致 |
| 端序 | `schEditPinOrder` 写入 `metadata.terminal_order` 或 `--pin-order` |
| Schematic 注释 | `schCreateNoteShape` / `schCreateNoteLabel` 保留 `text/drawing*` 图层 |

## 验收清单

- 端口名称、方向、端序与 `symbol.inspect` 结果一致。
- `<desc>` 中的控制映射、极性与实际网表一致。实例 `model` 已回读。
- SVG 在正常尺寸和缩小尺寸下可读，端口标签贴近对应端点，交叉反馈没有误加连接点。
- 原生视图读回得到预期终端数、`pin/label` 标签、`NLPLabel` 动态标签、`instance/drawing` 选择框和正确 pin order。
- 截图能看到完整功能组与引出路径。父级 Check/Save 在涉及父级改动时执行。
- 报告修改范围、使用的 scratch library、检查结果和仍待处理的问题。

## 文件

- `examples/cmp_strongarm_symbol.svg`：按 `FT0001A_SH/cmp_strongarm` 实际网表绘制的功能 symbol 示例。
- `examples/cmp_strongarm_symbol.png`：示例 SVG 的正常尺寸渲染。
- `examples/cmp_strongarm_ft0001a.svg`：按真实库原有 pin 框重排的功能 symbol，可直接回迁 `FT0001A_SH/cmp_strongarm/symbol`。
- `examples/cmp_strongarm_ft0001a.png`：该 SVG 的正常尺寸渲染。
- `examples/oa-ft0001a-cmp-strongarm-symbol.png`：回迁真实库后的 Virtuoso Symbol Editor 截图。
- `examples/oa-cmp-strongarm-artist.png`：示例回迁到 Virtuoso 后的画布截图，黑底反相为白底。
- `examples/oa-cmp-strongarm-window.png`：回迁后的 Virtuoso Symbol Editor 窗口原图。
- `references/style.md`：构图、配色、端口元数据和渲染检查。
- `references/oa-geometry.md`：OA 图层、几何、标签、Note 与端序的实测值。
- `references/verification.md`：live Virtuoso 评估记录与已知限制。
- `scripts/check_svg.py`：SVG 静态检查。
- `scripts/svg2oa.py`：SVG→SKILL 转换器。
- `scripts/apply_skill.py`：通过 `vcli rpc call --method skill.eval` 应用生成的 SKILL。
- `tests/test_svg_tools.py`：三个脚本的离线单元测试。

来源与许可证边界见 [NOTICE.md](NOTICE.md)。
