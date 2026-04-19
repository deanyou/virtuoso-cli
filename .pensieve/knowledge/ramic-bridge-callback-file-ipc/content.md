# RAMIC Bridge: Callback File IPC 协议

## Source
Session 2026-04-19: 借鉴 Arcadia-1/virtuoso-bridge-lite，修复
IC23.1/RHEL8 上 `ipcWriteProcess` 数据处理器停止触发的平台 bug。

## Summary
`ipcWriteProcess` 在 IC23.1/RHEL8 首次调用后停止触发；改用临时文件对传递结果，
daemon 轮询 `.done` 标记取代 stdin 轮询。

## Content

### 平台 Bug

`ipcWriteProcess(ipcId, result)` 在 IC23.1/RHEL8 上只在首次调用有效，
后续调用的数据处理器回调不再触发（Cadence 平台 bug）。表现为第二次及以后的
`vcli skill exec` 调用挂起直至超时。

### 修复：Callback File 协议

**SKILL 侧（`resources/ramic_bridge.il`）**：

```skill
procedure(RBSendCallback(msg)
  let((cbPort dataFile doneFile port)
    cbPort = RBPort + 1          ; PORT+1 mirrors cb_port in daemon
    dataFile = sprintf(nil "/tmp/.ramic_cb_%d" cbPort)
    doneFile = sprintf(nil "/tmp/.ramic_cb_%d.done" cbPort)
    port = outfile(dataFile "w")
    when(port fprintf(port "%s" msg) close(port))
    port = outfile(doneFile "w")
    when(port close(port))
  )
)
```

在 `RBIpcDataHandler` 末尾调用 `RBSendCallback(resultStr)` 替代
`ipcWriteProcess(ipcId, resultStr)`。

**Rust daemon 侧（`src/daemon/main.rs`）**：

```rust
fn read_callback_file(cb_port: u16, timeout_secs: u64) -> io::Result<Vec<u8>> {
    let data_file = format!("/tmp/.ramic_cb_{cb_port}");
    let done_file = format!("/tmp/.ramic_cb_{cb_port}.done");
    // poll for done_file, read data_file, strip trailing RS (0x1E), delete both
}
```

`cb_port = port + 1`（daemon 启动参数 `argv[2]` 的端口加一）。

### 关键细节

| 项目 | 值 |
|------|---|
| 数据文件 | `/tmp/.ramic_cb_{PORT+1}` |
| 完成标记 | `/tmp/.ramic_cb_{PORT+1}.done` |
| 结果格式 | `STX + %L + RS` 或 `NAK + %L + RS` |
| RS 处理 | daemon 读取后去掉末尾 `0x1E` 再发给 vcli 客户端 |
| 幂等性 | 每次请求前清理残留文件，避免上次遗留的 `.done` 被误读 |

### Rust Daemon 位置

活跃 daemon 是 Rust binary，不是 Python 脚本：

| 文件 | 状态 |
|------|------|
| `src/daemon/main.rs` | **活跃**，`cargo build --features daemon --bin virtuoso-daemon` |
| `resources/daemons/ramic_bridge_daemon_3.py` | **遗留**，不再使用 |
| `resources/daemons/ramic_bridge_daemon_27.py` | **遗留**，不再使用 |

`ramic_bridge.il` 中 `RBDPath` 解析顺序：
1. `RB_DAEMON_PATH` 环境变量
2. `~/.cargo/bin/virtuoso-daemon`（cargo install 路径）

## When to Use
- 调试 `vcli skill exec` 第二次调用挂起时
- 理解为何不用 `ipcWriteProcess`
- 修改 daemon 通信协议时（需同步改 IL 和 Rust）
- 确认哪个 daemon 文件是活跃实现

## Context Links
- Related: [[vcli-bridge-cli-name]] — daemon binary 路径混淆
- Related: [[ocean-createnetlist-prerequisites]] — bridge 连接建立后的使用
