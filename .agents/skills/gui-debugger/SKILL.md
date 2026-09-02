---
name: gui-debugger
description: "通过 xdotool 在 X11 上调试/操作原生 GUI 应用。当智能体需要点击、输入、按键、截图、查询窗口状态来排查或操作图形界面程序（对话框、设置面板、桌面应用、CI 里的 GUI 冒烟测试）时使用。仅支持 X11；Wayland 环境请先处理显示后端。"
---
# GUI Debugger（xdotool 闭环调试）

## Overview

本技能用「观察 → 决策 → 动作 → 校验」闭环操作 X11 原生 GUI。`xdotool` 负责注入
鼠标键盘事件（动作通道），截图工具负责把界面状态喂给视觉模型（观察通道）。核心纪律：
**每个动作必须跟一次校验**，校验失败就依据新截图校准坐标重试，最多 N 次，禁止盲点连击。

- **首选实现**：`scripts/gui.py`（Python 单入口 CLI，推荐）。
- **备用实现**：`scripts/bash/`（轻量 bash 原语，无需 Python 的环境可用）。
- 按需阅读 `references/xdotool-cheatsheet.md`（命令表 + 坐标换算 + 陷阱）。

## 前置检查（必须，任一失败即中止并说明原因）

```bash
python3 scripts/gui.py env
```

- 要求：X11（`XDG_SESSION_TYPE` ≠ wayland）、`DISPLAY` 已设置、`xdotool` 存在。
- Wayland 下报错退出，不静默降级；调试环境可改用 `Xvfb :99 + openbox`。
- 截图需要 `import`(ImageMagick) 或 `scrot`，缺失时观察通道受限，应明确提示。

## 核心工作流

### 1. 首次观察（先看，不要盲点）

```bash
python3 scripts/gui.py shot /tmp/gui.png      # 截图并读取
python3 scripts/gui.py state                   # 可见窗口列表 + 焦点窗口
python3 scripts/gui.py find <name|class>       # 锁定目标窗口（写入 /tmp/gui_session.env）
```

- 锁定窗口 ID 后，后续 click/type 只针对该窗口，防止误操作其他窗口。
- 若找不到目标窗口，先 `wait <关键字> 10 appear` 等待它出现。

### 2. 闭环动作单元（定位 → 动作 → 校验）

1. **定位**：从截图识别控件，用几何换算得到绝对坐标：
   ```bash
   # 读取锁定窗口几何（Python 中可用：from gui import window_geometry；或直接）
   python3 -c "import sys; sys.path.insert(0,'scripts'); import gui; print(gui.window_geometry(gui.load_wid()))"
   # 控件在窗口内的相对坐标 (rx,ry) → 绝对坐标 (X+rx, Y+ry)
   ```
2. **动作**：
   ```bash
   python3 scripts/gui.py click $CX $CY                        # 左键单击 + 自动校验截图
   python3 scripts/gui.py click $CX $CY --button 3            # 右键
   python3 scripts/gui.py click $CX $CY --count 2             # 双击
   python3 scripts/gui.py type "要输入的文本"                   # 聚焦后打字（默认 30ms/键）
   python3 scripts/gui.py drag $X1 $Y1 $X2 $Y2                 # 拖拽（拖窗口/滑块/画布）
   python3 scripts/gui.py scroll $CX $CY down 3                # 滚轮（up/down/left/right）
   python3 scripts/gui.py key ctrl+c                           # 组合键/功能键
   ```
3. **校验**：动作后必重新截图比对（`click` 已自动存校验截图 `/tmp/gui_after.png`），
   或查询窗口状态确认期望变化（弹窗出现、文字变化、窗口关闭）。
4. **失败处理**：回到定位步骤，依据新截图校准坐标重试，最多 N=3 次；仍失败记录并停止，
   不盲目连点。

### 3. 等待条件（异步界面）

```bash
python3 scripts/gui.py wait <关键字> 10 appear      # 等窗口出现
python3 scripts/gui.py wait <关键字> 10 disappear   # 等窗口关闭
```

### 4. 冒烟验证（交付前必须跑）

```bash
python3 scripts/gui.py smoke
```

- 验证本环境闭环可用：启动 xmessage → 找窗 → 截图 → 点击 → 校验窗口关闭。
- 输出 `SMOKE PASS` 才算通过；`SMOKE FAIL` 说明环境或坐标换算有问题。

### 5. 动作日志（回放定位）

- 每次动作自动追加到 `$GUI_LOG`（默认 `/tmp/gui_actions.log`），
  记录：时间、动作类型、坐标/参数、目标窗口 ID、校验截图路径。
- `type` 只记录输入长度、不记录内容，避免泄露密码等敏感输入。
- 排查"某步界面状态为何异常"时，回读该日志即可重建完整操作序列。

## 安全护栏

- 未锁定目标窗口（无 `GUI_WID`）前禁止发送任何输入。
- 破坏性操作（关闭窗口、alt+f4、删除类按钮）前先截图确认目标。
- 动作间保留必要延时（激活后 0.2~0.3s），避免事件丢失。

## 已知限制

- 仅 X11；Wayland 需要 ydotool 或 Xvfb 兜底。
- `type` 受当前键盘布局影响；特殊字符/组合键用 `key <keysym>`。
- HiDPI/多屏场景坐标可能偏移，用 `xrandr` 与几何换算校正。
