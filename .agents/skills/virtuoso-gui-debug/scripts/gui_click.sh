#!/usr/bin/env bash
# 用法: gui_click.sh <绝对X> <绝对Y> [--button 1|2|3] [--count 1|2] [--verify-shot 输出]
# 动作 + 校验闭环：激活锁定窗口 → 移动 → 点击 → 自动重新截图 + 写动作日志。
# 按钮: 1=左键 2=中键 3=右键；count=2 表示双击。
set -u
export DISPLAY="${DISPLAY:-:0}"

CX="${1:-}"; CY="${2:-}"
[ -n "$CX" ] && [ -n "$CY" ] || { echo "ERROR: 需要绝对屏幕坐标 X Y"; echo "用法: $0 <X> <Y> [--button 1|2|3] [--count 1|2]"; exit 2; }
shift 2
BTN=1; CNT=1; AFTER="${VERIFY_SHOT:-/tmp/gui_after.png}"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --button) BTN="${2:-1}"; shift 2 ;;
    --count) CNT="${2:-1}"; shift 2 ;;
    --verify-shot) AFTER="${2:-/tmp/gui_after.png}"; shift 2 ;;
    *) shift ;;
  esac
done
STATE_FILE="${STATE_FILE:-/tmp/gui_session.env}"
LOG="${GUI_LOG:-/tmp/gui_actions.log}"

[ -f "$STATE_FILE" ] && . "$STATE_FILE"
if [ -n "${GUI_WID:-}" ]; then
  xdotool windowactivate "$GUI_WID" 2>/dev/null || xdotool windowraise "$GUI_WID" 2>/dev/null || true
  sleep 0.2
fi

xdotool mousemove "$CX" "$CY"
sleep 0.15
xdotool click --repeat "$CNT" --delay 100 "$BTN"
sleep 0.5

if command -v import >/dev/null; then
  import -window root "$AFTER" 2>/dev/null && echo "VERIFY_SHOT $AFTER"
fi
echo "$(date '+%F %T') click x=$CX y=$CY button=$BTN count=$CNT wid=${GUI_WID:-none} shot=$AFTER" >> "$LOG"
echo "CLICK $CX,$CY button=$BTN count=$CNT done (window=${GUI_WID:-none})"
