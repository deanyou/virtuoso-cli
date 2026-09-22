#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""gui-debugger：基于 xdotool 的 X11 GUI 调试原语（Python 实现）。

核心闭环：观察 → 决策 → 动作 → 校验。
xdotool 负责注入输入（动作通道），截图工具负责观察（观察通道）。

用法（子命令）：
  python3 gui.py env
  python3 gui.py state
  python3 gui.py find <name或class>
  python3 gui.py shot [输出路径]
  python3 gui.py click <X> <Y> [--button 1|2|3] [--count 1|2] [--verify-shot 路径]
  python3 gui.py drag <X1> <Y1> <X2> <Y2> [--button] [--steps]
  python3 gui.py scroll <X> <Y> <up|down|left|right> [次数]
  python3 gui.py type <文本> [--delay 毫秒]
  python3 gui.py key <keysym...>            # 组合键，如 ctrl+c alt+f4 Return
  python3 gui.py wait <关键字> <超时秒> [appear|disappear]
  python3 gui.py smoke                       # 冒烟测试
"""
import argparse
import datetime
import os
import shutil
import subprocess
import sys
import time
from types import SimpleNamespace

STATE_FILE = os.environ.get("STATE_FILE", "/tmp/gui_session.env")
LOG_FILE = os.environ.get("GUI_LOG", "/tmp/gui_actions.log")


# ---------- 基础设施 ----------

def run(cmd, **kw):
    # Python 3.6 compatibility: capture_output and text were added in 3.7
    kw.setdefault("stdout", subprocess.PIPE)
    kw.setdefault("stderr", subprocess.PIPE)
    kw.setdefault("universal_newlines", True)
    return subprocess.run(cmd, **kw)


def log(msg):
    try:
        with open(LOG_FILE, "a", encoding="utf-8") as f:
            f.write(f"{datetime.datetime.now():%Y-%m-%d %H:%M:%S} {msg}\n")
    except OSError:
        pass


def die(msg, code=1):
    print(f"ERROR: {msg}", file=sys.stderr)
    sys.exit(code)


def require_env():
    """硬门槛检查：X11、DISPLAY、xdotool。任一失败即中止。"""
    if not os.environ.get("DISPLAY"):
        die("DISPLAY 未设置（无 X 会话）")
    if os.environ.get("XDG_SESSION_TYPE") == "wayland":
        die("当前为 Wayland，xdotool 不可用；请改用 Xvfb 或 ydotool")
    if shutil.which("xdotool") is None:
        die("缺少 xdotool，请安装 (apt install xdotool)")


def load_wid():
    """读取会话状态文件里锁定的目标窗口 ID。"""
    if os.path.exists(STATE_FILE):
        for line in open(STATE_FILE, encoding="utf-8"):
            if line.startswith("export GUI_WID="):
                return line.strip().split("=", 1)[1]
    return None


def lock_wid(wid):
    with open(STATE_FILE, "w", encoding="utf-8") as f:
        f.write(f"export GUI_WID={wid}\n")
    return wid


def activate(wid):
    """激活/聚焦窗口，失败则 raise；等待事件落地。"""
    if not wid:
        return
    if run(["xdotool", "windowactivate", wid]).returncode != 0:
        run(["xdotool", "windowraise", wid])
    time.sleep(0.2)


def window_geometry(wid):
    """返回窗口几何字典 {X,Y,WIDTH,HEIGHT,SCREEN}。"""
    r = run(["xdotool", "getwindowgeometry", "--shell", wid])
    geo = {}
    for line in r.stdout.splitlines():
        if "=" in line:
            k, v = line.strip().split("=", 1)
            geo[k] = v
    return geo


def screenshot(out, window_id=None):
    """观察通道：截图。默认整屏，指定 window_id 时截窗口。"""
    if shutil.which("import"):
        win = window_id if window_id else "root"
        cmd = ["import", "-window", win, out]
    elif shutil.which("scrot"):
        cmd = ["scrot", out]
    else:
        die("无可用截图工具 (import/scrot)")
    r = run(cmd)
    if r.returncode != 0:
        die(f"截图失败: {r.stderr.strip()}")
    print(f"SAVED {out}")
    log(f"shot {out} window={window_id or 'root'}")


# ---------- 子命令 ----------

def cmd_env(_a):
    require_env()
    v = run(["xdotool", "--version"]).stdout.strip()
    print(f"OK display={os.environ['DISPLAY']} {v}")
    print(f"geometry={run(['xdotool', 'getdisplaygeometry']).stdout.strip()}")


def get_window_name(wid):
    """Get window title; xdotool getwindowname may return empty for WM frames,
    fall back to xprop WM_NAME / _NET_WM_NAME."""
    name = run(["xdotool", "getwindowname", wid]).stdout.strip()
    if name:
        return name
    try:
        out = run(["xprop", "-id", wid, "_NET_WM_NAME", "WM_NAME"]).stdout
        for line in out.splitlines():
            if " = " in line:
                val = line.split("=", 1)[1].strip().strip('"')
                if val and val != "not found":
                    return val[:80]
    except (subprocess.CalledProcessError, OSError):
        pass
    return "(unnamed)"


def cmd_state(_a):
    require_env()
    print("== visible windows ==")
    r = run(["xdotool", "search", "--onlyvisible", "--name", ".*"])
    for wid in r.stdout.split()[:40]:
        name = get_window_name(wid)[:60]
        geo = " ".join(run(["xdotool", "getwindowgeometry", wid]).stdout.split())
        print(f"{wid} | {name} | {geo}")
    print("== active window ==")
    wid = run(["xdotool", "getactivewindow"]).stdout.strip()
    name = get_window_name(wid) if wid else ""
    print(f"active={wid} {name}")


def cmd_find(a):
    require_env()
    r = run(["xdotool", "search", "--onlyvisible", "--name", a.pattern])
    matches = r.stdout.split() if r.stdout.strip() else []
    if not matches:
        r = run(["xdotool", "search", "--onlyvisible", "--class", a.pattern])
        matches = r.stdout.split() if r.stdout.strip() else []
    if not matches:
        die(f"NOT_FOUND: 未找到匹配窗口 ({a.pattern})")
    if len(matches) > 1:
        print(f"WARN: {len(matches)} 个窗口匹配，锁定第一个（用 state 查看全部）:")
        for m in matches[:5]:
            print(f"  {m} | {get_window_name(m)[:50]}")
    wid = matches[0]
    lock_wid(wid)
    name = get_window_name(wid)
    log(f"lock wid={wid} pattern={a.pattern}")
    print(f"LOCKED WID={wid} name={name} state={STATE_FILE}")


def cmd_shot(a):
    require_env()
    wid = a.window if a.window else load_wid()
    screenshot(a.out, window_id=wid)


def cmd_click(a):
    require_env()
    wid = load_wid()
    activate(wid)
    run(["xdotool", "mousemove", str(a.x), str(a.y)])
    time.sleep(0.15)
    run(["xdotool", "click", "--repeat", str(a.count), "--delay", "100", str(a.button)])
    time.sleep(0.5)
    if shutil.which("import"):
        run(["import", "-window", "root", a.verify_shot])
        print(f"VERIFY_SHOT {a.verify_shot}")
    log(f"click x={a.x} y={a.y} button={a.button} count={a.count} wid={wid or 'none'} shot={a.verify_shot}")
    print(f"CLICK {a.x},{a.y} button={a.button} count={a.count} done (window={wid or 'none'})")


def cmd_drag(a):
    require_env()
    wid = load_wid()
    activate(wid)
    run(["xdotool", "mousemove", str(a.x1), str(a.y1)])
    time.sleep(0.15)
    run(["xdotool", "mousedown", str(a.button)])
    steps = max(1, a.steps)
    for i in range(1, steps + 1):
        x = a.x1 + (a.x2 - a.x1) * i // steps
        y = a.y1 + (a.y2 - a.y1) * i // steps
        run(["xdotool", "mousemove", str(x), str(y)])
        time.sleep(0.02)
    time.sleep(0.1)
    run(["xdotool", "mouseup", str(a.button)])
    time.sleep(0.3)
    log(f"drag ({a.x1},{a.y1})->({a.x2},{a.y2}) button={a.button} steps={steps} wid={wid or 'none'}")
    print(f"DRAG ({a.x1},{a.y1})->({a.x2},{a.y2}) done")


def cmd_scroll(a):
    require_env()
    btn = {"up": 4, "down": 5, "left": 6, "right": 7}.get(a.dir)
    if not btn:
        die("方向须为 up|down|left|right")
    run(["xdotool", "mousemove", str(a.x), str(a.y)])
    time.sleep(0.1)
    run(["xdotool", "click", "--repeat", str(a.n), "--delay", "60", str(btn)])
    time.sleep(0.3)
    log(f"scroll {a.dir} x={a.x} y={a.y} n={a.n} wid={load_wid() or 'none'}")
    print(f"SCROLL {a.dir} x{a.n} done")


def cmd_type(a):
    require_env()
    wid = load_wid()
    if wid:
        if run(["xdotool", "windowactivate", wid]).returncode != 0:
            run(["xdotool", "windowfocus", wid])
        time.sleep(0.3)
    # 注意: type 走当前键盘布局；特殊字符/组合键请用 key
    run(["xdotool", "type", "--clearmodifiers", "--delay", str(a.delay), "--", a.text])
    log(f"type len={len(a.text)} delay={a.delay} wid={wid or 'none'}")  # 只记长度防泄露
    print(f"TYPED {len(a.text)} chars (window={wid or 'none'})")


def cmd_key(a):
    require_env()
    wid = load_wid()
    if wid:
        if run(["xdotool", "windowactivate", wid]).returncode != 0:
            run(["xdotool", "windowfocus", wid])
        time.sleep(0.2)
    run(["xdotool", "key"] + a.keysyms)
    log(f"key {' '.join(a.keysyms)} wid={wid or 'none'}")
    print(f"KEY {' '.join(a.keysyms)} done")


def cmd_wait(a):
    require_env()
    deadline = time.time() + a.timeout
    while time.time() < deadline:
        r = run(["xdotool", "search", "--onlyvisible", "--name", a.pattern])
        n = len(r.stdout.split())
        if a.mode == "disappear" and n == 0:
            print(f"OK disappeared ({a.pattern})")
            return 0
        if a.mode == "appear" and n > 0:
            print(f"OK appeared ({a.pattern})")
            return 0
        time.sleep(0.5)
    print(f"TIMEOUT after {a.timeout}s waiting for {a.mode}: {a.pattern}")
    return 1


def cmd_smoke(_a):
    """冒烟测试：环境检查 → 启动 xmessage → 找窗 → 截图 → 点击 → 校验关闭。"""
    require_env()
    print("== 1/6 env ==")
    cmd_env(_a)
    if shutil.which("xmessage") is None:
        print("SKIP: 无 xmessage，无法做 GUI 冒烟测试")
        return 0
    tag = f"SMOKE-TEST-{os.getpid()}"
    print("== 2/6 launch app ==")
    proc = subprocess.Popen(["xmessage", "-title", tag, "smoke test, click OK"])
    time.sleep(1)
    print("== 3/6 find window ==")
    cmd_find(SimpleNamespace(pattern=tag))
    wid = load_wid()
    geo = window_geometry(wid)
    print("== 4/6 screenshot ==")
    screenshot("/tmp/gui_smoke_before.png")
    print("== 5/6 click (几何换算) ==")
    cx = int(geo.get("X", 0)) + 34
    cy = int(geo.get("Y", 0)) + 20
    cmd_click(SimpleNamespace(x=cx, y=cy, button=1, count=1,
                              verify_shot="/tmp/gui_smoke_after.png"))
    time.sleep(1)
    print("== 6/6 verify ==")
    r = run(["xdotool", "search", "--onlyvisible", "--name", tag])
    if r.stdout.strip():
        print("SMOKE FAIL: 点击后窗口仍存在")
        proc.kill()
        return 1
    print("SMOKE PASS: 点击后窗口关闭，闭环可用")
    return 0


# ---------- 入口 ----------

def build_parser():
    p = argparse.ArgumentParser(
        prog="gui.py",
        description="xdotool 驱动的 X11 GUI 调试原语（观察→决策→动作→校验闭环）",
    )
    sub = p.add_subparsers(dest="cmd")

    sub.add_parser("env", help="环境检查（X11/DISPLAY/xdotool）").set_defaults(func=cmd_env)
    sub.add_parser("state", help="窗口状态快照（可见窗口+焦点）").set_defaults(func=cmd_state)

    sp = sub.add_parser("find", help="按名称/class 锁定目标窗口")
    sp.add_argument("pattern")
    sp.set_defaults(func=cmd_find)

    sp = sub.add_parser("shot", help="截图（观察通道）")
    sp.add_argument("out", nargs="?", default="/tmp/gui_shot.png")
    sp.add_argument("--window", help="窗口 ID（默认整屏；已锁定窗口时自动用锁定窗口）")
    sp.set_defaults(func=cmd_shot)

    sp = sub.add_parser("click", help="点击（动作+校验）")
    sp.add_argument("x", type=int)
    sp.add_argument("y", type=int)
    sp.add_argument("--button", type=int, default=1, choices=[1, 2, 3], help="1=左 2=中 3=右")
    sp.add_argument("--count", type=int, default=1, help="1=单击 2=双击")
    sp.add_argument("--verify-shot", default="/tmp/gui_after.png", help="动作后校验截图路径")
    sp.set_defaults(func=cmd_click)

    sp = sub.add_parser("drag", help="拖拽（按下→分步移动→释放）")
    sp.add_argument("x1", type=int); sp.add_argument("y1", type=int)
    sp.add_argument("x2", type=int); sp.add_argument("y2", type=int)
    sp.add_argument("--button", type=int, default=1, choices=[1, 2, 3])
    sp.add_argument("--steps", type=int, default=8)
    sp.set_defaults(func=cmd_drag)

    sp = sub.add_parser("scroll", help="滚轮")
    sp.add_argument("x", type=int); sp.add_argument("y", type=int)
    sp.add_argument("dir", choices=["up", "down", "left", "right"])
    sp.add_argument("n", type=int, nargs="?", default=3, help="滚动格数")
    sp.set_defaults(func=cmd_scroll)

    sp = sub.add_parser("type", help="聚焦后打字")
    sp.add_argument("text")
    sp.add_argument("--delay", type=int, default=30, help="每键延时(ms)")
    sp.set_defaults(func=cmd_type)

    sp = sub.add_parser("key", help="组合键/功能键")
    sp.add_argument("keysyms", nargs="+")
    sp.set_defaults(func=cmd_key)

    sp = sub.add_parser("wait", help="轮询等待窗口条件")
    sp.add_argument("pattern")
    sp.add_argument("timeout", type=int)
    sp.add_argument("mode", nargs="?", choices=["appear", "disappear"], default="appear")
    sp.set_defaults(func=cmd_wait)

    sub.add_parser("smoke", help="冒烟测试").set_defaults(func=cmd_smoke)
    return p


def main():
    args = build_parser().parse_args()
    if not getattr(args, "cmd", None):
        print("ERROR: 缺少子命令（env|state|find|shot|click|drag|scroll|type|key|wait|smoke）", file=sys.stderr)
        sys.exit(2)
    sys.exit(args.func(args) or 0)


if __name__ == "__main__":
    main()
