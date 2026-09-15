#!/usr/bin/env bash
# 用法: gui_wait.sh <name|class关键字> <超时秒> [appear|disappear]
# 轮询等待窗口出现(appear，默认)或消失(disappear)。条件达成返回 0，超时返回 1。
set -u
export DISPLAY="${DISPLAY:-:0}"
PATTERN="${1:-}"; TIMEOUT="${2:-10}"; MODE="${3:-appear}"
[ -n "$PATTERN" ] || { echo "ERROR: 需要窗口匹配关键字"; exit 2; }

deadline=$(( $(date +%s) + TIMEOUT ))
while [ "$(date +%s)" -lt "$deadline" ]; do
  n=$(xdotool search --onlyvisible --name "$PATTERN" 2>/dev/null | wc -l)
  if [ "$MODE" = "disappear" ] && [ "$n" -eq 0 ]; then echo "OK disappeared ($PATTERN)"; exit 0; fi
  if [ "$MODE" = "appear" ] && [ "$n" -gt 0 ]; then echo "OK appeared ($PATTERN)"; exit 0; fi
  sleep 0.5
done
echo "TIMEOUT after ${TIMEOUT}s waiting for $MODE: $PATTERN"; exit 1
