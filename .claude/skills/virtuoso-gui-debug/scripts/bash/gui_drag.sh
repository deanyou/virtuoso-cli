#!/usr/bin/env bash
# 用法: gui_drag.sh <起点X> <起点Y> <终点X> <终点Y> [--button 1|2|3] [--steps N]
# 拖拽：按下 → 分步移动 → 释放。常用于拖动窗口、滑块、画布。
# 默认左键、8 步，步数越多轨迹越平滑（拖拽敏感应用可调大）。
set -u
export DISPLAY="${DISPLAY:-:0}"

X1="${1:-}"; Y1="${2:-}"; X2="${3:-}"; Y2="${4:-}"
[ -n "$X1" ] && [ -n "$Y1" ] && [ -n "$X2" ] && [ -n "$Y2" ] || { echo "ERROR: 需要 起点X 起点Y 终点X 终点Y"; exit 2; }
shift 4
BTN=1; STEPS=8
while [ "$#" -gt 0 ]; do
  case "$1" in
    --button) BTN="${2:-1}"; shift 2 ;;
    --steps) STEPS="${2:-8}"; shift 2 ;;
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

xdotool mousemove "$X1" "$Y1"
sleep 0.15
xdotool mousedown "$BTN"
for i in $(seq 1 "$STEPS"); do
  x=$(( X1 + (X2 - X1) * i / STEPS ))
  y=$(( Y1 + (Y2 - Y1) * i / STEPS ))
  xdotool mousemove "$x" "$y"
  sleep 0.02
done
sleep 0.1
xdotool mouseup "$BTN"
sleep 0.3

echo "$(date '+%F %T') drag ($X1,$Y1)->($X2,$Y2) button=$BTN steps=$STEPS wid=${GUI_WID:-none}" >> "$LOG"
echo "DRAG ($X1,$Y1)->($X2,$Y2) done"
