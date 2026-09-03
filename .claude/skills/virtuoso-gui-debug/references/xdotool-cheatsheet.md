# xdotool 速查

## 常用命令

| 目标 | 命令 |
| --- | --- |
| 找窗口 | `xdotool search --onlyvisible --name "关键字"` / `--class 关键字` |
| 窗口几何 | `xdotool getwindowgeometry --shell $WID`（输出 X/Y/WIDTH/HEIGHT/SCREEN） |
| 窗口标题 | `xdotool getwindowname $WID` |
| 激活/聚焦 | `xdotool windowactivate $WID`；失败用 `windowraise`；纯输入聚焦用 `windowfocus` |
| 当前焦点 | `xdotool getactivewindow` |
| 移动鼠标 | `xdotool mousemove $X $Y`（屏幕绝对坐标） |
| 相对移动 | `xdotool mousemove_relative $dx $dy` |
| 点击 | `xdotool click 1`（左键）/ `2`（中键）/ `3`（右键）；`--repeat 2 --delay 100` 双击 |
| 按住/释放 | `xdotool mousedown 1` / `xdotool mouseup 1` |
| 拖拽 | 按下→分步移动→释放，用 `python3 scripts/gui.py drag X1 Y1 X2 Y2` |
| 滚轮 | `xdotool click 4`(上)/`5`(下)/`6`(左)/`7`(右)，用 `python3 scripts/gui.py scroll X Y down 3` |
| 打字 | `xdotool type --clearmodifiers --delay 30 -- "文本"` |
| 组合键 | `xdotool key ctrl+c`、`alt+f4`、`Return`、`Tab` |
| 等待同步 | 部分命令支持 `--sync`（等命令完成再返回） |
| 屏幕尺寸 | `xdotool getdisplaygeometry` |

## Python 实现（推荐入口）

`scripts/gui.py` 单入口 CLI 封装以上全部原语，并统一状态文件、校验截图与动作日志：

| 功能 | 命令 |
| --- | --- |
| 环境检查 | `python3 scripts/gui.py env` |
| 窗口快照 | `python3 scripts/gui.py state` |
| 锁定窗口 | `python3 scripts/gui.py find <name或class>` |
| 截图 | `python3 scripts/gui.py shot [路径]` |
| 点击 | `python3 scripts/gui.py click X Y [--button 3] [--count 2]` |
| 拖拽 | `python3 scripts/gui.py drag X1 Y1 X2 Y2 [--steps 8]` |
| 滚轮 | `python3 scripts/gui.py scroll X Y down [格数]` |
| 打字 | `python3 scripts/gui.py type "文本" [--delay 30]` |
| 组合键 | `python3 scripts/gui.py key ctrl+c alt+f4` |
| 等待 | `python3 scripts/gui.py wait <关键字> 10 [appear\|disappear]` |
| 冒烟 | `python3 scripts/gui.py smoke` |

也可 `import gui` 复用 `window_geometry()` / `load_wid()` / `lock_wid()` 等函数。
bash 备用实现位于 `scripts/bash/`（`gui_*.sh` + `smoke.sh`）。

## 动作日志
- 所有原语脚本自动追加动作到 `$GUI_LOG`（默认 `/tmp/gui_actions.log`）：时间、动作、
  坐标/参数、目标窗口 ID、校验截图路径；`type` 只记长度不记内容（防敏感信息泄露）。
- 回读日志可重建完整操作序列，用于排查"哪一步导致界面异常"。

## 坐标换算（重要）

- xdotool 的 `mousemove/click` 使用**屏幕绝对坐标**（左上角为原点）。
- 拿到窗口几何后：控件绝对坐标 = `X + 窗口内相对x`，`Y + 窗口内相对y`。
- 截图与 X 坐标共用同一坐标系，视觉模型在截图上识别到的像素位置可直接换算。
- 注意窗口装饰条（标题栏）可能带来偏移，必要时用 `xwininfo -id $WID` 复核。

## 已知陷阱

| 陷阱 | 现象 | 对策 |
| --- | --- | --- |
| Wayland | 命令静默失败/无效 | 检测 `$XDG_SESSION_TYPE`；改用 Xvfb 或 ydotool |
| 激活时序 | 激活后立即输入丢失 | 激活后 `sleep 0.2~0.5` |
| 无 WM | `windowactivate` 无效 | 装 openbox/fluxbox，或用坐标点击 + `windowraise` |
| 盲点坐标 | 点击不生效（控件位置≠猜测位置） | 先截图 → 视觉/OCR 定位 → 换算 → 再点 |
| 键盘布局 | `type` 中文/特殊字符错误 | 用 `--clearmodifiers`；特殊字符改 `key <keysym>` |
| 多屏/HiDPI | 坐标偏移 | 固定 Xvfb 分辨率；用 `xrandr` 校正 |
| 窗口未就绪 | search 返回空 | `python3 scripts/gui.py wait <关键字> 10 appear` 轮询 |
| 多窗口匹配 | 命中错误窗口 | 加 `--onlyvisible`，用 name+class 双条件 |

## 调试小技巧

- 卡住时先 `xdotool getactivewindow && xdotool getwindowgeometry --shell $(xdotool getactivewindow)` 确认焦点和几何。
- 输入密码/中文等敏感场景优先用 `key` 指定 keysym，避免布局歧义。
- 后台运行脚本需 `export DISPLAY=:99`（或目标显示号）。
