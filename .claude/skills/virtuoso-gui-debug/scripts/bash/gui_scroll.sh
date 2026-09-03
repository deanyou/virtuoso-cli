#!/usr/bin/env bash
# 用法: gui_scroll.sh <X> <Y> <up|down|left|right> [次数]
# 滚轮：up=4, down=5, left=6, right=7。默认滚动 3 格。
set -u
export DISPLAY="${DISPLAY:-:0}"

CX="${1:-}"; CY="${2:-}"; DIR="${3:-down}"; N="${4:-3}"
[ -n "$CX" ] && [ -n "$CY" ] || { echo "ERROR: 需要坐标 X Y"; exit 2; }
case "$DIR" in
  up) BTN=4 ;;
  down) BTN=5 ;;
  left) BTN=6 ;;
  right) BTN=7 ;;
  *) echo "ERROR: 方向须为 up|down|left|right"; exit 2 ;;
esac
LOG="${GUI_LOG:-/tmp/gui_actions.log}"

xdotool mousemove "$CX" "$CY"
sleep 0.1
xdotool click --repeat "$N" --delay 60 "$BTN"
sleep 0.3

echo "$(date '+%F %T') scroll $DIR x=$CX y=$CY n=$N wid=${GUI_WID:-none}" >> "$LOG"
echo "SCROLL $DIR x$N done"
