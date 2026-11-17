# 原生几何与转换

以下数值来自 Cadence Virtuoso IC26.1 的 live 实测。目标项目的数据库分辨率、display resource 和父级接口格点优先。

## 坐标与尺度

SVG 的 y 轴向下，OA 的 y 轴向上。`svg2oa.py` 使用：

```text
x_oa = origin_x + (x_svg - viewBox_x) * scale
y_oa = origin_y + (viewBox_y + viewBox_h - y_svg) * scale
```

默认 `scale = 1/256`，`origin_x / origin_y` 默认为 `0`，由 `--origin-x / --origin-y` 传入。改写已有 symbol 时，把 origin 设为原 bBox 左下角，并按原 bBox 宽高设置 `viewBox` 的宽高，这样 `data-port` 才能落在父级原理图已经连好的 pin 坐标上。目标 cellView 以 `schematicSymbol` 类型、追加模式打开，脚本末尾执行 `dbSave` 和 `dbClose`。

实测合法起点：

| 项目 | 值 |
|---|---|
| Symbol 字体 | `stick` |
| 字号 | `0.0625` |
| Snap spacing | `0.03125` |
| 电气连接格点 | `0.0625` |
| 同侧 pin pitch | `0.125` 起 |
| 最小 path width | `0.01` |
| 合法文字高度 | `0.03125`、`0.0375`、`0.05`、`0.0625`、`0.075`、`0.09375`、`0.125`、`0.1875` |

`0.00625`、`0.005`、`0.0025`、`0.001` 被当前技术拒绝。`svg2oa.py` 把小于 `--min-width` 的线宽提升到合法值。

## 图层与对象

| 用途 | 图层 / 对象 |
|---|---|
| 功能图形 | `wire/drawing` 上的 `dbCreatePath` / `dbCreatePolygon` |
| 红端口 | `pin/drawing` 上的 `dbCreatePath` / `dbCreatePolygon` |
| 普通文字 | `dbCreateLabel` |
| 端口名 | `pin/label`，`normalLabel`，`schGlueLabel` 粘合到 pin |
| 实例名 | `instance/label`，`NLPLabel`，文本 `[@instanceName]` |
| 器件名 | `device/label`，`NLPLabel`，文本 `[@partName]` |
| 选择框 | `instance/drawing` 上的 `dbCreateRect` |
| Schematic 辅助图形 | `text/drawing*` 上的 Note Shape / Note Label |

## SKILL 调用实测

`schCreateSymbolLabel` 可用 `t_labelChoice` 设置图层和标签类型：

```skill
schCreateSymbolLabel(cv list(0.5 0.5) "instance label" "[@instanceName]"
  "centerLeft" "R0" "stick" 0.0625 "NLPLabel")
schCreateSymbolLabel(cv list(0.5 0.75) "logical label" "[@partName]"
  "centerCenter" "R0" "stick" 0.0625 "NLPLabel")
schCreateSymbolLabel(cv list(0.6 0.5) "pin name" "VIP"
  "centerLeft" "R0" "stick" 0.0625 "normal")
```

`schCreateSymbolPin` 和 `schGlueLabel` 组合：

```skill
pin0 = schCreateSymbolPin(cv symPin "VIP" "input" list(0.1 1.2) "R180")
lab0 = schCreateSymbolLabel(cv list(0.15 1.2) "pin name" "VIP" "centerLeft" "R0" "stick" 0.0625 "normal")
schGlueLabel(lab0 pin0)
```

`schEditPinOrder` 写入 `portOrder`：

```skill
schEditPinOrder(cv list("VIP" "VIN" "CLKB" "CLK" "QB" "Q" "VSS" "VDD") nil)
schGetPinOrder(cv)
```

`schCreateNoteShape` 的 `t_type` 实测取值与结果：

| 调用 | objType | lpp |
|---|---|---|
| `"rectangle" "solid"` | `rect` | `text/drawing` |
| `"rectangle" "dashed"` | `rect` | `text/drawing1` |
| `"line" "solid"` | `path` | `text/drawing2` |
| `"polygon" "solid"` | `polygon` | `text/drawing` |
| `"rect" "solid"` | 报错 | 合法值为 `line`、`rectangle`、`polygon`、`arc`、`ellipse`、`circle` |

`schCreateNoteLabel(cv point text just orient font height "normalLabel")` 生成 `text/drawing` 上的 `label`。这些对象不影响连接关系，`schematic.check` 在 scratch schematic 返回 `(0 0)`。

## 回读

`vcli rpc call --method symbol.inspect` 返回 bBox、terminals、`schGetPinOrder(cv)`、`cv~>portOrder` 和 `cv~>termOrder`。`skill.eval` 可进一步枚举 `cv~>shapes`，读取 `objType`、`lpp`、`theLabel`、`labelType` 和 `bBox`。

验收时确认：

- terminals 的名称、方向、数量与 SVG 元数据一致。
- `schGetPinOrder` 与 `terminal_order` 一致。
- 端口名标签为 `pin/label` 且 `labelType` 为 `normalLabel`。
- 动态标签为 `NLPLabel`，分别位于 `instance/label` 与 `device/label`。
- 选择框位于 `instance/drawing`。
- 形状数量、bBox 和截图与 SVG 渲染一致。
