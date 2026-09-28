<div align="center">

# vcli

### Control Cadence Virtuoso from anywhere.

A Rust CLI bridge for Cadence Virtuoso — designed for AI Agents and humans alike.
Local or remote, single-session or multi-instance, SKILL execution or GUI automation.

[**Get started ↓**](#get-started) · [GUI Debug Skill](.claude/skills/virtuoso-gui-debug/SKILL.md) · [Architecture](docs/architecture_v2_1.html) · [Releases](https://github.com/deanyou/virtuoso-cli/releases)

</div>

---

<div align="center">

**SEMANTIC FIRST. EVIDENCE FIRST. FROZEN BELOW.**

Drive Virtuoso via SKILL, then X11 — never OCR, never guess.

</div>

<table>
<tr>
<td width="25%" valign="top">
<h3>SKILL Bridge</h3>
<strong>Execute SKILL anywhere.</strong>
<p>TCP daemon inside Virtuoso, JSON CLI outside. Multi-session, multi-profile, remote tunnel.</p>
</td>
<td width="25%" valign="top">
<h3>GUI Automation</h3>
<strong>Deterministic X11 control.</strong>
<p>Click, type, key, drag, screenshot with server-side window identity re-validation.</p>
</td>
<td width="25%" valign="top">
<h3>Simulation</h3>
<strong>Spectre & Maestro.</strong>
<p>Async job registry, PSF parsing, ADE Explorer management (IC23.1+).</p>
</td>
<td width="25%" valign="top">
<h3>GUI Debug Skill</h3>
<strong>Replayable scenarios.</strong>
<p>Strict JSON DSL, fake/live/local executors, Router, Verifier, Evidence, Recovery.</p>
</td>
</tr>
</table>

<table>
<tr>
<td width="50%" valign="top">
<h3>🔌 Remote Tunnel</h3>
<strong>One command to any compute host.</strong>
<p>SSH tunnel with cross-arch daemon deploy, attach/detach non-destructive, native Rust SSH backend.</p>
</td>
<td width="50%" valign="top">
<h3>🤖 Agent-Native CLI</h3>
<strong>JSON output, schema introspection.</strong>
<p>Noun-verb commands, semantic exit codes, capability gating, per-client scratch isolation.</p>
</td>
</tr>
</table>

<div align="center">

**The essentials, included.**

Multi-session · Dynamic ports · Session history · Skill Finder<br>
Schematic editing · Maestro ADE · Spectre simulation<br>
TUI dashboard · Native SSH · GUI automation · Experience Platform

</div>

## Get Started

```bash
# Install
cargo install virtuoso-cli

# Load in Virtuoso CIW
load("/path/to/ramic_bridge.il")

# From terminal
vcli skill exec 'getCurrentTime()'
```

<details>
<summary><strong>Quick Start · Full</strong></summary>

**Load RAMIC Bridge in Virtuoso CIW:**

```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
```

**Connect from terminal:**

```bash
vcli session list                    # list active sessions
vcli skill exec 'getCurrentTime()'   # auto-connects if single session
```

**Remote mode:**

```bash
export VB_REMOTE_HOST=my-server
vcli tunnel start
vcli skill exec 'getCurrentTime()'
vcli tunnel stop
```

</details>

<details>
<summary><strong>Installation & Dependencies</strong></summary>

**From crates.io (recommended):**

```bash
cargo install virtuoso-cli                          # vcli (main CLI)
cargo install virtuoso-cli --bin vtui               # vtui (TUI dashboard)
cargo install virtuoso-cli --features daemon         # virtuoso-daemon (bridge backend)
```

**From source:**

```bash
git clone https://github.com/deanyou/virtuoso-cli.git
cd virtuoso-cli
cargo install --path .
```

> **Note**: Do not name the binary `virtuoso` — it conflicts with Cadence's executable.

**X11 GUI automation dependencies (on Virtuoso host):**

| Tool | Required for | Install |
|------|-------------|---------|
| xdotool (3.20140419.1+) | All GUI input | `apt install xdotool` |
| ImageMagick | Screenshots | `apt install imagemagick` |
| xwininfo (optional) | Geometry precheck | `apt install x11-utils` |

</details>

<details>
<summary><strong>SSH Backend & Configuration</strong></summary>

Two SSH transports: `openssh` (default, system ssh) or `native` (pure-Rust russh).

```bash
export VB_REMOTE_HOST=compute-host
export VB_JUMP_HOST=bastion          # optional
export VB_SSH_KEY=~/.ssh/id_ed25519_vcli
export VB_SSH_BACKEND=native          # optional, pure-Rust
```

See [Configuration](#configuration) for all env vars and capability matrix.

</details>

<details>
<summary><strong>System Architecture: EvoOntology ↔ GUI RSI</strong></summary>

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                              EVOONTOLOGY LAYER (v3)                                 │
│  ┌──────────────────────────────────────────────────────────────────────────────┐   │
│  │  Semantics Graph                                                           │   │
│  │  ├── Terms (22): skill_result, simulation_result, gm_over_id, gui_scenario... │   │
│  │  ├── Mappings (18): PSF, Maestro, xdotool, scenario DSL                    │   │
│  │  ├── Relations (13): derivation, hierarchy, composition                       │   │
│  │  └── Constraints (13): trigger keywords for discovery                      │   │
│  └──────────────────────────────────────────────────────────────────────────────┘   │
│                                    │                                               │
│                          browse_semantics / resolve_semantics                        │
│                                    ▼                                               │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                            AGENT / PLANNER                                         │
│                        Natural language intent                                      │
│                                    │                                               │
│                                    ▼                                               │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                    GUI RSI (Reflexive Semantic Interface)                           │
│                                                                                   │
│  ┌────────────────────────────────────────────────────────────────────────────┐     │
│  │                         SKILL Intelligence                                │     │
│  │  Skill Finder (FTS5) → SKILL Exec → Maestro Ops → Schematic Ops        │     │
│  └────────────────────────────────────────────────────────────────────────────┘     │
│                                    │                                               │
│                                    ▼                                               │
│  ┌────────────────────────────────────────────────────────────────────────────┐     │
│  │                            Action Router                                   │     │
│  │                                                                             │     │
│  │   SKILL ──→ Shortcut ──→ X11 (xdotool) ──→ Vision (last fallback)     │     │
│  │     │           │              │                │                          │     │
│  │     └───────────┴──────────────┴────────────────┘                          │     │
│  │                              │                                             │     │
│  │                         [fallback chain]                                   │     │
│  └────────────────────────────────────────────────────────────────────────────┘     │
│                                    │                                               │
│                                    ▼                                               │
│  ┌──────────────┐  ┌──────────┐  ┌─────────────┐  ┌──────────────┐            │
│  │   Executor   │  │  Verifier │  │  Recovery   │  │  Evidence    │            │
│  │─────────────│  │───────────│  │────────────│  │─────────────│            │
│  │ fake        │  │ passed    │  │ risk-class │  │ screenshot   │            │
│  │ live (vcli) │  │ failed    │  │ aware      │  │ metadata     │            │
│  │ local (xdot)│  │ skipped   │  │ retry      │  │ timestamp    │            │
│  └──────────────┘  └──────────┘  └─────────────┘  └──────────────┘            │
│                                                                                   │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                              VCLI RUNTIME                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐        │
│  │   Bridge     │  │   Tunnel     │  │   Spectre    │  │    Ocean     │        │
│  │─────────────│  │─────────────│  │─────────────│  │─────────────│        │
│  │ TCP/STX-NAK│  │ SSH Control  │  │ PSF Parser  │  │ Expr Eval   │        │
│  │ Multi-Sess  │  │ Cross-arch  │  │ Job Registry│  │ SKILL List  │        │
│  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘        │
│                                    │                                               │
│                                    ▼                                               │
├─────────────────────────────────────────────────────────────────────────────────────┤
│                           VIRTuoso + RAMIC Bridge                                  │
│                                                                                   │
│   ┌──────────────┐     ┌──────────────────────────────────────────────┐          │
│   │   CIW CLI    │     │            virtuoso-daemon                      │          │
│   │─────────────│     │──────────────────────────────────────────────│          │
│   │ SKILL REPL  │ ←→  │ TCP Server │ IPC │ SKILL Runtime │ RSICmd │          │
│   └──────────────┘     └──────────────────────────────────────────────┘          │
│                                                                                   │
│                              TCP Port (dynamic)                                   │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

### Layer Responsibilities

| Layer | Component | Function |
|-------|-----------|----------|
| **EvoOntology** | Semantics Graph | Discover constraints, map concepts to data sources |
| **Agent** | Planner | Natural language → semantic queries |
| **GUI RSI** | Action Router | SKILL → Shortcut → X11 → Vision fallback chain |
| **GUI RSI** | Executor | fake (offline) / live (vcli) / local (xdotool) |
| **GUI RSI** | Verifier | Evidence-grounded: passed/failed/skipped/unavailable |
| **VCLI** | Bridge | STX/NAK protocol, multi-session, SKILL exec |
| **Virtuoso** | Daemon | IPC bridge, RSICmd handler |

### Data Flow Example

```
User: "Run AC analysis and check if gain is correct"
         │
         ▼
EvoOntology: discover constraint_ac_mag_requires_mag_param
         │
         ▼
Agent: "Check vsource has mag=1 for AC"
         │
         ▼
GUI RSI: Router selects SKILL path
         │
         ▼
VCLI: virtuoso sim run --analysis ac
         │
         ▼
Virtuoso: Spectre executes, PSF output
         │
         ▼
Verifier: Compare gain vs expected, record evidence
         │
         ▼
Result: Evidence + VerifierResult
```

[📊 Architecture v2.1](docs/architecture_v2_1.html) · [📈 Evolution Playbook](docs/evolution_playbook.md) · [🤖 AGENTS.md](AGENTS.md#evoontology)

</details>

### EvoOntology Roadmap

```
┌───────────────────────────────────────────────────────────────────────────┐
│                        virtuoso-cli 知识域覆盖                           │
├───────────────────────────────────────────────────────────────────────────┤
│                                                                       │
│  ┌─────────────────────────┐    ┌─────────────────────────┐        │
│  │  EvoOntology ✅ 已覆盖   │    │  RAG 待集成 ⚙️         │        │
│  ├─────────────────────────┤    ├─────────────────────────┤        │
│  │ SKILL/Spectre 仿真      │    │ Design Guide 开放问答   │        │
│  │ • skill_nil_means_fail │    │ • ESD 设计注意事项     │        │
│  │ • psf_requires_success │    │ • 长尾工艺文档         │        │
│  │ • ac_mag_requires_mag  │    │ • 旧版本文档差异       │        │
│  │                          │    │                         │        │
│  │ GUI Debug RSI           │    │                         │        │
│  │ • executor selection    │    │                         │        │
│  │ • verifier semantics   │    │                         │        │
│  └─────────────────────────┘    └─────────────────────────┘        │
│                                                                       │
│  ────────────────────────────────────────────────────────────────   │
│                                                                       │
│  待扩展 (Gap)：                                                     │
│  ┌─────────────────┬────────────────────────────────────┐        │
│  │ PDK 版本差异    │ 需 PDK-specific mapping             │        │
│  │                  │ TT/FF/SS corner 规则              │        │
│  ├─────────────────┼────────────────────────────────────┤        │
│  │ 电路拓扑结构    │ Schematic hierarchy modeling          │        │
│  │                  │ Instance/net relationship          │        │
│  ├─────────────────┼────────────────────────────────────┤        │
│  │ 工具链兼容性    │ IC23 vs IC25 API 差异               │        │
│  │                  │ Maestro/ADE 版本差异               │        │
│  └─────────────────┴────────────────────────────────────┘        │
│                                                                       │
└───────────────────────────────────────────────────────────────────────┘
```

**构建原则**（见 [AGENTS.md](AGENTS.md#evoontology)）：
- 本体管「确定性事实」：约束、映射、规则断言
- RAG 管「相似候选」：开放文本、长尾文档
- 触发纪律：检索 miss → 先改 aliases/metadata；真实 miss 证据出现才上 vector

---

## English

Control Cadence Virtuoso from anywhere — locally or remotely. Designed for AI Agents and humans alike.

> **Based on** [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite) by Arcadia-1.
> `vcli` is a full Rust rewrite and major extension.

### Key Features

- **Multi-session** — Multiple Virtuoso instances, unique session IDs, no port conflicts
- **Multi-session broadcast** — `vcli skill broadcast` fans out to all live sessions concurrently
- **Dynamic port assignment** — Daemon binds port 0 (OS assigns), eliminating collisions
- **Session auto-discovery** — Single session auto-connects; multiple sessions require `--session`
- **Version unification** — `vcli session show` reports `daemon_version` + `version_skew` warning
- **Stale-daemon recovery** — `ipcIsAliveProcess` check on load, silent reaping
- **Session history** — Per-session SKILL + CLI history, survives reconnection
- **Local + remote modes** — Direct or SSH tunnel with ControlMaster multiplexing
- **Cross-arch tunnel deploy** — Auto-detects remote CPU arch, uploads matching daemon binary
- **Non-destructive tunnel attach** — Connect to existing daemon without deploying new one
- **Per-client scratch scoping** — Concurrent operators get isolated `/tmp/virtuoso_bridge/<client>/`
- **Skill Finder** — Fuzzy/prefix/suffix/exact/regex search over Cadence `.fnd` files
- **Agent-native CLI** — Noun-verb structure, JSON output, schema introspection, semantic exit codes
- **Schematic editing** — Create, place, wire, read instances/nets/pins/parameters
- **Maestro ADE management** — Open/close sessions, set variables, run simulations, export results
- **Spectre simulation** — Sync/async, job registry, PSF parser
- **X11 GUI automation** — `vcli window action-x11` with server-side window identity re-validation
- **Interactive TUI** — `vtui` terminal dashboard
- **Native SSH backend** — Optional pure-Rust `russh` transport with connection pool

### Command Reference

```
vcli [--profile P] [--session S] [--format json|table]
├── session                           Manage bridge sessions
│   ├── list / show / current / cleanup / history
├── tunnel                            SSH tunnel (start/stop/attach/detach/status)
├── skill                             SKILL execution
│   ├── exec / load / broadcast / find / info
├── cell                              Cellview management
│   ├── open / save / close / info
├── schematic                         Schematic editing
│   ├── open / place / wire / list-instances / get-params
├── maestro                           Maestro ADE Explorer
│   ├── open / set-var / run / export
├── sim                               Spectre simulation
│   ├── run-async / job-list / job-status / job-cancel
└── schema [noun] [verb]             Output command schema
```

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `VB_SESSION` | — | Target session ID |
| `VB_REMOTE_HOST` | — | SSH remote hostname (compute host) |
| `VB_JUMP_HOST` | — | Bastion/jump host |
| `VB_SSH_KEY` | — | SSH identity file |
| `VB_SSH_BACKEND` | `openssh` | `openssh` or `native` |
| `VB_TIMEOUT` | `30` | Connection/execution timeout (seconds) |
| `VCLI_CAPABILITY` | `user` | Set `admin` to unlock broadcast/raw SKILL |
| `VB_CLIENT_ID` | hostname | Per-client scratch isolation |

---

## 中文

从任何地方控制 Cadence Virtuoso，本地或远程均可。为 AI Agent 和人类共同设计。

[English](#english) · [核心特性](#key-features) · [快速开始](#get-started)

---

## License

MIT License — see [LICENSE](LICENSE)