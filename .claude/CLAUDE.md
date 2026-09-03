# Claude Code — Project Guide

## GUI Debugging Skill

涉及 Virtuoso 窗口操作、X11 自动化、GUI 回归测试时，**必须**使用 `.claude/skills/virtuoso-gui-debug/` 技能。

### 三种执行器

| 执行器 | 命令 | 适用场景 |
|--------|------|----------|
| `fake` | `--executor fake` | 离线确定性回放，回归测试，逻辑验证 |
| `live` | `--executor live --session ID --vcli PATH [--ssh-host HOST]` | 远程 Virtuoso 会话，vcli 驱动，服务端窗口校验 |
| `local` | `--executor local [--window-id WID]` | 本地 X11，xdotool 直连，支持 SCROLL 滚轮 |

### 工作流

1. **先 validate**：`python3 .claude/skills/virtuoso-gui-debug/scripts/gui_runner.py validate SCENARIO.json`
2. **再 run**：`python3 .../gui_runner.py run SCENARIO.json --output OUT_DIR --executor <fake|live|local>`
3. **快速检查**：`python3 .../scripts/xdotool_cli.py <env|state|find|shot|click|type|key|drag|scroll|wait|smoke>`

### DSL 操作

`VCLI_LOAD` / `VCLI_CALL`（schema 接受但 live/local 均 fail-closed）、`WINDOW_WAIT`、`WINDOW_ACTIVATE`、`KEY`、`TYPE`、`CLICK_REL`、`DRAG_REL`、`SCROLL`（仅 local）、`SCREENSHOT`、`VERIFY`、`RECOVER`。

### 参考

- 场景 DSL 规范：`.claude/skills/virtuoso-gui-debug/references/scenario-schema.md`
- xdotool 命令参考：`.claude/skills/virtuoso-gui-debug/references/xdotool-cheatsheet.md`
- 测试：`cd .claude/skills/virtuoso-gui-debug && python3 -m unittest discover tests`
