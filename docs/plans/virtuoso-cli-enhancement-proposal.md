# virtuoso-cli AI-First EDA Infrastructure Enhancement Proposal

## 概述

这份文档将 [deanyou/virtuoso-cli](https://github.com/deanyou/virtuoso-cli) 的架构评估与升级建议整理为结构化格式，供作者和社区参考。

**评估结论：** 这不是一个玩具级 CLI，而是一个相当完整的 EDA Agent Infrastructure。它已经隐约在把 Virtuoso 变成 "Agent Operating System"，方向完全正确。当前最关键的升级路径是：**从字符串自动化（string bridge）进化为类型化 EDA 平台（typed EDA platform）**。

---

## 一、当前架构优点

### 1. Session Registry + Dynamic Port

非常专业的设计。很多 bridge 工具写死端口、单实例、session 混乱，而这个项目已经实现了：
- port 0 自动分配（OS 分配）
- session registry
- auto-discovery

本质已经接近 tmux、docker daemon、ssh-agent 这种"本地基础设施层"。

### 2. JSON-first CLI

```bash
vcli schematic read --format json
```

意味着 humans 可读、agents 可消费、pipelines 可组合。这不是传统 CLI，而是 **Agent-compatible system API**，是未来趋势。

### 3. noun-verb command structure

```bash
vcli session list
vcli skill exec
vcli sim run
```

比 `--run-sim` 这种 spaghetti CLI 强很多，符合 Unix 传统。

### 4. Rust 技术选型

非常适合 daemon、async jobs、TCP bridge、TUI、SSH multiplexing。EDA 环境通常长生命周期、大量状态、多线程，Rust 比 Python 稳定得多。

---

## 二、目前最大的结构问题

真正的问题不在 CLI，而在：**系统抽象层次还不够高**。

目前本质是：

```
CLI -> skill string -> evalstring()
```

这会导致长期问题：
- schema 不稳定
- agent 极容易 hallucinate（因为无类型约束）
- skill 拼接危险
- command 不可推理
- 类型系统缺失

---

## 三、升级路线图（按价值优先级排序）

### Phase 1 — 最关键（安全 + 稳定性）

#### 1. Typed RPC Layer（第一优先级）

**现状：**
```bash
vcli skill exec "(dbOpenCellViewByType ...)"
```

问题：无 schema、无 compile-time validation、AI 极容易 hallucinate。

**目标：**
```json
{
  "method": "schematic.create_instance",
  "params": {
    "lib": "analogLib",
    "cell": "nmos4",
    "x": 10,
    "y": 20
  }
}
```

**架构：**
```
RPC layer
  ↓
SKILL adapter
  ↓
Virtuoso
```

需要：
- 为每个操作定义 JSON Schema
- 从 Schema 自动生成 Rust 类型 + CLI 参数解析
- 保留 skill exec 作为向后兼容的 escape hatch

#### 2. 安全加固（刻不容缓）

当前 `evalstring` 是最大隐患。本质是 Virtuoso 的 root shell。

**A. Whitelist RPC**
定义可调用的白名单方法，禁用任意 evalstring。

**B. Sandbox Mode**
```bash
vcli --safe
```
禁掉：ipc、filesystem、shell、destructive APIs。

**C. Readonly Session**
```bash
vcli --readonly
```
只允许：inspect、query、export。

**D. 权限系统**
```json
{
  "agent": "layout-agent",
  "permissions": ["read_schematic", "modify_layout", "run_drc"]
}
```

#### 3. MCP（Model Context Protocol）支持（战略优先级）

项目天然适合 MCP，建议直接：

```bash
vcli mcp serve
```

**为什么 MCP 是最值得优先做的：**

这是成本最低、收益最高的升级：
- **无需修改核心代码**：作为独立 crate，在外层包装即可
- **生态红利**：直接接入 Claude Desktop、Cursor、OpenHands、ChatGPT Roo、Aider、Zed 等所有主流 AI 工具
- **双向价值**：AI agent 可以调用 Virtuoso，Virtuoso 的设计数据可以反向给 AI 上下文
- **typed RPC 天然对齐**：MCP 的 tools 机制正好是 typed RPC 的前端

**MCP Tool 设计示例：**

```json
{
  "name": "vcli_schematic_create_instance",
  "description": "在原理图中创建元件实例",
  "inputSchema": {
    "type": "object",
    "properties": {
      "lib": { "type": "string", "description": "库名" },
      "cell": { "type": "string", "description": "元件名" },
      "view": { "type": "string", "default": " schematic" },
      "x": { "type": "number", "description": "X 坐标" },
      "y": { "type": "number", "description": "Y 坐标" }
    },
    "required": ["lib", "cell"]
  }
}
```

**MCP Resources 设计：**

```json
{
  "uri": "vcli://sessions",
  "name": "Virtuoso Sessions",
  "mimeType": "application/json"
}
```

```
vcli://library/{lib}/cell/{cell}/views      # design hierarchy
vcli://session/{id}/jobs                   # active simulation jobs
vcli://simulation/{id}/waveforms           # latest waveforms
```

**MCP Prompts 设计：**

```
vcli://prompts/schematic-review    # 帮我审查这张原理图
vcli://prompts/sim-setup           # 帮我设置 ADE 仿真
vcli://prompts/layout-debug        # 帮我分析 LVS 错误
```

**技术实现路径：**

```
1. 新建 crates/vcli-mcp/
2. 实现 mcp-server crate（基于 mcp-rs）
3. 将现有 CLI 命令映射为 MCP tools
4. 添加 resource provider（session/job/artifact 数据）
5. 注册为 vcli mcp serve 子命令
6. 提供 Claude Desktop config 示例
```

---

### Phase 2 — 架构升级

#### 4. EDA Object Model

现在命令还是 procedural，未来应该变成：

```
Library
  └── Cell
        └── View
              ├── Instance
              ├── Net
              ├── Pin
              └── Shape
```

Agent 应该操作 typed objects + graph + hierarchy，而不是字符串。

#### 5. Design Diff Engine

AI EDA 核心能力之一：
```bash
vcli schematic diff old new
```
输出：
```json
{
  "instances_added": [],
  "nets_modified": [],
  "params_changed": []
}
```

AI 才能做 reasoning、review、self-correction，否则 agent 没有世界模型。

#### 6. Transaction / Rollback System

```bash
vcli tx begin
vcli tx diff
vcli tx commit
vcli tx rollback
```

EDA 非常需要这个，AI 操作连错线、删除 cell、覆盖 layout 很危险。

---

### Phase 3 — AI-Native 演进

#### 7. Graph-based Design Representation

电路本质是 graph。内部统一使用 property graph（类似 Neo4j/NetworkX），极大提升 retrieval、reasoning、optimization、similarity search。

#### 8. Simulation Job System 升级

- dependency DAG（dc_op → [ac, tran, montecarlo]）
- artifact system（统一管理 psf、logs、waveforms、measurements，类似 ML experiment tracking）
- cache（netlist 没变、corners 没变则 reuse）

#### 9. TUI → EDA Terminal IDE

```
┌─────────────┬──────────────────┬──────────────┐
│  Sessions   │   Schematic      │ AI Copilot   │
│  Hierarchy  │   Preview/Graph  │   Logs       │
│             │                  │ Waveforms    │
└─────────────┴──────────────────┴──────────────┘
```

类似 lazygit / k9s / aerc 这种现代 terminal app。

---

## 四、隐藏风险：Cadence 版本兼容

README 提到 `IC23.1+ unified ADE`，这是风险点。建议建立 compatibility abstraction layer：

```
VirtuosoAdapter trait
  ├── IC617Adapter
  ├── IC618Adapter
  └── IC231Adapter
```

否则 ICADVM20、IC6.1.8、IC23.1 会越来越难维护，technical debt 会爆炸。

---

## 五、最终评价

这个项目最厉害的地方不是"做了个 CLI"，而是：**它已经隐约在把 Virtuoso 变成 "Agent Operating System"**。

很多人还停留在 pySKILL、socket bridge、shell automation，但这个项目已经有了：
- multi-session
- remote orchestration
- structured interfaces
- async simulation
- AI-native CLI

已经非常接近未来 AI IC Design 的形态。真正需要加强的是：

> **从字符串自动化 → 类型化 EDA 平台**

这是决定它能不能从"好用工具"进化成"AI 时代 EDA 基础设施"的关键。

---

## 六、建议贡献方式（按难度/价值排序）

如果你想帮助推进这个项目，推荐从以下开始：

1. **PR - MCP Server（最推荐，门槛最低）**：作为独立 crate 贡献，不需要动核心代码，且生态价值极高。详细设计见上方 Section 3。
2. **PR - 安全加固**：先做 `--readonly` 和 `evalstring` 白名单，是一个独立的安全模块
3. **PR - typed RPC**：选一个子命令（如 `schematic`）做完整示范，其他命令可照此扩展
4. **Issue**：在 repo 提一个 Enhancement Proposal，附上本文档链接
5. **长期合作**：这个方向需要 IC 设计 + Rust + AI agent 三个领域结合，欢迎深入协作