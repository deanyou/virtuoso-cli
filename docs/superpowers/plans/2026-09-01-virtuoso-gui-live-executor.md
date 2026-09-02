# Virtuoso GUI 真实执行器实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**目标：** 将 `virtuoso-gui-debug` 的严格 DSL 接入真实 `vcli`/SSH/X11，并以 PID、DISPLAY、唯一窗口和独占锁约束所有 GUI 动作。

**架构：** Rust `vcli window` 是唯一 GUI 输入安全边界，负责远端身份解析、窗口重验证和固定语义的 xdotool 调用。Python `LiveExecutor` 只执行允许的 `vcli` argv，通过可注入 command runner 测试；Runner 继续负责重试、恢复和证据日志。真实验收先只读，再执行 activation-only smoke。

**技术栈：** Rust、clap、serde、Python 3.9 标准库、unittest、SSH、X11、xdotool。

**规格：** `docs/superpowers/specs/2026-09-01-virtuoso-gui-live-executor-design.md`

## 全局约束

- 不允许 shell 拼接用户输入；Rust 外部命令使用结构化参数，Python 只允许固定 argv。
- PID 缺失或为零、DISPLAY 不匹配、窗口匹配不唯一、锁冲突时关闭式失败。
- 不允许默认 `DISPLAY=:0`、首个标题匹配、root-window 坐标或未绑定窗口的 xdotool。
- TYPE 日志只能记录字符数，不能记录文本；不得记录环境、凭据和 license 路径。
- fake executor 及其现有测试必须保持兼容。
- 不自动安装依赖，不 commit、push、pull，不处理或覆盖本地及远端已有修改。
- CCB 必须先写失败测试并运行确认，再写生产实现。

---

### 任务 1：远端 X11 scratch 隔离和窗口身份解析

**文件：**
- 修改：`src/transport/x11.rs`
- 修改：`src/commands/window.rs`
- 修改：`resources/x11_dismiss_dialog.py`

**接口：**
- 产生：`resolve_unique_window(windows, expected_pid, expected_display, optional_title) -> Result<WindowInfo>`。
- 产生：用户与 profile 隔离的 scratch 路径，禁止复用其他用户拥有的目录。

- [x] **步骤 1：编写失败的 Rust/Python 测试**

覆盖 PID 为零、PID 不同、DISPLAY 不同、零匹配、多匹配、唯一匹配；覆盖 scratch 路径包含安全化 user/client_id，并拒绝不安全路径成分。

- [x] **步骤 2：运行测试并确认 RED**

运行：

```bash
cargo test transport::x11 -- --nocapture
python3 -m unittest discover -s tests/docs -p 'test_x11_dismiss_dialog.py' -v
```

预期：新 resolver 和 user-scoped scratch 测试因功能缺失失败。

- [x] **步骤 3：实现最小身份解析与 scratch 修复**

resolver 仅接受正 PID；比较 `WindowInfo.pid` 和 `WindowInfo.display`；可选标题仅用于缩小集合；结果数量不是 1 时返回 `VirtuosoError::Conflict` 或 `NotFound`。scratch 目录形如 `/tmp/virtuoso_bridge/<safe-user>/<safe-client>/x11`，创建时使用 `umask 077`/等价固定命令，不包含场景自由文本。

- [x] **步骤 4：运行测试并确认 GREEN**

重复步骤 2 的两个命令，预期全部通过。

---

### 任务 2：Rust 固定语义 X11 动作命令

**文件：**
- 修改：`src/main.rs`
- 修改：`src/commands/window.rs`
- 修改：`src/transport/x11.rs`
- 修改：`resources/x11_dismiss_dialog.py`

**接口：**
- 产生 CLI：`vcli window action-x11 --operation <activate|key|type|click-rel|drag-rel|screenshot|wait> --window-id ID --pid PID --display DISPLAY ... --format json`。
- 每次动作前调用任务 1 resolver 重验证精确窗口 ID、PID 和 DISPLAY。

- [x] **步骤 1：编写失败测试**

覆盖每个 operation 的 clap 参数、非法 key chord、TYPE 日志脱敏、相对坐标越界、负尺寸、错误 PID/DISPLAY、窗口消失、超时、截图输出路径越界，以及结构化 xdotool argv。

- [x] **步骤 2：运行测试并确认 RED**

```bash
cargo test commands::window transport::x11 -- --nocapture
```

预期：`action-x11` 尚不存在，新测试失败。

- [x] **步骤 3：实现最小动作集合**

只允许设计规格列出的 operation。key chord 解析为有限 token；TYPE 传值但返回详情只含 `text_length`；click/drag 将相对坐标限制在窗口宽高内；screenshot 只允许调用方输出目录下的路径；wait 使用条件轮询而非固定 sleep。所有动作返回 JSON：`status`、`operation`、`window_id`、`pid`、`display`、`duration_ms` 和脱敏 details。

- [x] **步骤 4：运行 GREEN 与 Rust 门禁**

```bash
cargo test commands::window transport::x11 -- --nocapture
cargo clippy -- -D warnings
cargo fmt --check
```

预期：全部通过且无警告。

---

### 任务 3：Python 固定 argv transport 与 LiveExecutor

**文件：**
- 创建：`.claude/skills/virtuoso-gui-debug/scripts/vgui_runner/command_runner.py`
- 创建：`.claude/skills/virtuoso-gui-debug/scripts/vgui_runner/live_executor.py`
- 修改：`.claude/skills/virtuoso-gui-debug/scripts/gui_runner.py`
- 修改：`.claude/skills/virtuoso-gui-debug/scripts/vgui_runner/engine.py`
- 创建：`.claude/skills/virtuoso-gui-debug/tests/test_command_runner.py`
- 创建：`.claude/skills/virtuoso-gui-debug/tests/test_live_executor.py`
- 修改：`.claude/skills/virtuoso-gui-debug/tests/test_cli.py`

**接口：**
- 产生：`CommandRunner.run(argv: Sequence[str], timeout_seconds: int) -> CommandResult`。
- 产生：`LiveExecutor(command_runner, vcli_path, ssh_host, session_id, output_dir)`，实现现有 `Executor` 接口。

- [x] **步骤 1：编写失败的 command runner 测试**

断言本地执行使用 `shell=False` 和清理后的环境；SSH 模式只接受合法 host 与固定 vcli argv；拒绝换行、NUL、额外远端命令；超时和非 JSON 输出产生结构化错误。

- [x] **步骤 2：运行 RED**

```bash
python3 -m unittest .claude/skills/virtuoso-gui-debug/tests/test_command_runner.py -v
```

预期：模块不存在，测试失败。

- [x] **步骤 3：实现 command runner**

使用 `subprocess.run(list(argv), shell=False, timeout=...)`。SSH transport 构造固定的 `ssh -- HOST <encoded-fixed-vcli-argv>`；使用标准库安全编码每个 argv，不接受任意命令字符串。环境只保留 PATH、LANG/LC_ALL，并显式加入 `VCLI_CAPABILITY=admin`。

- [x] **步骤 4：编写 LiveExecutor 失败测试**

覆盖 session 缺失/端口不符、PID 为零、PID 回退失败、DISPLAY 不符、窗口零/多/唯一匹配、锁冲突、baseline 截图、每个 DSL operation 映射、database-first verifier、rollback、输入脱敏。

- [x] **步骤 5：运行 RED**

```bash
python3 -m unittest .claude/skills/virtuoso-gui-debug/tests/test_live_executor.py -v
```

预期：`LiveExecutor` 不存在，测试失败。

- [x] **步骤 6：实现 LiveExecutor 与 CLI 接线**

`--executor live` 必须同时要求 `--session`、`--ssh-host`、`--output`；场景 session 必须与 CLI session 相同。precheck 固定调用 session list、PID 查询、window list；只把解析后的窗口 ID 保存为运行状态。execute 只调用任务 2 CLI；verify 优先固定 vcli 查询；recover 只执行已验证 rollback。

- [x] **步骤 7：运行 Python GREEN**

```bash
python3 -m unittest discover -s .claude/skills/virtuoso-gui-debug/tests -v
PYTHONPYCACHEPREFIX=/private/tmp/vgui-live-pycache python3 -m py_compile .claude/skills/virtuoso-gui-debug/scripts/gui_runner.py .claude/skills/virtuoso-gui-debug/scripts/vgui_runner/*.py
```

预期：全部测试通过，fake executor 行为不变。

---

### 任务 4：Skill 文档和本地总验收

**文件：**
- 修改：`.claude/skills/virtuoso-gui-debug/SKILL.md`
- 修改：`.claude/skills/virtuoso-gui-debug/references/scenario-schema.md`

- [x] **步骤 1：更新 live 使用契约**

写明强制 session/PID/DISPLAY/cellView、唯一输出目录、先 validate/dry-run、只读 smoke 顺序、失败关闭规则，以及远端仓库 dirty 时禁止 pull。

- [x] **步骤 2：运行完整门禁**

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt --check
python3 -m unittest discover -s .claude/skills/virtuoso-gui-debug/tests -v
python3 /Users/dean/.codex/skills/.system/skill-creator/scripts/quick_validate.py .claude/skills/virtuoso-gui-debug
```

预期：所有门禁通过。若仓库已有无关失败，必须记录精确测试名并证明本任务聚焦测试通过，不得修改无关代码掩盖失败。

---

### 任务 5：远端只读 smoke 与 activation-only 验收

**文件：**
- 不修改远端仓库；测试产物写入用户私有临时目录。

- [ ] **步骤 1：检查远端 dirty 状态**

```bash
ssh ubuntu-docker 'git -C /home/user1/git/virtuoso-cli status --short --branch'
```

预期：记录现状；不执行 pull/reset/checkout。

- [ ] **步骤 2：部署隔离测试产物**

把已验收二进制和 skill 脚本复制到新的 user-scoped staging 目录，不覆盖 `/home/user1/git/virtuoso-cli`。

- [ ] **步骤 3：只读 precheck**

针对 `dean-user1-34929` 依次验证 session、PID、DISPLAY、X11 依赖、唯一窗口和截图。任何一项失败都停止，不发送 GUI 输入。

- [ ] **步骤 4：activation-only smoke**

仅在步骤 3 成功后激活同一窗口，再重新验证窗口 ID、PID、DISPLAY 和可见状态。不得发送 key/type/click/drag/dismiss 或 SKILL load。

- [ ] **步骤 5：保存证据**

保存 `task.json`、`agent-actions.jsonl`、`summary.json` 和截图，确认日志不含输入文本、环境变量或凭据。
