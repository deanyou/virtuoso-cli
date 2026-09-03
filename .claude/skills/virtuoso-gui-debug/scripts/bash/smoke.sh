#!/usr/bin/env bash
# 冒烟测试：验证技能在目标环境可用。
# 流程：环境检查 → 启动 xmessage → 找窗锁定 → 截图 → 点击按钮 → 校验窗口关闭。
# 通过输出 SMOKE PASS，失败输出 SMOKE FAIL 并退出非零。
set -u
export DISPLAY="${DISPLAY:-:0}"
HERE="$(cd "$(dirname "$0")" && pwd)"
TAG="SMOKE-TEST-$$"

echo "== 1/6 env =="
bash "$HERE/gui_env.sh" || exit 1

echo "== 2/6 launch app =="
command -v xmessage >/dev/null || { echo "SKIP: 无 xmessage，无法做 GUI 冒烟测试"; exit 0; }
xmessage -title "$TAG" "smoke test, click OK" & APP=$!
sleep 1

echo "== 3/6 find window =="
bash "$HERE/gui_find.sh" "$TAG" || { kill "$APP" 2>/dev/null; exit 1; }

echo "== 4/6 screenshot =="
bash "$HERE/gui_shot.sh" /tmp/gui_smoke_before.png || true

echo "== 5/6 click (几何换算) =="
# shellcheck source=/dev/null
. /tmp/gui_session.env
eval "$(xdotool getwindowgeometry --shell "$GUI_WID")"
# xmessage 的 OK 按钮位于窗口左上角区域，取相对 (34,20) 换算为绝对坐标
CX=$((X + 34)); CY=$((Y + 20))
VERIFY_SHOT=/tmp/gui_smoke_after.png bash "$HERE/gui_click.sh" "$CX" "$CY"

echo "== 6/6 verify =="
sleep 1
if xdotool search --onlyvisible --name "$TAG" 2>/dev/null | head -1 | grep -q .; then
  echo "SMOKE FAIL: 点击后窗口仍存在"; kill "$APP" 2>/dev/null; exit 1
fi
echo "SMOKE PASS: 点击后窗口关闭，闭环可用"
