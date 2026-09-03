#!/usr/bin/env bash
# 环境检查：确认 X11、DISPLAY、xdotool、截图工具可用。
# 任一不满足即非零退出并输出明确原因，禁止静默继续。
set -u

[ -n "${DISPLAY:-}" ] || { echo "ERROR: DISPLAY 未设置（无 X 会话）"; exit 1; }
[ "${XDG_SESSION_TYPE:-}" = "wayland" ] && { echo "ERROR: 当前为 Wayland，xdotool 不可用；请改用 Xvfb 或 ydotool"; exit 1; }

command -v xdotool >/dev/null || { echo "ERROR: 缺少 xdotool，请安装 (apt install xdotool)"; exit 1; }

# 截图工具：import(ImageMagick) 优先，scrot 次之
if ! command -v import >/dev/null && ! command -v scrot >/dev/null; then
  echo "WARN: 缺少 import/scrot，截图观察通道不可用（可安装 imagemagick）"
fi

echo "OK display=$DISPLAY xdotool=$(xdotool --version 2>&1 | head -1)"
geom=$(xdotool getdisplaygeometry 2>/dev/null)
echo "geometry=$geom"
