# P0-A · 环境问题证据记录（未定论）

> 日期：2026-09-05
> 状态：**记录证据，不作"已彻底定位/已解决"的结论**。不修改全局 shell 配置。

## 1. streaming 管道测试偶发失败（`transport::ssh`）

**现象**：`streaming_pipeline_drains_large_producer_stderr_without_deadlock`
在**隔离运行**时通过（默认构建约 0.74s），但在**全量 `cargo test --lib`
并行负载**下偶发 `Timeout(20)`。

**已抓取的确凿 panic**（`/tmp/libtest_full.log`）：

```
thread '...streaming_pipeline_drains_large_producer_stderr_without_deadlock' panicked at src/transport/ssh.rs:859:90:
called `Result::unwrap()` on an `Err` value: Timeout(20)
```

**同一运行中的负载证据**：测试框架报告
`fixture_allows_open_ssh_transport_roundtrip has been running for over 60 seconds`
（该测试最终通过，但耗时 >60s）——说明失败当次整套件负载极重。

**对照结果**：

| 运行条件 | 结果 |
|---|---|
| 隔离，默认构建 | 通过（~0.74s） |
| 隔离，native-ssh 构建 | 通过 |
| 全量，ulimit 256 | 失败 `Timeout(20)` |
| 全量，ulimit 4096，native-ssh | 通过（lib 31.78s） |
| 全量，ulimit 4096，默认 | 失败 `Timeout(20)`（另见 >60s 测试） |

**结论（暂定）**：20s 是死锁**检测**上限而非性能上限——真死锁无论预算多少
都不会完成，超时会照常失败。失败当次存在"其他测试 >60s"的负载证据，且
隔离/部分条件下通过，指向**机器负载 + fd 压力叠加的环境问题**，而非确定的
代码并发缺陷。**不能据此宣称无并发缺陷，也未排除机器处于异常慢状态。**

**新增负载证据（决定性）**：失败期间 `uptime` 报告
`load averages: 29.40 33.69 44.26`（1/5/15 分钟），`6 users` 登录的共享机器，
严重过载（远超本机核心数）。同时段简单 `grep`/`sed` 均出现 >15s 挂起。该证据
支持"环境负载"而非代码缺陷的判断。

**处置**：保留 5s→20s 作为测试容错调整（不构成修复）；是否再放宽待后续
在负载正常时机重跑判定。

## 2. 文件描述符上限

- 当前 shell `ulimit -n` = **256**；内核上限 `kern.maxfilesperproc` = **122880**。
- 提高至 4096 后 native-ssh 全量套件通过一次，但默认构建在 4096 下仍失败
  （见上表）——**fd 上限是重要线索，但不是已被证实的唯一根因**。
- **未修改 `~/.zshrc` 等全局配置**。测试/构建时以 `ulimit -n 4096` 前缀临时设置，
  保留上述对照结果。

## 3. `cargo fmt` 段错误（未解决）

- 本机 `cargo fmt`（rustfmt 1.9.0-stable）**稳定段错误（exit 139）**，属工具链
  问题，与文件内容无关；单文件 / 批量 rustfmt 均正常。
- 临时替代：`rustfmt --edition 2021 $(find src tests -name '*.rs')`（应用与
  `--check` 均可用）。
- **等价性说明**：替代命令的 `--edition 2021`、文件集合 `src tests -name '*.rs'`、
  默认 rustfmt 配置均与 `cargo fmt` 对齐，`--check` 通过。但 `cargo fmt` 崩溃
  仍单列为**未解决项**，CI 若走 `cargo fmt --check` 需在工具链正常的环境执行，
  不能以本机替代命令的通过宣称"全部标准门禁已通过"。

## 4. 后续建议（不自动执行）

- 在负载正常时段重跑全量套件，确认 streaming 测试通过率；
- 若频繁复现，考虑给该测试标注 `#[ignore]` 并加独立说明，或在测试内用更长
  检测上限（不影响真死锁检出）；
- fd 上限与 rustfmt 崩溃是否修复，由用户在 shell 配置 / 工具链层面决定。
