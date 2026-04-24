# TODO: createNetlist 路径 CMI-2116 问题

## 状态
待解决

## 问题描述

对于 SMIC 0.13µm mmRF PDK（n33/p33 BSIM3v3），Cadence 所有 netlist 导出路径均不写入 MOSFET 端口连接：

```spectre
M2 n33 w=(280n) l=350n ...    ← 无 d g s b 连接（CMI-2116）
```

端口映射储存于 OA 二进制数据库，spectre 须以 `-env ade +adespetkn=TOKEN` 在运行时查询。

## 已验证失败的路径（2026-04-19）

| 方式 | 结果 |
|------|------|
| `vcli sim netlist` → Ocean `createNetlist()` | nil（需要 ADE L 窗口） |
| `asiCreateNetlist(fnxSession0)` | 生成 netlist，但仍无端口 |
| 已有 `runSimulation` 里的 token | CMI-2116（token stale/不解析端口） |
| `asiGetAdespetkn()` | 函数不存在 |

## 当前 Workaround

SKILL 拓扑提取：`dbOpenCellViewByType` + `instTerms` → 手动写端口连接 → standalone spectre。
见：`spectre-cmi-2116-ade-netlist`、`2026-04-19-standalone-spectre-for-one-off-verification`。

## 待调查方向

1. **触发真实 Maestro run 获取 token**：让 Maestro 完整跑一次 spectre（可能需要 GUI 点击），读取生成的 `runSimulation` 里的 token，后续复用。需确认 token 有效期。
2. **`spectre -odb` 或 OA 直出端口**：调查 Spectre 是否有其他标志可从 OA 直接导出完整 netlist（不依赖 ADE session token）。
3. **PDK netlister 配置**：检查 SMIC PDK 的 `nlFormatterClass` / CDF `simInfo` 是否有选项强制写入端口连接。
4. **`vcli sim netlist` 改为调用 `asiCreateNetlist`**：目前调用 Ocean `createNetlist()`；改为 Maestro API 后至少能生成完整 model include 和 cell 骨架，再由 SKILL 提取补充端口。

## 补充：IC23.1 Maestro API 全部阻塞 bridge（2026-04-20）

`maeRunSimulation`、`maeCreateNetlist`、`asiCreateNetlist` 在 IC23.1 ADE Explorer 中均阻塞 SKILL bridge（触发 GUI 进度 modal）。无可用的非阻塞 run API。

**结论**：Maestro API 不可用于 vcli 自动化 run。

**唯一可靠自动化路径**：独立 spectre + Maestro netlist（已验证，n12/p12 正常，0 error）。

