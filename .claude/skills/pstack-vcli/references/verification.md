# 可重复的验证

从仓库根目录执行：

```bash
python3 .claude/skills/pstack-vcli/scripts/verify.py --scope all
python3 .claude/skills/pstack-vcli/scripts/verify.py --scope smoke
python3 .claude/skills/pstack-vcli/scripts/verify.py --scope rust --plan
```

可用 `--repo PATH` 明确 checkout，用 `--output-dir DIR` 保存证据，用 `--timeout SECONDS` 限制每个命令。查看脚本 `--help` 获取当前参数。计划的状态是 `planned`，不执行工具，也不代表通过。

| Scope | 证据 | 边界 |
|---|---|---|
| rust | `cargo test`、`cargo clippy -- -D warnings`、`cargo fmt --check` 的输出及退出码 | 默认 feature；不能声称 daemon/native-ssh 等所有 feature 已验证。 |
| skills | 临时快照里的 `.github/scripts/run-skill-tests.sh`，包含新技能的 unittest | 快照隔离 tracked SQLite；只代表当前 Python 版本。CI 负责 3.9/3.13 matrix。 |
| smoke | 构建本 checkout 的 vcli，检查 version、SKILL exec 的 schema 和空 session registry | 独立 VB runtime 路径与明确 profile，无 Virtuoso 连接。 |
| all | 以上检查顺序执行 | 任一失败返回非零，后续检查跳过，不把 skipped 算作通过。 |

输出目录包含 `report.json` 和每一步的 stdout/stderr。失败或超时仍保留证据。输出可能包括项目测试诊断，分享前遵守项目的凭据/PDK 数据约束。不同运行使用独立目录。

离线 smoke 不调用 `config check`，因为它还检查不可由 VB 路径变量隔离的 legacy `~/.vcli/.env`。不修改 HOME，不枚举用户已有 session，不使用已有安装的 vcli 代替当前源码构建。

实机能力需单独验证。先用已有 `tunnel-connect`/`diag` 确认指定会话、compute hostname 和可用能力。GUI 使用 `virtuoso-gui-debug` 的场景验证与身份绑定；fake 回放只检查自动化逻辑。共享后端由一个驱动者执行，使用任务明确的 scratch 库/单元。清理仅处理本次创建的资源。

没有 live Virtuoso、X11 或 PDK 时，在交付中列为未验证，不降低门槛或自动扩大连接范围。
