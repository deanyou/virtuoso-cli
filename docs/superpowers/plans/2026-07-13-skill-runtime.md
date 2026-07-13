# SKILL Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 集中 SKILL string literal、transport/non-nil 成功语义和 JSON 解码，同时保持现有 Rust public Interface 与 CLI 成功输出兼容。

**Architecture:** 新增内部 `client::skill_runtime` Module，`VirtuosoClient` 继续只处理 TCP 与 STX/NAK。结构化命令通过 runtime 检查和解码结果，raw `skill exec`、`run()` 与 measurement 保持 transport-level 语义。

**Tech Stack:** Rust 2021、serde_json、现有 VirtuosoResult/VirtuosoError、Cargo test、Clippy

---

## 文件结构

- Create: `src/client/skill_runtime.rs` — SKILL literal、结果检查、JSON 解码及其单元测试。
- Modify: `src/client/mod.rs` — 注册内部 runtime Module。
- Modify: `src/client/bridge.rs` — public `escape_skill_string()` 转发到 runtime。
- Modify: `src/commands/schematic.rs` — 使用 runtime JSON decoder，删除通用 parser。
- Modify: `src/commands/maestro.rs` — 使用 runtime JSON decoder，删除对 schematic noun 的依赖。
- Modify: `src/ocean/mod.rs` — 结构化 string 参数使用完整 literal，并增加 builder 回归测试。
- Modify: `src/commands/process.rs` — design/getData 的 string 参数使用完整 literal。
- Modify: `src/commands/sim.rs` — 按动作语义使用 transport/non-nil 检查。

### Task 1: SKILL string literal

**Files:**
- Create: `src/client/skill_runtime.rs`
- Modify: `src/client/mod.rs`
- Modify: `src/client/bridge.rs`

- [ ] **Step 1: 写失败测试**

在 `skill_runtime.rs` 添加测试，声明期望 Interface：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_literal_escapes_skill_control_characters() {
        assert_eq!(
            string_literal("a\\b\"c\nd\re"),
            "\"a\\\\b\\\"c\\nd\\re\""
        );
    }

    #[test]
    fn string_literal_handles_empty_string() {
        assert_eq!(string_literal(""), "\"\"");
    }
}
```

- [ ] **Step 2: 确认 RED**

Run: `cargo test skill_runtime::tests::string_literal -- --nocapture`

Expected: FAIL，因为 `string_literal` 尚未定义。

- [ ] **Step 3: 写最小 Implementation**

```rust
pub(crate) fn escape_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub(crate) fn string_literal(value: &str) -> String {
    format!("\"{}\"", escape_string(value))
}
```

在 `client/mod.rs` 注册 `pub(crate) mod skill_runtime;`。将 `bridge::escape_skill_string()` 改为调用 `skill_runtime::escape_string()`，保持原方法路径与返回形状。

- [ ] **Step 4: 确认 GREEN**

Run: `cargo test skill_runtime::tests::string_literal -- --nocapture`

Expected: 相关测试全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/client/skill_runtime.rs src/client/mod.rs src/client/bridge.rs
git commit -m "refactor: centralize SKILL string literals"
```

### Task 2: transport/non-nil 结果语义

**Files:**
- Modify: `src/client/skill_runtime.rs`

- [ ] **Step 1: 写失败测试**

构造真实 `VirtuosoResult`，覆盖 transport error、裸 `nil` 与字符串 `"nil"`：

```rust
#[test]
fn require_transport_accepts_data_nil() {
    let result = VirtuosoResult::success("nil");
    assert_eq!(require_transport(&result, "measure").unwrap(), "nil");
}

#[test]
fn require_non_nil_rejects_bare_nil() {
    let result = VirtuosoResult::success("  nil\n");
    let error = require_non_nil(&result, "open design").unwrap_err();
    assert!(error.to_string().contains("open design"));
}

#[test]
fn require_non_nil_accepts_string_nil() {
    let result = VirtuosoResult::success("\"nil\"");
    assert_eq!(require_non_nil(&result, "read value").unwrap(), "\"nil\"");
}

#[test]
fn require_transport_maps_result_error() {
    let result = VirtuosoResult::error(vec!["daemon rejected request".into()]);
    let error = require_transport(&result, "run analysis").unwrap_err();
    assert!(error.to_string().contains("daemon rejected request"));
}
```

- [ ] **Step 2: 确认 RED**

Run: `cargo test skill_runtime::tests::require_ -- --nocapture`

Expected: FAIL，因为检查函数尚未定义。

- [ ] **Step 3: 写最小 Implementation**

```rust
pub(crate) fn require_transport<'a>(
    result: &'a VirtuosoResult,
    action: &str,
) -> Result<&'a str> {
    if result.ok() {
        return Ok(result.output.trim());
    }
    let detail = if result.errors.is_empty() {
        "transport failed".to_string()
    } else {
        result.errors.join("; ")
    };
    Err(VirtuosoError::Execution(format!("{action}: {detail}")))
}

pub(crate) fn require_non_nil<'a>(
    result: &'a VirtuosoResult,
    action: &str,
) -> Result<&'a str> {
    let output = require_transport(result, action)?;
    if output == "nil" {
        return Err(VirtuosoError::Execution(format!("{action}: SKILL returned nil")));
    }
    Ok(output)
}
```

- [ ] **Step 4: 确认 GREEN**

Run: `cargo test skill_runtime::tests::require_ -- --nocapture`

Expected: 四个测试 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/client/skill_runtime.rs
git commit -m "refactor: encode SKILL result expectations"
```

### Task 3: JSON decoder 与 noun 解耦

**Files:**
- Modify: `src/client/skill_runtime.rs`
- Modify: `src/commands/schematic.rs`
- Modify: `src/commands/maestro.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn decode_json_accepts_direct_json() {
    let result = VirtuosoResult::success(r#"{"name":"M1"}"#);
    assert_eq!(decode_json(&result, "instances").unwrap()["name"], "M1");
}

#[test]
fn decode_json_accepts_skill_wrapped_json() {
    let result = VirtuosoResult::success(r#""{\"name\":\"M1\"}""#);
    assert_eq!(decode_json(&result, "instances").unwrap()["name"], "M1");
}

#[test]
fn decode_json_rejects_invalid_payload() {
    let result = VirtuosoResult::success("not-json");
    let error = decode_json(&result, "instances").unwrap_err();
    assert!(error.to_string().contains("instances"));
}
```

- [ ] **Step 2: 确认 RED**

Run: `cargo test skill_runtime::tests::decode_json -- --nocapture`

Expected: FAIL，因为 `decode_json` 尚未定义。

- [ ] **Step 3: 写最小 Implementation**

```rust
pub(crate) fn decode_json(result: &VirtuosoResult, action: &str) -> Result<Value> {
    let output = require_non_nil(result, action)?;
    let outer: Value = serde_json::from_str(output).map_err(|error| {
        VirtuosoError::Execution(format!("{action}: invalid JSON result: {error}"))
    })?;
    match outer {
        Value::String(inner) => serde_json::from_str(&inner).map_err(|error| {
            VirtuosoError::Execution(format!("{action}: invalid wrapped JSON result: {error}"))
        }),
        value => Ok(value),
    }
}
```

将 schematic 的四个 reader 和 Maestro session listing 改为调用 `decode_json()`；删除 `commands::schematic::parse_skill_json()` 以及 Maestro 对它的 import。

- [ ] **Step 4: 确认 GREEN 与兼容输出**

Run: `cargo test decode_json -- --nocapture`

Expected: decoder 测试 PASS。

Run: `cargo test`

Expected: 全部现有测试 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/client/skill_runtime.rs src/commands/schematic.rs src/commands/maestro.rs
git commit -m "refactor: centralize SKILL JSON decoding"
```

### Task 4: OCEAN 与 process string 参数安全迁移

**Files:**
- Modify: `src/client/skill_runtime.rs`
- Modify: `src/ocean/mod.rs`
- Modify: `src/commands/process.rs`

- [ ] **Step 1: 写失败测试**

在 `ocean/mod.rs` 添加：

```rust
#[test]
fn setup_skill_uses_safe_string_literals() {
    let skill = setup_skill("lib\"x", "cell\\x", "schematic\nnext", "spectre");
    assert!(skill.contains(r#"design("lib\"x" "cell\\x" "schematic\nnext")"#));
}

#[test]
fn analysis_string_values_are_escaped() {
    let mut params = HashMap::new();
    params.insert("stop".into(), "1u\" injected".into());
    let skill = analysis_skill_simple("tran", &params);
    assert!(skill.contains(r#"?stop "1u\" injected""#));
}

#[test]
fn analysis_atoms_remain_unquoted() {
    let params = HashMap::from([
        ("saveOppoint".into(), "t".into()),
        ("points".into(), "10".into()),
    ]);
    let skill = analysis_skill_simple("dc", &params);
    assert!(skill.contains("?saveOppoint t"));
    assert!(skill.contains("?points 10"));
}

#[test]
fn identifier_rejects_skill_injection() {
    let error = require_identifier("tran) system(\"bad\")", "analysis").unwrap_err();
    assert!(error.to_string().contains("analysis"));
}
```

- [ ] **Step 2: 确认 RED**

Run: `cargo test ocean::tests -- --nocapture`

Expected: `setup_skill_uses_safe_string_literals` 和 `analysis_string_values_are_escaped` FAIL；identifier 测试因函数未定义而 FAIL。

- [ ] **Step 3: 写最小 Implementation**

`setup_skill()` 使用 `string_literal()` 生成 lib/cell/view；`analysis_skill()` 和 `analysis_skill_simple()` 的 string 值使用完整 literal，数字与 `t`/`nil` 保持原子。

在 runtime 增加只接受 ASCII letter/number/underscore、且首字符不能为数字的 `require_identifier()`。它返回现有 `VirtuosoError::Config`，供 Task 5 在 CLI system boundary 校验 analysis type 和 keyword name；不改变现有 public OCEAN builder 签名。

`commands/process.rs` 的 design target、results directory 和 `getData()` signal name 使用 `string_literal()`，不改变数值表达式。

- [ ] **Step 4: 确认 GREEN**

Run: `cargo test ocean::tests -- --nocapture`

Expected: OCEAN tests PASS。

Run: `cargo test`

Expected: 全部测试 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/client/skill_runtime.rs src/ocean/mod.rs src/commands/process.rs
git commit -m "fix: escape structured OCEAN string values"
```

### Task 5: sim 命令采用显式结果语义

**Files:**
- Modify: `src/commands/sim.rs`

- [ ] **Step 1: 确认已有 GREEN 基线**

Task 2 的 RED/GREEN 已锁定 transport/non-nil 行为，Task 4 已锁定 identifier validation。本 Task 是这些已测试 Interface 的 REFACTOR 阶段，不新增 runtime 行为。

Run: `cargo test skill_runtime::tests -- --nocapture`

Expected: runtime tests 全部 PASS。

- [ ] **Step 2: 迁移调用点**

- setup、analysis、sweep、corner、results directory 等需要结果的动作使用 `require_non_nil()`；
- `run()` 与 measurement expression 使用 `require_transport()`；
- `spectre.out` 后验检查、warnings 和 CLI JSON shape 保持不变；
- netlist fallback 用 `require_non_nil(...).ok()` 判断每种方法，不提前中止 fallback。
- 在调用 OCEAN builder 前，用 `require_identifier()` 校验 analysis type 和 analysis keyword name；corner config 同样在文件输入 Seam 校验。

- [ ] **Step 3: 验证 REFACTOR 保持 GREEN**

Run: `cargo test`

Expected: 全部测试 PASS。

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: exit 0，无 warning。

- [ ] **Step 4: 提交**

```bash
git add src/client/skill_runtime.rs src/commands/sim.rs
git commit -m "fix: apply explicit SKILL result semantics"
```

### Task 6: 最终兼容性审计

**Files:**
- Review: `src/client/bridge.rs`
- Review: `src/commands/skill.rs`
- Review: `src/commands/sim.rs`
- Review: `src/commands/schematic.rs`
- Review: `src/commands/maestro.rs`

- [ ] **Step 1: 检查 public Interface**

Run: `rg -n "pub fn (execute_skill|escape_skill_string)|pub fn (ok|skill_ok)" src`

Expected: 现有方法仍存在，签名未改变。

- [ ] **Step 2: 检查 noun 反向依赖与旧 parser**

Run: `rg -n "parse_skill_json|commands::schematic" src/commands/maestro.rs src/commands/schematic.rs`

Expected: 无匹配。

- [ ] **Step 3: 检查 raw Interface**

确认 `commands/skill.rs` 仍直接调用 `execute_skill()`，且 `run()`/measurement 没有使用 non-nil expectation。

- [ ] **Step 4: 最终验证**

Run: `cargo test && cargo clippy --all-targets --all-features -- -D warnings`

Expected: exit 0，全部测试通过，无 warning。

- [ ] **Step 5: 确认工作区范围**

Run: `git status --short`

Expected: 只显示用户原有未跟踪文件；本计划的源码修改均已提交。
