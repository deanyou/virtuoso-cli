#!/usr/bin/env bash
# 状态快照：列出可见窗口（ID/名称/几何）与当前焦点窗口。
# 供决策层在每次观察阶段调用，快速了解当前界面状态。
set -u
export DISPLAY="${DISPLAY:-:0}"

echo "== visible windows =="
for wid in $(xdotool search --onlyvisible --name ".*" 2>/dev/null | head -40); do
  name=$(xdotool getwindowname "$wid" 2>/dev/null | head -c 60)
  geo=$(xdotool getwindowgeometry "$wid" 2>/dev/null | awk '/Position|Geometry/ {gsub(/\(screen: [0-9]+\)/,""); printf "%s ", $0}')
  echo "$wid | $name | $geo"
done

echo "== active window =="
awid=$(xdotool getactivewindow 2>/dev/null)
echo "active=$awid $(xdotool getwindowname "$awid" 2>/dev/null)"
