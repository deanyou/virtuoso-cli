# SKILL Runtime 深化设计

## 目标

在不破坏现有 Rust public Interface、CLI 参数和成功输出 JSON shape 的前提下，集中处理 SKILL 字符串转义、transport/SKILL 两层成功语义和返回值解码。

本阶段只实现架构审视报告的最高优先级候选，不同时重构 Ops/Editor、execution context、CLI schema、SSH transport 或 gm/Id 数据模型。

## 兼容性约束

- 保留 `VirtuosoClient::execute_skill()` 的方法签名和 raw SKILL 能力。
- 保留 `VirtuosoResult::ok()` 与 `VirtuosoResult::skill_ok()`。
- 保留 `client::bridge::escape_skill_string()` 的 public 路径。
- 保留现有 CLI 参数、命令名称和成功输出字段。
- 保留 session、profile、SSH tunnel 和 Spectre Job 行为。
- `skill exec` 继续采用 transport-level 成功语义。

## Architecture

新增内部 `client::skill_runtime` Module。它位于领域命令与 `VirtuosoClient::execute_skill()` 之间，集中四类行为：

1. 生成安全的 SKILL string literal。
2. 规范化 SKILL 返回字符串。
3. 按调用语义检查 transport success 或 non-nil success。
4. 解码普通 JSON 和被 SKILL string 包装的 JSON。

`VirtuosoClient::execute_skill()` 继续只负责：

- TCP connect/read/write；
- timeout；
- STX/NAK；
- response size 限制；
- command logging。

raw SKILL source code 不经过结构化 builder。只有 `skill exec` 和明确接收用户表达式的 measurement Interface 可以继续传递 raw expression。

## Runtime Interface

SKILL runtime 提供以下内部能力：

### String literal

将任意 Rust `&str` 编码为完整的 SKILL string literal，包括外围双引号。编码必须处理：

- `\`；
- `"`；
- 换行；
- 回车；
- 空字符串。

已有 `escape_skill_string()` 保持为兼容转发，只返回不含外围双引号的转义内容。新代码优先使用完整 literal，避免调用者忘记加引号。

### `require_transport`

要求 `VirtuosoResult::ok()` 为 true，并返回规范化后的 output。适用于：

- measurement expression；
- `run()`；
- 明确允许以裸 `nil` 表示数据缺失的查询。

transport failure 映射为现有 `VirtuosoError::Execution`，错误文本沿用 `VirtuosoResult.errors`。

### `require_non_nil`

先执行 transport 检查，再拒绝去除空白后的裸 `nil`。字符串值 `"nil"` 是合法的非 nil 数据。

适用于：

- `design()`；
- 打开或创建 cellview；
- 创建 netlist；
- 其他以裸 `nil` 表示 SKILL 动作失败的调用。

### `decode_json`

先执行 non-nil 检查，然后按两级策略解析：

1. 直接把 output 解析为 JSON；
2. 如果外层 JSON 是字符串，取出其内容并再次解析为 JSON。

解析失败映射为 `VirtuosoError::Execution`，错误包含动作上下文，但不暴露额外的 CLI 输出字段。

## 数据流

```text
领域命令
  → SKILL literal / expression builder
  → VirtuosoClient::execute_skill()
  → VirtuosoResult
  → require_transport | require_non_nil | decode_json
  → 现有命令 JSON 输出
```

### 特殊语义

- `skill exec`：只检查 transport，raw `nil` 仍可作为成功输出。
- `run()`：允许返回裸 `nil`；继续通过 `spectre.out` 是否存在判断仿真是否真正执行。
- measurement：允许返回裸 `nil`，并保留现有 warnings/diagnostics 行为。
- `parse_skill_json()`：从 `commands::schematic` 移入 runtime，Maestro 不再反向依赖 schematic noun。

## 迁移范围

### `src/commands/sim.rs`

- 对 analysis/setup 等动作使用 non-nil 语义时才拒绝裸 `nil`。
- 对 `run()`、measurement 使用 transport 语义。
- 不改变已有的 `spectre.out` 后验验证。

### `src/commands/process.rs` 与 `src/ocean/mod.rs`

- lib、cell、view、instance、results directory、analysis 参数名和值使用统一 literal/identifier 生成规则。
- raw measurement expression 保持 raw，不把表达式误编码成 string literal。
- 数字和 `t`/`nil` 等 SKILL 原子保持原子语义。

### `src/commands/schematic.rs` 与 `src/commands/maestro.rs`

- 删除 schematic noun 内的通用 JSON 解码 Implementation。
- 两者通过 runtime 的同一个 JSON Interface 解码。
- 返回 JSON shape 保持不变。

## 错误处理

- STX/NAK、timeout 和 socket error 的 transport 分类保持不变。
- 原本需要 non-nil、却只检查 `ok()` 的调用将不再静默继续。
- 合法的 data-level `nil` 不升级为错误。
- 成功输出 JSON shape 不变；失败继续通过现有 `CliError` 和语义化 exit code 输出。

## 测试策略

采用 TDD，每个行为先写失败测试并确认失败原因，再写最小 Implementation。

### Runtime 单元测试

- string literal 正确处理引号、反斜杠、换行、回车和空字符串；
- transport success 返回 output；
- transport error 返回 `VirtuosoError::Execution`；
- non-nil 检查拒绝带空白的裸 `nil`；
- non-nil 检查接受字符串 `"nil"`；
- JSON decoder 接受普通 JSON；
- JSON decoder 接受 SKILL string 包装的 JSON；
- JSON decoder 拒绝非法 JSON，并包含动作上下文。

### OCEAN builder 回归测试

- design target 中的双引号、反斜杠和换行不会逃逸出 string literal；
- analysis string 参数被正确编码；
- 数字、`t`、`nil` 保持 SKILL 原子；
- measurement expression 保持 raw expression。

### 验证命令

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

## 非目标

- 不引入完整 typed SKILL AST。
- 不新增 CLI 命令。
- 不改变 daemon wire protocol。
- 不增加 safe/readonly 权限模式。
- 不创建新的 SSH Adapter 或 execution context Seam。
- 不顺带修复与 SKILL runtime 无关的领域行为。

## 完成标准

- 新 runtime Module 成为 schematic、Maestro 和结构化 OCEAN 调用的共同解码/转义位置。
- `commands::schematic` 不再拥有通用 JSON parser。
- 需要 non-nil 的结构化调用不再只依赖 transport-level `ok()`。
- raw `skill exec`、`run()` 和 measurement 的合法 `nil` 行为保持兼容。
- 全部测试和严格 Clippy 通过。
