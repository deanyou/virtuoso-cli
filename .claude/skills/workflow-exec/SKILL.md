---
name: workflow-exec
description: Declarative workflow execution with SHA-256 manifest. Reads requirement YAML, generates execution brief with content hash, runs vcli commands (maestro/skill/spectre), and verifies results against manifest. Use when "run workflow", "execute brief", "manifest-driven validation", "requirement intake".
argument-hint: '[workflow file path or inline requirement YAML]'
allowed-tools: Bash(vcli *), Read, Write
---

# Workflow Exec — Declarative Execution with Manifest

读取声明式需求，生成带 SHA-256 哈希的 execution brief，执行 vcli 命令，验证结果。

## 核心概念

| 概念 | 说明 |
|------|------|
| **Requirement** | 声明式 YAML，描述目标规格和命令序列 |
| **Execution Brief** | JSON，包含哈希、命令序列、执行状态 |
| **Manifest** | SHA-256 校验文件，确保执行未被篡改 |
| **Artifact** | 执行产物：session ID、结果路径、日志 |

## 快速使用

```bash
# 方式1: 从 YAML 文件执行
vcli workflow run requirement.yaml

# 方式2: 从内联 YAML 执行
vcli workflow run --stdin << 'EOF'
testbench: FT0001A_SH/CMOP_TB/schematic
specs:
  - name: gain_db
    min: 80
  - name: gbw_hz
    target: 10e6
commands:
  - action: maestro.open_session
  - action: maestro.run
EOF

# 方式3: 验证已有 brief
vcli workflow verify brief-20260928-001.json
```

## Requirement YAML 格式

```yaml
# requirement.yaml
id: optimize_opamp           # 可选，自动生成
testbench:
  lib: FT0001A_SH
  cell: CMOP_TB
  view: schematic

specs:                       # 规格列表
  - name: gain_db
    min: 80
    unit: dB
  - name: gbw_hz
    min: 1e6
    target: 10e6
    unit: Hz
  - name: pm_deg
    min: 60
    unit: deg
  - name: power_uw
    max: 200
    unit: uW

variables:                   # Design variables
  L: 500e-9
  VDD: 3.3

commands:                    # 命令序列
  - action: maestro.open_session
    args:
      lib: "{{testbench.lib}}"
      cell: "{{testbench.cell}}"
      view: "{{testbench.view}}"
  
  - action: maestro.set_var
    args:
      name: L
      value: 500e-9
  
  - action: maestro.run
    timeout: 300
  
  - action: maestro.save
    args:
      name: opt_setup

outputs:                     # 期望产物
  - type: maestro_session
  - type: results_dir
  - type: psf_files
    pattern: "*.psf"

verify:                      # 验证规则
  - check: results_exist
    path: "{{outputs.results_dir}}"
  - check: spec_pass
    specs: "{{specs}}"
```

## Execution Brief 格式

```json
{
  "id": "brief-20260928-001",
  "sha256": "a3f5d8c7e9b2...",
  "created": "2026-09-28T18:30:00Z",
  "status": "success",
  "requirement": {
    "id": "optimize_opamp",
    "testbench": "FT0001A_SH/CMOP_TB/schematic",
    "specs": [...]
  },
  "commands": [
    {
      "action": "maestro.open_session",
      "args": {...},
      "status": "success",
      "result": "fnxSession0",
      "duration_ms": 1234
    },
    ...
  ],
  "artifacts": {
    "maestro_session": "fnxSession0",
    "results_dir": "/path/to/results",
    "psf_files": ["tran.tran", "ac.ac"]
  },
  "verification": {
    "passed": true,
    "checks": [
      {"name": "results_exist", "passed": true},
      {"name": "spec_pass", "passed": true, "details": {...}}
    ]
  }
}
```

## 执行流程

```
requirement.yaml
      │
      ▼
┌─────────────────┐
│  Parse & Validate│
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Generate Brief   │  ← SHA-256(content)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Execute Commands │  ← vcli maestro/skill/spectre
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Collect Artifacts│
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Verify Manifest  │  ← Check SHA-256, spec pass
└────────┬────────┘
         │
         ▼
     brief.json
```

## 与 circuit-optimizer 的区别

| 特性 | workflow-exec | circuit-optimizer |
|------|-------------|-----------------|
| **用途** | 通用工作流 | 贝叶斯优化 |
| **输入** | 声明式 YAML | 规格 + 参数范围 |
| **执行** | 命令序列 | 迭代优化循环 |
| **验证** | SHA-256 manifest | spec pass/fail |

## 命令行接口

```bash
# 生成 brief（不执行）
vcli workflow brief requirement.yaml

# 执行并生成 brief
vcli workflow run requirement.yaml

# 验证已有 brief
vcli workflow verify brief.json

# 查看 brief 状态
vcli workflow status brief-20260928-001
```

## 内部脚本

- `scripts/brief.py` — 解析 YAML，生成带哈希的 brief
- `scripts/run.py` — 执行 vcli 命令序列
- `scripts/verify.py` — SHA-256 校验和规格验证

## EvoOntology 约束覆盖

- `psf_requires_success` — 仿真结果目录必须存在
- `skill_nil_means_failure` — SKILL 返回 nil 表示失败
- `resultsdir_binding` — ADE session 的 resultsDir 绑定规则
- `remote_tunnel_prerequisite` — 远程连接前检查 tunnel
