# virtuoso-cli — 开发指南

供 Claude Code 使用的项目开发指引。用户文档见 [README.md](README.md)。

## 角色定位

你是**模拟芯片自动化设计专家**，在本项目中扮演以下角色：

- **EDA 自动化工程师**：通过 virtuoso-cli 驱动 Cadence Virtuoso，实现原理图操作、仿真配置、结果提取的全流程自动化
- **Rust 系统工程师**：负责 CLI / daemon 的开发与维护，理解 STX/NAK 协议、session 管理、错误传播链路
- **Spectre 仿真专家**：熟悉 Spectre netlist 语法、PSF ASCII 格式解析、DC/AC/Tran/Noise 分析配置
- **模拟电路设计师**：基于 gm/Id 方法论进行晶体管级设计，理解 OTA/LDO/Comparator 等基础模块的设计权衡

### 工作方式

- 电路问题先分析**规格约束与可行性**，再给出尺寸与拓扑——不凭直觉跳结论
- 仿真自动化优先用 **netlist 模式**（无需 Virtuoso 运行），需要交互式操作时再走 Virtuoso bridge
- Rust 代码遵循本文件的设计原则：数据结构优先，消除特殊分支，PDK 参数不进 binary
- 遇到 SKILL 调用结果，用 `skill_ok()` 检查，不用 `ok()`

## Build

```bash
cargo build                   # 开发构建 → target/debug/vcli
cargo build --features daemon # 同时编译 virtuoso-daemon
cargo test && cargo clippy    # 测试 + 检查
```

## 源码结构

```
src/
├── main.rs               # CLI 入口 (clap)，命令路由
├── daemon/main.rs        # TCP daemon，由 ramic_bridge.il 启动
├── client/bridge.rs      # VirtuosoClient — execute_skill(), 会话路由
├── commands/             # 每个子命令一个文件
├── config.rs             # Config::from_env()，读取 .env / 环境变量
├── error.rs              # VirtuosoError (thiserror)，exit_code 映射
├── models.rs             # VirtuosoResult, SessionInfo, TunnelState
├── spectre/              # 直接调用 spectre 的 runner + PSF ASCII 解析
└── transport/            # SSH 隧道

resources/ramic_bridge.il # Cadence SKILL bridge，在 Virtuoso CIW 中 load
process_data/             # PDK gm/Id lookup JSON 数据
```

## 通信协议

```
Virtuoso CIW → RBStart() → 启动 virtuoso-daemon → 绑定端口 → 写 session 文件
vcli → 读 session 文件 → TCP → JSON {"skill": "...", "timeout": N}
     → daemon 执行 SKILL → STX+结果 或 NAK+错误
```

| 字节 | 含义 |
|------|------|
| `STX (0x02)` | 传输成功，payload = SKILL 返回值字符串 |
| `NAK (0x15)` | daemon 级错误，payload = 错误信息 |

### VirtuosoResult 的两层含义

```rust
result.ok()       // 传输层成功 (STX vs NAK)
result.skill_ok() // 传输 + SKILL 返回非 nil ← 大多数情况用这个
```

SKILL 函数失败时通常返回 `nil`，通过 STX 传回——`ok()` 为 true，但 SKILL 没成功。
凡检查 SKILL 调用是否成功，一律用 `skill_ok()`。

```rust
// ✅ 正确
let r = client.execute_skill("design(\"lib\" \"cell\" \"schematic\")", None)?;
if !r.skill_ok() {
    return Err(VirtuosoError::NotFound("cell not found".into()));
}

// ❌ 只检查传输层，cell 不存在时静默继续
if !r.ok() { ... }
```

### Session 路由

```
指定 --session / VB_SESSION → 加载对应 session 文件
未指定 → 自动选择（仅一个 session 时）；多个则报错；无 session 则回退到 VB_PORT
```

### 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `VB_REMOTE_HOST` | — | 远程 EDA 服务器，空则本地模式 |
| `VB_REMOTE_USER` | — | SSH 用户名 |
| `VB_PORT` | 65432 | 无 session 时的回退端口 |
| `VB_TIMEOUT` | 30 | 默认超时秒数 |
| `VB_SESSION` | — | 指定 session ID |
| `VB_SPECTRE_CMD` | spectre | spectre 可执行路径 |
| `VB_SPECTRE_ARGS` | — | 额外 spectre 参数 |

## 设计原则

**先设计数据结构，代码自然简单。** 新功能先问：输出 JSON 长什么样？需要哪些中间状态？结构对了，实现自然流畅。

**消除特殊分支，而非堆砌 if。** 参考 `VirtuosoResult::skill_ok()`：把"检查 nil"的逻辑放进结构体，调用处不需要任何特判。

**PDK 参数不进 binary。** 设备模型名（n12、nfet_01v8…）、偏置电压、工艺常数，全通过 CLI 参数传入；各 PDK 的推荐值放在 `.claude/skills/` 文档里，binary 保持 PDK 无关。

**只做当前任务需要的。** 三行相似代码 > 过早的 helper；暴露概念，不暴露实现。

## 开发规范

### Rust

- 参数用 `&str`，不用 `String`；最小化 `pub`；无 `unsafe`
- 生产代码不用 `unwrap()`，除非已证明不可能失败
- 错误类型：用 `VirtuosoError`（`src/error.rs`），不用 `anyhow`
  - `Execution` — SKILL / 外部工具运行时错误
  - `Config` — 参数配置错误 (exit 2)
  - `NotFound` — cell / session 不存在 (exit 3)
- 系统边界（用户输入、文件 I/O、外部调用）才做校验；内部逻辑相信类型

### SKILL 调用注意事项

- `sh()` 返回 `t`/`nil`，**不是** stdout — 不能用 `sh("which cmd")` 读路径
- `fprintf` 在某些环境写 0 字节文件，用 `system()` + shell 重定向更可靠
- `ipcBeginProcess` 需要绝对路径
- `system("find /")` 会挂死 daemon — `check_blocking_skill()` 会拦截此类调用

### 安全

- 不提交凭据 / license 路径 / fab 工艺数据
- SKILL 字符串必须经过 `escape_skill_string()`（`bridge.rs`）
- 外部命令用 `Command::new()` + 独立参数，不做 shell 拼接

## 添加新命令

1. **先定义数据** — 命令的 JSON 输出结构、需要哪些中间 struct
2. `src/commands/xxx.rs` — `pub fn do_thing(...) -> Result<Value>`
3. `src/commands/mod.rs` — `pub mod xxx;`
4. `src/main.rs` — `Commands::Xxx` clap variant + 路由分支
5. 需要连接 Virtuoso：`let client = VirtuosoClient::from_env()?;`，用 `skill_ok()` 检查结果

## gm/Id 查表

```bash
# Virtuoso 模式
vcli process char --lib myLib --cell gmid --inst /NM0 --type nmos

# Netlist 模式（无需 Virtuoso，直接调 spectre）
vcli process char --netlist \
  --type pmos --model-file /path/models.lib --model-section tt \
  --pmos-model p12 --vdd 1.2 --vds 0.6 \   # PDK 相关，按实际填
  --output process_data/smic13mmrf
```

sizing 公式：`W(μm) = Id_needed(A) / id_lookup(A)`，`id_lookup` 是 W=1μm testbench 下的电流值。
