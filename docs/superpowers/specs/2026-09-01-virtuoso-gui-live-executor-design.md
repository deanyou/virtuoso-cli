# Virtuoso GUI 真实执行器设计

## 目标

通过 `vcli`、SSH 和 X11，将 `.claude/skills/virtuoso-gui-debug/` 接入真实 Virtuoso 会话，同时把所有 GUI 输入限制在明确的 Rust 安全边界内。保留现有 fake executor，用于确定性的离线测试。

首个真实目标为：SSH 主机 `ubuntu-docker`、会话 `dean-user1-34929`、daemon 端口 `34929`。真实验收不得修改 schematic 数据。

## 当前探测结果

- `ssh ubuntu-docker '... vcli session list --format json'` 返回唯一会话：`dean-user1-34929`，端口 `34929`。
- 该会话缓存的 PID 为 `0`。
- 通过会话直接调用 `getpid()` 失败，因为远端 bridge 尚未加载新版辅助函数。
- `window list-windows-x11` 已进入远端 X11 路径，但创建 `/tmp/virtuoso_bridge/virtuoso_bridge` 时权限失败，说明存在跨用户 scratch 目录所有权冲突。

## 架构

### Rust 安全边界

扩展现有 `vcli window` X11 实现，为 DSL 提供语义固定的受限操作：

- 列出窗口，并唯一解析目标窗口；
- 激活已解析的窗口；
- 发送白名单内的组合键；
- 输入文字，但日志不得记录文字内容；
- 使用窗口相对坐标执行点击和拖动；
- 截取目标窗口截图；
- 等待窗口满足指定条件。

每个会产生输入的操作都必须接收 resolver 返回的精确窗口 ID。resolver 必须同时校验场景中的 DISPLAY 和 PID；PID 缺失或为零、匹配结果为零个或多个时都必须拒绝执行。发送输入前必须再次验证目标窗口。窗口标题只能作为辅助证据，不能单独作为身份依据。

外部命令必须使用 `Command::new()` 并逐项传参，用户输入不得进入 shell 命令字符串。所有操作必须设置明确超时。X11 scratch 路径必须包含远端有效用户和 client/profile 身份，并设置严格权限，避免多个用户在 `/tmp` 下发生冲突。

### Python LiveExecutor

在 `FakeExecutor` 旁新增 `LiveExecutor`。它只允许通过注入的 command runner 调用参数固定的 `vcli` 命令；不得直接调用 `ssh`、`xdotool`、`xprop` 或 shell。

CLI 新增 `--executor live`、`--vcli PATH`、`--ssh-host HOST`，并要求显式指定 session。使用 `--ssh-host` 时，由 transport adapter 通过 SSH 执行固定的 `vcli` argv；远端 argv 必须安全编码，环境必须经过清理。该 adapter 不接受任意远端命令。

`precheck` 必须依次验证：

1. 指定的远端 session 存在，且端口与选择的 session 一致；
2. vcli 连接成功；
3. session PID 为正数；旧 bridge 元数据为零时，可使用固定 bridge 查询或安全的远端进程/端口发现方式；
4. DISPLAY 与场景中的值完全一致；
5. 所需 X11 工具存在；
6. 目标窗口按 PID 绑定后恰好匹配一个；
7. 能以非阻塞方式获得该 DISPLAY 的独占锁。

`baseline` 记录已脱敏的 session/窗口元数据和截图。`execute` 将每个 DSL 操作映射到固定的 vcli 动作。`verify` 优先使用 vcli 数据库谓词，仅在验证显示状态时使用 X11 谓词。`recover` 只能执行场景中已经通过验证的 rollback 操作，并在恢复前重新验证目标身份。

### 远端仓库与技能协作

远端 `/home/user1/git/virtuoso-cli` 是本仓库的运行副本；`/home/user1/git/skill/.claude/skills/virtuoso-skill-dev/` 提供 SKILL 静态验证、vcli admin 调试和 SkillBridge 回退能力。真实调试优先使用远端已经运行的 `vcli --session dean-user1-34929`，需要编写或修复 `.il` 时遵循 `virtuoso-skill-dev` 的“静态验证 → vcli load → 函数测试”闭环。

当前远端 `virtuoso-cli` 仓库的 `resources/ramic_bridge.il` 存在未提交修改，因此不得直接执行 `git pull`。开发期间把待测脚本复制到用户私有的隔离 staging 目录，或使用独立远端 worktree；不得覆盖远端工作树中的现有修改。只有本地实现通过 Codex 验收，并且用户另行授权 commit/push 及远端修改处理方案后，才进入 Git 同步部署。

`virtuoso-skill-dev` 只能扩展 SKILL 编写和诊断能力，不能绕开本设计的 GUI 身份绑定、DISPLAY 锁、操作白名单与日志脱敏要求。SkillBridge 仅作为 vcli 不可用时的诊断回退，不作为 GUI 输入通道。

### 状态与证据

现有 Runner 继续负责有限重试和 JSONL trace。真实执行细节必须脱敏：可以记录命令名称、窗口 ID、几何信息、退出状态和耗时；不得记录输入文字、环境变量值、凭据、license 路径或原始进程环境。

DISPLAY 锁存放在用户级 runtime 目录中，并在整个运行期间持续持有。所有输出仍必须限制在调用方指定的新目录内，截图也存放在该目录下。

## 失败策略

以下情况必须关闭式失败，不得降级继续执行：

- PID 缺失或为零；
- session 或 DISPLAY 不匹配；
- 窗口匹配不唯一，或窗口在执行前消失；
- DISPLAY 锁被占用；
- 缺少 X11 依赖；
- 子进程超时或返回无法解析的 JSON；
- operation 或 verifier 不受支持；
- 截图或动作后验证失败。

禁止退化为“选择标题匹配的第一个窗口”、root-window 坐标、默认 `DISPLAY=:0` 或未绑定目标的 xdotool 操作。

## 测试方案

所有测试先于实现编写，并通过注入 command runner 完成：

- argv 构造和无 shell 执行；
- 远端 session 与 PID 发现，包括旧 bridge 回退路径；
- 严格的 DISPLAY/PID/唯一窗口绑定；
- 多窗口歧义和 PID 为零时的拒绝行为；
- X11 动作映射及相对坐标边界；
- 超时与畸形输出处理；
- DISPLAY 锁互斥；
- 输入文字和环境信息的日志脱敏；
- live CLI 参数验证；
- 现有全部 fake executor 测试继续通过。

Rust 单元和集成测试在不连接真实 Virtuoso 的情况下覆盖命令构造与安全约束。Python 测试使用 fixture 和 fake。最终真实 smoke test 分阶段执行：

1. 查询远端 session；
2. 解析真实 PID；
3. 唯一枚举 X11 目标窗口；
4. 截取目标窗口截图；
5. 执行一次可恢复且不产生键鼠输入的窗口激活；
6. 验证动作后的状态。

在只读 smoke test 成功之前，不允许发送按键、输入文字、点击、拖动、关闭对话框、加载 SKILL 文件或修改数据库。后续如需执行这些动作，必须另行获得授权。

## 范围

本阶段包括：Rust X11 安全边界、Python live executor、CLI 接线、测试、skill 文档、远端只读 smoke test，以及解决用户级 X11 scratch 目录冲突所需的修复。

本阶段不包括：任意 shell/SKILL 执行、视觉识别或 OCR、自主修改 schematic、无限恢复、自动安装依赖、自动处理远端未提交修改、提交或推送代码，以及 Wayland 兼容。
