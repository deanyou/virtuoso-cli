#!/usr/bin/env bash
# 截图（观察通道入口）。用法: gui_shot.sh [输出路径]
# 输出路径默认为 /tmp/gui_shot.png，保存后打印实际路径。
set -e
export DISPLAY="${DISPLAY:-:0}"
OUT="${1:-/tmp/gui_shot.png}"
LOG="${GUI_LOG:-/tmp/gui_actions.log}"

if command -v import >/dev/null; then
  import -window root "$OUT"
elif command -v scrot >/dev/null; then
  scrot "$OUT"
else
  echo "ERROR: 无可用截图工具 (import/scrot)"; exit 1
fi
echo "$(date '+%F %T') shot $OUT" >> "$LOG"
echo "SAVED $OUT ($(du -h "$OUT" 2>/dev/null | cut -f1))"
