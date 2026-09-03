#!/usr/bin/env bash
# 用法: gui_find.sh <name或class关键字>
# 按名称匹配可见窗口，把目标窗口 ID 写入会话状态文件（默认 /tmp/gui_session.env）。
# 后续 click/type 都只针对这个锁定的窗口，避免误操作其他窗口。
set -u
export DISPLAY="${DISPLAY:-:0}"

PATTERN="${1:-}"
[ -n "$PATTERN" ] || { echo "ERROR: 缺少匹配关键字"; echo "用法: $0 <name|class关键字>"; exit 2; }
STATE_FILE="${STATE_FILE:-/tmp/gui_session.env}"
LOG="${GUI_LOG:-/tmp/gui_actions.log}"

WID=$(xdotool search --onlyvisible --name "$PATTERN" 2>/dev/null | head -1)
[ -z "$WID" ] && WID=$(xdotool search --onlyvisible --class "$PATTERN" 2>/dev/null | head -1)
[ -n "$WID" ] || { echo "NOT_FOUND: 未找到匹配窗口 ($PATTERN)"; exit 1; }

echo "export GUI_WID=$WID" > "$STATE_FILE"
echo "$(date '+%F %T') lock wid=$WID pattern=$PATTERN" >> "$LOG"
echo "LOCKED WID=$WID name=$(xdotool getwindowname "$WID" 2>/dev/null) state=$STATE_FILE"
