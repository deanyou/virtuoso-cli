#!/usr/bin/env bash
# 用法: gui_type.sh "<文本>" [--delay 毫秒]
# 聚焦锁定窗口后安全打字。默认每键 30ms 延时，避免应用漏字。
# 注意: type 走当前键盘布局；特殊字符/组合键请用 xdotool key（如 ctrl+c）。
set -u
export DISPLAY="${DISPLAY:-:0}"
TEXT="${1:-}"
[ -n "$TEXT" ] || { echo "ERROR: 缺少要输入的文本"; exit 2; }
DELAY="${DELAY:-30}"
STATE_FILE="${STATE_FILE:-/tmp/gui_session.env}"
LOG="${GUI_LOG:-/tmp/gui_actions.log}"

[ -f "$STATE_FILE" ] && . "$STATE_FILE"
if [ -n "${GUI_WID:-}" ]; then
  xdotool windowactivate "$GUI_WID" 2>/dev/null || xdotool windowfocus "$GUI_WID" 2>/dev/null || true
  sleep 0.3
fi

xdotool type --clearmodifiers --delay "$DELAY" -- "$TEXT"
# 日志只记录长度，不记录文本内容（避免泄露密码等敏感输入）
echo "$(date '+%F %T') type len=${#TEXT} delay=$DELAY wid=${GUI_WID:-none}" >> "$LOG"
echo "TYPED ${#TEXT} chars (window=${GUI_WID:-none})"
