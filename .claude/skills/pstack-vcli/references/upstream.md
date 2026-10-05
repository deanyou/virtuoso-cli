# 来源与移植范围

来源是 [backnotprop/pstack](https://github.com/backnotprop/pstack)，本次分析固定版本：

`157aae39a733135e93d8b5b19ff62c6a84b0ad56`

上游是 Cursor pstack 的 standalone mirror。其核心是 Agent Skills，不是 IC 设计工具或 Rust dependency。本项目安装的是独立 `pstack-vcli` 移植版，保留 [MIT 许可证](../LICENSE)，不是全量 pstack 插件安装。

派生内容对应上游：

- `skills/poteto-mode/SKILL.md` 与 investigation、bug-fix、feature、refactoring、perf-issue、authoring-a-skill、eval playbooks：本技能的路由和分步验证。
- `skills/how/`、`skills/architect/`、`skills/interrogate/`：源码追踪、接口候选和独立审查。
- `skills/tdd/SKILL.md`：有廉价稳定测试时先复现失败。
- `skills/principle-*/SKILL.md`：按任务选择的工程原则，见 [principles.md](principles.md)。
- `skills/create-verification-skill/SKILL.md`、`skills/maintain-verification-skill/SKILL.md`：真实用户路径、隔离、证据保留和维护；`scripts/verify.py` 是本次为 vcli 新写的 helper。

本地修改：合法名称与通用 frontmatter；使用当前 harness 的原生工具和继承模型；不依赖默认 Grok/Claude slug、Cursor Task 参数、`cursor-team-kit` 或 Bun；保留 vcli 的 Rust/Python/SKILL 分工、两层结果语义、主机模型与重试规则。

不安装上游 shipping/orchestrate/watch-pr/worktree-cleanup 脚本、Slack automations、bot UI、TypeScript 专项技能和全局模型配置。这些功能与当前 vcli 技能安装任务无直接关系。工作流不自动发外部消息、不自动开 PR 或合并、不用 reset 清理用户修改。

维护时先阅读上游变更并检查上述差异，再更新本地版本和测试。不要用重新下载整个插件覆盖本技能或其它项目技能。
