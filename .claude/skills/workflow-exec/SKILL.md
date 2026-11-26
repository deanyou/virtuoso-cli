---
name: workflow-exec
description: Declarative workflow execution with SHA-256 manifest. Reads requirement YAML, generates execution brief with content hash, runs vcli commands (maestro/skill/spectre), and verifies results against manifest. Use when "run workflow", "execute brief", "manifest-driven validation", "requirement intake".
argument-hint: '[workflow file path or inline requirement YAML]'
allowed-tools: Bash(python3 *), Read, Write
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
# 方式1: 生成 brief（不执行）
python3 scripts/brief.py generate requirement.yaml

# 方式2: 执行 brief
python3 scripts/run.py execute brief.json

# 方式3: 验证已有 brief
python3 scripts/verify.py brief.json
```

## Requirement YAML 格式

```yaml
# requirement.yaml
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

variables:                   # Design variables
  L: 500e-9
  VDD: 3.3

commands:                    # 命令序列
  - action: maestro.open_session
    args:
      lib: FT0001A_SH
      cell: CMOP_TB
      view: schematic
  
  - action: maestro.run
    timeout: 300

outputs:                     # 期望产物
  - type: maestro_session
  - type: results_dir
```

## Execution Brief 格式

```json
{
  "id": "brief-20260929-001",
  "sha256": "a3f5d8c7e9b2...",
  "created": "2026-09-29T18:30:00Z",
  "status": "success",
  "requirement": {
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
    }
  ],
  "artifacts": {
    "maestro_session": "fnxSession0"
  }
}
```

## 执行流程

```
requirement.yaml
      │
      ▼
┌─────────────────┐
│ brief.py generate  │  ← SHA-256(content)
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ run.py execute    │  ← vcli maestro/skill
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ verify.py        │  ← Check SHA-256, specs
└────────┬────────┘
         │
         ▼
     brief.json
```

## EvoOntology 约束覆盖

- `psf_requires_success` — 仿真结果目录必须存在
- `skill_nil_means_failure` — SKILL 返回 nil 表示失败
- `remote_tunnel_prerequisite` — 远程连接前检查 tunnel

## 内部脚本

| 脚本 | 功能 |
|------|------|
| `scripts/brief.py` | 解析 YAML，生成带哈希的 brief |
| `scripts/run.py` | 执行 vcli 命令序列 |
| `scripts/verify.py` | SHA-256 校验和规格验证 |

## 局限性

- **模板语法**: 不支持 `{{variable}}` 替换，参数需直接写在 YAML 中
- **Spec 验证**: 是 stub 实现，需要结合 maestro-read-results skill
- **Shell 执行**: 命令通过 shell=True 执行，需注意注入风险
