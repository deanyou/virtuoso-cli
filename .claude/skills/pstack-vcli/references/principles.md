# 从 pstack 移植的决策原则

这是上游原则在 vcli 中的应用索引，按需要使用。出处与许可证见 [upstream.md](upstream.md)。

| 上游原则 | vcli 中的决定 |
|---|---|
| laziness-protocol / subtract-before-you-add | 先去重复逻辑；选择能解决已证实问题的最小 diff。 |
| foundational-thinking / model-the-domain | 明确 transport 状态、SKILL 结果和 verifier 状态的类型，避免散落布尔条件。 |
| redesign-from-first-principles | 新需求暴露核心模型缺陷时，修正模型并迁移调用者。 |
| attack-the-premise | 多个同前提修复失败后，核对实际主机、会话和边界假设。 |
| minimize-reader-load | 保持 command 层薄，构造 SKILL 的代码集中到 client ops。 |
| outcome-oriented-execution | 迁移收敛到最终结构，不保留临时方案。 |
| experience-first | JSON、退出码和错误提示先服务 CLI 用户与自动化调用者。 |
| exhaust-the-design-space | 重大接口分歧用有限候选比较；小修改不强制多套原型。 |
| build-the-lever | 重复验证用可重跑脚本；先复用项目现有 harness。 |
| boundary-discipline / type-system-discipline | 在 CLI、文件、配置、TCP 与外部执行入口验证，内部信任类型。 |
| make-operations-idempotent | 会话清理和重试先明确副作用；不假设任意 Virtuoso 写入可重放。 |
| migrate-callers-then-delete-legacy-apis | 内部 API 迁移与删除一起完成；保持明确要求的外部兼容性。 |
| separate-before-serializing-shared-state | 优先独立 profile、缓存、库和单元；共享 DISPLAY 仍需现有独占锁。 |
| prove-it-works | 观察真实命令输出、退出码与目标状态；编译或代理总结不足以证明行为。 |
| fix-root-causes | 用最小复现确定失败层，不用 nil/default/success fallback 消除症状。 |
| sequence-verifiable-units | 每个工作单元结束于检查；用户未要求的 commit/PR 不是验证前提。 |
| test-behavior-not-implementation | 在现有测试入口断言调用者看到的行为，而非内部函数调用顺序。 |
| guard-the-context-window | 大范围分析分给代理，主线程保留结论及证据位置。 |
| never-block-on-the-human | 已授权的可逆工作继续，必要的权限和缺失输入仍按当前环境处理。 |
| encode-lessons-in-structure | 稳定规则放进类型、测试、lint 或脚本，避免不断加文字约束。 |
