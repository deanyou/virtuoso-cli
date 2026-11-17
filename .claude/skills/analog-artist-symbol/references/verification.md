# live Virtuoso 评估记录

评估环境：Cadence Virtuoso IC26.1，`vcli` RPC bridge，`VCLI_CAPABILITY=admin`，X11 截图，OA scratch library `artistEval`。目标电路为 `FT0001A_SH/cmp_strongarm`，端口为 `VIP`、`VIN`、`CLK`、`CLKB`、`VDD`、`VSS`、`Q`、`QB`。

> 脱敏说明：本文件面向公开仓库。本机绝对路径、工作库名、备份时刻、主机名、用户名和会话标识统一替换为 `<...>` 占位符；命令结构、报错原文、测试计数与几何数值原样保留。`examples/` 里两张 OA 窗口截图的 Property Editor 用 `<USER>` 代替了操作者账号字段。

## 实测步骤

1. 用 `symbol.inspect` 读取真实 symbol 的 bBox、端口方向、端序。
2. 从 scratch 复制的 schematic 读取实例 `model` 和网名。`M5` / `M6` 为 `nch`，`CLK` 同时控制 `M7` / `M8` 预充电管与 `M9` 尾管，`CLKB` 控制 `M12` 的 latch source 供电。
3. 按网表绘制功能 SVG，渲染正常尺寸与缩小尺寸，核对端口、控制线、交叉反馈、文字和裁切。
4. 用 `svg2oa.py --pins` 生成 SKILL，在 scratch library 创建 `cmp_strongarm_artist/symbol`。
5. 读回 `symbol.inspect`、terminals、`schGetPinOrder`、labelType、lpp、bBox 和形状数量；打开 cellview 截图。
6. 在 `artistEval/note_demo/schematic` 验证 `schCreateNoteShape`、`schCreateNoteLabel` 和 `schematic.check`。

## 实测结果

| 检查 | 结果 |
|---|---|
| SVG 端口 | 8 个唯一 `data-port`，方向与 `symbol.inspect` 一致 |
| SVG 端序 | `VIP, VIN, CLKB, CLK, QB, Q, VSS, VDD`，与真实 symbol terminal 顺序一致 |
| 生成 SKILL | 全新 scratch cell 一次执行成功，`skill.eval` 返回 `"OK"` |
| 原生 terminals | 8 个，方向为 4 个 `input`、2 个 `output`、2 个 `inputOutput` |
| 原生形状 | 85 个 shape，bBox `(0 0)(3.2 1.8375)` |
| 动态标签 | `[@instanceName]` 在 `instance/label`，`[@partName]` 在 `device/label`，均为 `NLPLabel` |
| 端口标签 | 8 个 `pin/label`、`normalLabel`，`schGlueLabel` 返回 `t` |
| pin order | `schGetPinOrder` 与 `cv~>portOrder` 均为预期端序 |
| 选择框 | `instance/drawing` 上的矩形 |
| Schematic Note | `rectangle` / `line` / `polygon` / `dashed` 分别落在 `text/drawing`、`text/drawing2`、`text/drawing`、`text/drawing1` |
| `schematic.check` | `(0 0)` |

示例 SVG 渲染与 OA 窗口截图保存在 `examples/`：

- `examples/cmp_strongarm_symbol.png`：源 SVG 的渲染。
- `examples/oa-cmp-strongarm-artist.png`：OA 画布截图，黑底反相为白底。
- `examples/oa-cmp-strongarm-window.png`：Virtuoso Symbol Editor 窗口原图。

## 转换器修正记录

首次回迁时 OA 里的红色端口标签压在下方功能名上，例如 `CLKB` 压住 `PRECHARGE`。原因是两套坐标语义不同：SVG 的 `<text>` 的 `y` 是文字基线，OA 的 `centerLeft` / `centerRight` 以文字垂直中心为锚点，而转换器把锚点往下挪了 `0.3 * text_height`。修正为向上抬 `0.35 * text_height` 后，OA 标签与 SVG 基线对齐。`tests/test_svg_tools.py` 的 `test_pin_label_sits_above_its_pin` 锁住这个方向：把偏移改回旧值，该测试会失败。

源 SVG 自身也有两处排布问题，一并修掉：`LATCH` 标签原先落在 CLKB 控制竖线上，移到锁存器方框内部；`CLKB: latch_src` 上移并把控制线转折点从 `x=430` 改到 `x=392`，两者不再相交。

## 回迁真实设计库 `FT0001A_SH/cmp_strongarm/symbol`

在获得单独授权后，把 artist symbol 直接写入用户正在使用的 cellview。原始 pin 框是验收契约：改写外观时端口名、方向、端序和每个 pin 中心必须逐点保持不变，否则父级 `cmp_strongarm_tb/schematic` 的连接会断。

| 项目 | 原始值 | 回写后 |
|---|---|---|
| bBox | `(-0.025 -0.625) (2.16875 0.525)` | 相同 |
| 终端 | 8 个；`VIP/VIN/CLKB/CLK` 为 `input`，`Q/QB` 为 `output`，`VDD/VSS` 为 `inputOutput` | 相同 |
| pin 中心 | `VIP(0 -0.375)`、`VIN(0 -0.25)`、`CLKB(0 -0.125)`、`CLK(0 0)`、`Q(1.6875 -0.125)`、`QB(1.6875 0)`、`VDD(0.8125 0.5)`、`VSS(0.9375 0.5)` | 相同 |
| `schGetPinOrder` | `Q,QB,VDD,VSS,CLK,CLKB,VIN,VIP` | 相同 |
| shapes | 28 | 81（35 paths、16 polygons、功能标签、8 个 pin 图与端口名、2 个动态标签、1 个选择框） |
| 父级 `schematic.check` | `(0 4)` | `(0 4)` |
| 父级实例 | `I0` → `FT0001A_SH/cmp_strongarm`，位置 `(0 20)` | 相同 |

画布 `viewBox="0 0 561.6 294.4"` 与 `--origin-x -0.025 --origin-y -0.625` 让选择框恰好等于原 bBox。`vy + vh` 恒为 `294.4`，所以收紧画布顶部的空白不会改动任何端口坐标。

备份（磁盘目录复制，含 `master.tag`、`symbol.oa`、缩略图）。公开版本把本地工作库路径与备份时刻换成占位符：`<WORK_LIB>` 是工作库根目录，`<TIMESTAMP>` 是备份时刻。

```text
<WORK_LIB>/.backups/cmp_strongarm_symbol_<TIMESTAMP>/symbol/   原始 symbol.oa
<WORK_LIB>/.backups/cmp_strongarm_artist_<TIMESTAMP>/symbol/   artist 回写版
```

回滚时关闭该 cellview 的所有窗口与编辑态对象，把对应备份目录的内容复制回 `FT0001A_SH/cmp_strongarm/symbol/`，再重新打开视图。示例文件为 `examples/cmp_strongarm_ft0001a.svg`，窗口截图为 `examples/oa-ft0001a-cmp-strongarm-symbol.png`。

### 转换器在真实库上暴露的修正

- 清理已有视图时，terminal 拥有 pin、pin 拥有 pin 图形，必须先删 `cv~>terminals` 再删 shapes 与 instances，否则 `schCreateSymbolPin` 会因同名终端报错。
- 同一次 SKILL 求值内 `length(cv~>terminals)` 读到的是删除前的快照，用 `when(length(...) error(...))` 做清理校验会误报并中断；随后的独立读回显示删除其实已经生效。改为检查每个 `schCreateSymbolPin` 的返回句柄是否为 `nil`（`pinfail` 计数），失败即报错。
- `snap_text_height` 原先总是向上取合法高度，13px 标注会被吸到和 16px 相同的 `0.0625`。改为取最接近的合法高度后，次级标注为 `0.05`，在 OA 画布里与端口名拉开距离。

## 已知限制

上游方法论能提示构图、控制映射和渲染检查。模型在没有强制回读网表时，会把 `nch` 输入对描述成 PMOS 输入对并写反 `Q` / `QB` 极性。本技能因此把实例 `model`、网名和 `symbol.inspect` 回读列为第一步，并要求 `<desc>` 与 `<metadata>` 记录控制作用和端序。

SVG→OA 转换按颜色映射为两层图形：`pin/drawing` 与 `wire/drawing`。目标项目若有特殊 display resource、动态标签表达式或选择框约定，需要按项目设置调整转换脚本。

live SKILL 求值的几处方言限制：`t` 是保留字，不能作 `lambda` 形参（用 `term` / `x` / `fig`）；`lambda` 体内不能出现 `if`，否则表现为 `mapcar: too few arguments`；`sprintf` 的首参需要符号常量，直接返回 `list(...)` 给 RPC 更省事；`car` 在本环境未绑定，读 bBox 时直接把原始 `((x0 y0) (x1 y1))` 返回到 Python 侧再算中心点。

bridge 操作风险：无参 `hiZoomIn()` 会进入交互式框选缩放并等待鼠标点击，`skill.eval` 因此超时，daemon 会把该会话的 SKILL 通道标记为挂死。恢复办法是在 CIW 里执行 `RBStop()` 再 `RBStart()`，会生成新的会话 id 与端口；之后所有 `vcli` 调用要带 `--session <新 id>`。截图优先用 X11 单窗口路径（`import -window <id> png:<path>`），`window.screenshot_by_pattern` 内部依赖 `system(` 会被白名单拦下。

`cell.open` 用 `r` 模式打开一个仍以 `a` 模式挂在会话里的 cellview 会报 "already open in edit mode"，并留下空窗口和确认弹窗。先用 SKILL 枚举 `dbGetOpenCellViews()` 并对目标 cellview 执行 `dbClose(cv)`，再重开；关闭 GUI 窗口后残留的 `cdslck` 是过期编辑锁，确认没有编辑态 cellview 后删除即可。

清理 scratch library 时不要调用 `ddDeleteObj(ddGetObj("artistEval"))`。实测它会改写工作目录的 `cds.lib`，把相邻的 `DEFINE FT0001A_SH` 行注释为 `#Removed by ddDeleteObj`，同时清空 scratch 库目录。安全做法是先关闭所有 scratch cellview、从 `cds.lib` 移除本次新增的 `DEFINE`，再直接删除 `/tmp` 下的 scratch 目录；会话内的库名残留会在 Virtuoso 重启后消失。

上游技能为 GPL-3.0，本仓为 MIT。许可证边界见 [NOTICE.md](../NOTICE.md)。
