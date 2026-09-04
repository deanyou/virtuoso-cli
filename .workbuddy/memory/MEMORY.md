# virtuoso-cli — 项目长期记忆

## 主线状态（2026-09-02）
- step 0–8 主线 + 8a–8g2 + CI 全修复链均已在 `origin/main`，integration matrix 6/6 ✅。
- v1.2.0 已发布。**OpenSSH 恒为默认后端，无自动迁移。**
- **PR #29 已合并**（`b5cbe64`）；**PR #31 已合并**（`9e54dd0`）→ Security Audit 红灯根除：
  `rsa` RUSTSEC-2023-0071 经 `cargo-audit 0.22.2 --locked` + CLI `--ignore RUSTSEC-2023-0071` 处理；
  `audit.toml` 仅作文档（0.22.2 不读它）；`rustsec/audit-check@v2.0.0` 弃用。默认构建不链 rsa。
- ~~待清：`lru 0.12.5` 两条 unsound~~ → **2026-09-04 核实：已失效**。`lru` 既不在 `Cargo.toml` 也不在 `Cargo.lock` 的依赖树里（Security Audit 常绿），该 TODO 可删。

## native 范围边界（长期事实，勿误述为已交付）
v1.2.0 native 后端 = 单跳直连 + 公钥认证最小 stable 切片：
- step 5 SOCKS5 + jump **未实现**：`establish` 在 `cfg.jump_host.is_some()` 返回 `UnsupportedOperation`（连接期 fail-fast）。
- step 6 RAMIC/X11 转发 **未实现**：trait 默认 `UnsupportedOperation`。
- agent/password/keyboard-interactive 未实现；缺 key 时 `from_config` 返回 `TransportError::Configuration`（构造期，非 UnsupportedOperation）。
- 均属主动 scope 裁剪。

## 待办
- **issue #30 → PR #32 已合并 main @ `bb90460`**：x11.rs 截图路径 hardening 全套修复（request-changes review #1/#2/#3/#4/#6 + #5 测试）。main 后 CI 双 run 6/6 绿（run 33606273561 / 33606273587）。
  🔴#3 `LocalTransport::download_file` 改 `std::fs::copy(remote, local)`（commit 57f95f0）；
  🟠#1 `validate_png_artifact` `std::fs::metadata`→`symlink_metadata`；
  🟠#2 `:986/977` 本地 `remove_file(remote_path)` 删不到远程 → 改 `runner.run_command("rm -f …")`（success/validation-fail/fetch-error 三处）；
  🟡#4 token 用 `Uuid::new_v4()`；🟡#5 补 `validate_png_artifact`/`MouseButtonGuard::drop`/`LocalTransport::download_file` 单测；🟡#6 0×0 窗口跳过边界检查。
  残留下游（非本 PR）：`validate_png_artifact` 的 `File::open` 仍跟随 symlink（低危，本地 evidence 目录）→ **已由 PR #33 用 unix O_NOFOLLOW 关闭；issue #30 已于 2026-09-03 关闭**。
  ~~唯一残留：dispatcher.rs:764 注释~~ → **2026-09-04 核实：已修**。现注释（766-767 行）写的是
  "Ping uses the idempotent probe, NOT `execute_skill_unchecked`"，与 776 行
  `execute_skill_idempotent_probe("plus(1 1)")` 一致，准确。**#30 无残留。**

## Issue 盘点（2026-09-03，基线 origin/main `602e163` / v1.3.0）
- 已关闭：#30 #34 #35 #47 #48（对应 PR #32/#33/#39/#42/#51 + README 文档）。
- 仍开：#49（dismiss-window-x11 仍是 positional）、#50 与 #53（**互为重复**，SKILL 侧反射只出 `{"name"}`，`hiGetCurrentDialog` 在 IC25.1 仍未换）、
  → **#49/#50/#53/#54/#55 已于 2026-09-03 全部修完**（PR #70 `#71543c0`）；#50 的 `id` 在 IC25.1 回归
    另由 **PR #72 `#67ca635`** 修（`errset` 版本容错）。#50 保持 closed，kind/mode/geometry/pid 反射永久 deferred（IC25.1 无 API）。
  #54（枚举现为 activate|key|type|click-rel|drag-rel|scroll|screenshot|wait|close —— 新增 scroll/close，但 absolute click / minimize / maximize / double-click 未做）、
  #55（`--pid` 仍必填 `u32`，无 _NET_WM_PID 的窗口不可操作）。

## 环境事实（易踩）
- 🔴 **本地 git 常落后远程数天**：判 issue 是否已修 / 查代码现状前必先 `git fetch origin`，并用 `git show origin/main:<path>` 落临时文件再 Grep；**绝不能拿工作区代码判定**（曾因此差点误判 #47/#48 未修）。
- macOS / dean。**Bash 实际跑 fish**：heredoc/fish begin-end 失败；`VAR=x cmd` 前缀无效；多行写 .sh 用 `bash`；长 commit `git commit -F <file>`。gh 长评论走 `Write /tmp/x.md` + `gh issue comment N --body-file`。
- macOS **无 timeout**（用 Bash timeout 参数）；**无 /proc**；手动 sshd 被 Seatbelt 拦；`sysctl KERN_PROCARGS2` EPERM。
- ✅ `gh` CLI 可达 api.github.com（github.com HTML 被拦）→ `gh run list/view` 查 CI 关键手段；日志导出后用 Grep 检索 `error\[|##\[error\]|FAILED`。
- 🔴 Bash 的 `grep`/`tail` 不可靠 → 一律用 Grep 工具 / `run_in_background`。
- **网络**：crates.io API 可达；RustSec advisory-db git 被拦 → 本机 `cargo audit` 拉不到 DB（环境限制，非漏洞）；`curl https://crates.io` 裸 403 是 UA 拦截。

## 活体 Virtuoso（IC25.1）连接方法（2026-09-04 实测跑通）
1. `ssh ubuntu-docker`（virtuoso-skill-dev 技能提供）→ docker 上找 daemon 实际端口：
   `ss -ltnp | grep virtuoso` + 读 `$HOME/.cache/virtuoso_bridge/sessions/*.json`（**端口是动态的**，旧 session 会失效）。
2. 建隧道：`ssh -f -N -L <port>:localhost:<port> ubuntu-docker`。
3. **本机必须显式覆盖两个变量**否则连不上：
   - `VB_CACHE_DIR=/Users/dean/.cache`（本机 `XDG_CACHE_HOME` 被设成它 → `cache_root()` 不走 `~/Library/Caches`）
   - `VB_SESSION=<session-id>`（`/Users/dean/.env` 会经 `load_dotenv_upward` 注入另一套环境的 `VB_SESSION`/`VB_REMOTE_HOST`，必须覆盖）
4. 完整调用：
   `VCLI_CAPABILITY=admin VB_CACHE_DIR=/Users/dean/.cache VB_SESSION=dean-user1-<port> VB_TIMEOUT=120 ./target/debug/vcli <cmd>`
5. 探测函数存在性用 `fboundp`（查符号绑定，可靠）；**不要**用 `let`/`hiGetWindowList()` 执行路径判存在——daemon 上偶发 flaky 会误报 "not a function"。
6. 喂复杂 SKILL 一律 `vcli skill eval --stdin < file`（Write 落文件），**别用 shell 引号**（转义必炸）。注意 `skill exec` 只收位置参数 `<CODE>`，没有 `--stdin`。

## 🔴 SKILL 求值器陷阱（IC25.1 daemon 实测，写新 SKILL 必守）
1. **`sprintf` 误求值**：作为 `if`/`when` 分支或顶层表达式时，返回原始 `(window:1)` 对象而非字符串。在 `let`/`foreach`/`strcat` 内部正常 → **JSON 一律用 `strcat` 拼**（顺便内联把 fixnum 转字符串）。
2. **`if`/`when` 只接受自求值（常量）分支**：分支体是 `strcat`/`sprintf`/赋值等复合形式时静默误求值成 `(window:1)`。
3. `strcat` **拒绝 nil** → 拼接用的字符串变量必须默认 `""`。
4. 因此版本容错的正确形状是 **`errset` + 默认 `""`**，而不是 `if(fboundp(...)) …`（后者需要复合分支）。

## IC25.1 窗口 API 可用性（fboundp 实测表）
可用：`hiGetWindowList`、`hiGetWindowName`、`geGetEditCellView`
**不可用**：`hiGetWindowId`、`hiGetWindowType`、`hiGetWindowMode`、`hiGetWindowScreenBox`、`hiGetProcessId`、`hiGetWindowProcessId`、`dbGetWindowId`、`hiGetEditCellView`
→ 结论：kind/mode/geometry/pid 的 SKILL 反射在 IC25.1 **无法实现**，`annotate_modes`（按 window name 启发式）是唯一跨版本安全路径。任何依赖 `hiGetWindowId` 的 SKILL 都必须做版本容错。

## 约定
- 验收门：`cargo fmt --check` / `clippy --all-targets -- -D warnings` / `cargo test` / `git diff --check`。
- 🔴 **升版必须同步 `resources/ramic_bridge.il:1` 的 `; RB_VERSION:` 戳**：守护测试
  `ramic_bridge_version_stamp_matches_cargo_toml`（tests/daemon_user_guard.rs:614）会比对
  Cargo.toml 版本，漏改会让 Integration Matrix 6 job 全红（2026-09-04 实际发生，run 33817533275）。
  升版后本地先跑 `cargo test --test daemon_user_guard` 再 push。
- 🔴 **开 PR 前先确认 `origin/main` 的 CI 本身是绿的**：main 曾因 fmt（58d862f）+ clippy（acf3d72）两处违规把 `Check & Lint` 搞红，导致新 PR 一并变红。修法：另开一个纯 lint 修复 PR 先合（见 PR #71），再把功能分支 rebase 上去。
- 一次提交一个接缝、零行为变更；偏离设计文档显式标 delta。
- 平台专有逻辑按 `#[cfg(unix)]`/`#[cfg(not(unix))]` 拆独立函数（fail loud 优于 fail silent）。
- README 严格双语（## English / ## 中文 对称）；文档改动回代码核实。
- `VirtuosoResult` 两层：SKILL 检查用 `r.skill_ok()`（非 `ok()`）。
- `cargo test` 只接受一个位置过滤参数 → 整组用模块级过滤。

## crates.io 发布
- `~/.cargo/credentials.toml` 需 `[registries.crates-io]` + `[registry]` 两段；`CARGO_REGISTRY_TOKEN` 优先。
- 流程：升版→CHANGELOG→**同步 `resources/ramic_bridge.il` RB_VERSION 戳**→`cargo publish --allow-dirty`→commit→tag→push main+tag→`gh release create`。
- ⚠️ release tag 打在 feature 分支会漏 main 提交：发布前先 `git merge --ff-only origin/main`。
- ✅ v1.3.1 已于 2026-09-04 发布闭环（crates.io + tag + GitHub Release）。
- crates.io 上线核验：`curl -A <UA> https://crates.io/api/v1/crates/virtuoso-cli/<ver>`（裸 curl 403 是 UA 拦截）。
