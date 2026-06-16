#!/usr/bin/env python3
"""X11 dialog finder and dismisser for Virtuoso. Runs on the remote Virtuoso host.

Adapted from virtuoso-bridge-lite
(https://github.com/Arcadia-1/virtuoso-bridge-lite), which is MIT-licensed.

Usage:
    python3 x11_dismiss_dialog.py [DISPLAY] [--dismiss] [--action enter|escape|alt-y|alt-n]

Output (stdout): JSON lines, one per dialog found:
    {"window_id": "0x2e01f16", "title": "Save Changes", "x": 1010, "y": 378, "w": 239, "h": 142}

With --dismiss, the chosen --action is sent to each dialog after reporting it.
DISPLAY/XAUTHORITY are auto-detected from the running virtuoso process if omitted.

Exit codes: 0 = dialogs found/dismissed, 1 = no dialogs found, 2 = error.

Notes (pinned here for downstream test stability; see Virtuoso FAQ):
- Modal dialogs at 1x DPI:  ~300-600w x 100-350h (e.g. ADE "Update and Run" 580x140).
- Editor/log panes:         ~500-800w x 500-900h.
- Main app frames:          1200+w x 700+h.
- We skip windows with h > 420 (tall=editor pane) or w > 1000 && h > 300 (main frame).
"""
import ctypes
import ctypes.util
import json
import os
import subprocess
import sys
import time

VIRTUOSO_WM_CLASSES = ["virtuoso", "libManager"]

# Geometric thresholds; see module docstring for observed Virtuoso window sizes.
MAX_DIALOG_HEIGHT = 420
MAX_DIALOG_WHEN_LARGE_WIDTH = 1000
MAX_DIALOG_WHEN_LARGE_HEIGHT = 300
MIN_DIALOG_DIM = 20

VALID_ACTIONS = ("enter", "escape", "alt-y", "alt-n")

KEYSYM_RETURN = 0xFF0D
KEYSYM_ESCAPE = 0xFF1B
KEYSYM_N = 0x006E
KEYSYM_Y = 0x0079
KEYSYM_ALT_L = 0xFFE9


def find_x11_env(user=None):
    """Auto-detect DISPLAY and XAUTHORITY from running virtuoso process.

    Skips batch virtuoso processes (those with -nograph in cmdline).
    Returns first candidate found, or {"DISPLAY": None, "XAUTHORITY": None}.
    """
    candidates = []
    try:
        pids = subprocess.check_output(
            ["pgrep", "-u", user or os.environ.get("USER", ""), "-x", "virtuoso"],
            stderr=subprocess.PIPE,
        ).strip().splitlines()
    except (subprocess.CalledProcessError, OSError):
        return {"DISPLAY": None, "XAUTHORITY": None}

    for raw_pid in pids:
        pid = raw_pid.strip().decode("utf-8", "replace")
        if not pid:
            continue
        # Skip batch processes (have -nograph in cmdline)
        try:
            cmdline = open("/proc/%s/cmdline" % pid, "rb").read()
            if b"-nograph" in cmdline:
                continue
        except (IOError, OSError):
            pass
        env_file = "/proc/%s/environ" % pid
        try:
            data = open(env_file, "rb").read()
        except (IOError, OSError):
            continue
        info = {"DISPLAY": None, "XAUTHORITY": None}
        for chunk in data.split(b"\x00"):
            if chunk.startswith(b"DISPLAY="):
                info["DISPLAY"] = chunk.split(b"=", 1)[1].decode("utf-8", "replace")
            elif chunk.startswith(b"XAUTHORITY="):
                info["XAUTHORITY"] = chunk.split(b"=", 1)[1].decode("utf-8", "replace")
        if info["DISPLAY"]:
            candidates.append(info)

    if not candidates:
        return {"DISPLAY": None, "XAUTHORITY": None}
    return candidates[0]


def find_dialogs(display):
    """Find top-level dialog windows belonging to Virtuoso.

    Returns a list of dicts: {window_id, title, x, y, w, h}.
    Empty list if no dialogs found or xwininfo is missing.
    """
    os.environ["DISPLAY"] = display
    try:
        tree = subprocess.check_output(
            ["xwininfo", "-root", "-children"],
            stderr=subprocess.PIPE,
        ).decode("utf-8", "replace")
    except (subprocess.CalledProcessError, OSError) as exc:
        print(json.dumps({"error": "xwininfo failed: %s" % exc}))
        return []

    # Step 1: collect top-level frame IDs that look dialog-sized.
    candidates = []
    in_children = False
    for line in tree.splitlines():
        if "children" in line.lower() and ":" in line:
            in_children = True
            continue
        if not in_children:
            continue
        parts = line.strip().split()
        if not parts or not parts[0].startswith("0x"):
            continue
        win_id = parts[0]
        geo_w = geo_h = 0
        for token in parts:
            if "x" in token and "+" in token and token[0].isdigit():
                try:
                    size, _, _ = token.partition("+")
                    geo_w, geo_h = (int(v) for v in size.split("x"))
                except (ValueError, IndexError):
                    pass
        if geo_w < MIN_DIALOG_DIM or geo_h < MIN_DIALOG_DIM:
            continue
        if geo_h > MAX_DIALOG_HEIGHT:
            continue
        if geo_w > MAX_DIALOG_WHEN_LARGE_WIDTH and geo_h > MAX_DIALOG_WHEN_LARGE_HEIGHT:
            continue
        candidates.append(win_id)

    # Step 2: keep only frames whose subtree contains a virtuoso-class window.
    dialogs = []
    for win_id in candidates:
        try:
            subtree = subprocess.check_output(
                ["xwininfo", "-id", win_id, "-tree"],
                stderr=subprocess.PIPE,
            ).decode("utf-8", "replace")
        except (subprocess.CalledProcessError, OSError):
            continue
        is_virtuoso = False
        child_title = ""
        for sl in subtree.splitlines():
            for cls in VIRTUOSO_WM_CLASSES:
                if ('"%s"' % cls) in sl:
                    is_virtuoso = True
                    if '"' in sl:
                        start = sl.index('"') + 1
                        end = sl.index('"', start)
                        child_title = sl[start:end]
                    break
            if is_virtuoso:
                break
        if not is_virtuoso:
            continue

        # Get precise geometry
        try:
            info = subprocess.check_output(
                ["xwininfo", "-id", win_id],
                stderr=subprocess.PIPE,
            ).decode("utf-8", "replace")
        except (subprocess.CalledProcessError, OSError):
            continue
        x = y = w = h = 0
        mapped = False
        for il in info.splitlines():
            il = il.strip()
            if il.startswith("Absolute upper-left X:"):
                x = int(il.split(":", 1)[1].strip())
            elif il.startswith("Absolute upper-left Y:"):
                y = int(il.split(":", 1)[1].strip())
            elif il.startswith("Width:"):
                w = int(il.split(":", 1)[1].strip())
            elif il.startswith("Height:"):
                h = int(il.split(":", 1)[1].strip())
            elif "Map State:" in il and "IsViewable" in il:
                mapped = True
        if not mapped:
            continue
        dialogs.append({
            "window_id": win_id,
            "title": child_title,
            "x": x, "y": y, "w": w, "h": h,
        })
    return dialogs


def _find_app_child(display, frame_id_str):
    """Find the actual app window inside a WM frame (first named child)."""
    try:
        tree = subprocess.check_output(
            ["xwininfo", "-id", frame_id_str, "-children"],
            stderr=subprocess.PIPE,
        ).decode("utf-8", "replace")
        for line in tree.splitlines():
            line = line.strip()
            if line.startswith("0x") and '"' in line:
                return line.split()[0]
    except (subprocess.CalledProcessError, OSError):
        pass
    return frame_id_str  # fallback to frame itself


def _parse_window_line(line):
    """Parse a line from `xwininfo -root -children` or `-tree`.

    Returns a dict {id, title, class} or None if the line doesn't look
    like a window row.
    """
    line = line.strip()
    if not line.startswith("0x"):
        return None
    parts = line.split()
    win_id = parts[0]
    title = ""
    classes = []
    for token in parts:
        if token.startswith('"') and token.endswith('"') and len(token) > 2:
            # "Title" or "Class:Subclass"
            inner = token[1:-1]
            if ":" in inner and " " not in inner:
                classes.extend(inner.split(":"))
            else:
                title = inner
    return {"id": win_id, "title": title, "class": classes}


def _read_window_geometry(win_id):
    """Read x, y, w, h and mapped state for a window. Returns a dict."""
    try:
        info = subprocess.check_output(
            ["xwininfo", "-id", win_id],
            stderr=subprocess.PIPE,
        ).decode("utf-8", "replace")
    except (subprocess.CalledProcessError, OSError):
        return {"x": 0, "y": 0, "w": 0, "h": 0, "mapped": False}
    geometry = {"x": 0, "y": 0, "w": 0, "h": 0}
    mapped = False
    for il in info.splitlines():
        il = il.strip()
        if il.startswith("Absolute upper-left X:"):
            try:
                geometry["x"] = int(il.split(":", 1)[1].strip())
            except ValueError:
                pass
        elif il.startswith("Absolute upper-left Y:"):
            try:
                geometry["y"] = int(il.split(":", 1)[1].strip())
            except ValueError:
                pass
        elif il.startswith("Width:"):
            try:
                geometry["w"] = int(il.split(":", 1)[1].strip())
            except ValueError:
                pass
        elif il.startswith("Height:"):
            try:
                geometry["h"] = int(il.split(":", 1)[1].strip())
            except ValueError:
                pass
        elif "Map State:" in il and "IsViewable" in il:
            mapped = True
    geometry["mapped"] = mapped
    return geometry


def _is_virtuoso_class(classes):
    lowered = [c.lower() for c in (classes or [])]
    for cls in VIRTUOSO_WM_CLASSES:
        if cls.lower() in lowered:
            return True
    return False


def _root_frames():
    """Enumerate top-level window frames from `xwininfo -root -children`."""
    try:
        tree = subprocess.check_output(
            ["xwininfo", "-root", "-children"],
            stderr=subprocess.PIPE,
        ).decode("utf-8", "replace")
    except (subprocess.CalledProcessError, OSError) as exc:
        print(json.dumps({"error": "xwininfo failed: %s" % exc}))
        return []
    frames = []
    in_children = False
    for line in tree.splitlines():
        if "children" in line.lower() and ":" in line:
            in_children = True
            continue
        if not in_children:
            continue
        frame = _parse_window_line(line)
        if not frame:
            continue
        frames.append(frame)
    return frames


def _frame_children(frame_id):
    """Enumerate the immediate children of a frame (1 level of `xwininfo -tree`)."""
    try:
        tree = subprocess.check_output(
            ["xwininfo", "-id", frame_id, "-tree"],
            stderr=subprocess.PIPE,
        ).decode("utf-8", "replace")
    except (subprocess.CalledProcessError, OSError):
        return []
    children = []
    for line in tree.splitlines():
        child = _parse_window_line(line)
        if child and child["id"] != frame_id:
            children.append(child)
    return children


def _is_dialog_sized(geometry):
    """Geometric test: is this window a modal dialog (not editor pane / main frame)?"""
    w = int(geometry.get("w") or 0)
    h = int(geometry.get("h") or 0)
    if w < MIN_DIALOG_DIM or h < MIN_DIALOG_DIM:
        return False
    if h > MAX_DIALOG_HEIGHT:
        return False
    if w > MAX_DIALOG_WHEN_LARGE_WIDTH and h > MAX_DIALOG_WHEN_LARGE_HEIGHT:
        return False
    return True


def discover_windows(display):
    """Enumerate Virtuoso-related X11 windows with frame + child details.

    Returns a list of dicts: {frame_id, window_id, dismiss_id, title, class, geometry}.
    Each Virtuoso-associated child window is one entry, with the parent frame_id
    recorded alongside so callers can dismiss via the child id directly.

    Unlike `find_dialogs`, this does NOT apply the dialog-size filter — it returns
    every Virtuoso-related window, so callers can decide which to dismiss.
    """
    os.environ["DISPLAY"] = display
    windows = []
    seen = set()
    for frame in _root_frames():
        frame_id = frame["id"]
        geometry = _read_window_geometry(frame_id)
        if not geometry.get("mapped", False):
            continue
        children = _frame_children(frame_id)
        virt_children = [c for c in children if _is_virtuoso_class(c.get("class"))]
        if _is_virtuoso_class(frame.get("class")):
            virt_children.append(frame)
        for child in virt_children:
            dismiss_id = child["id"]
            key = (frame_id, dismiss_id)
            if key in seen:
                continue
            seen.add(key)
            windows.append({
                "frame_id": frame_id,
                "window_id": dismiss_id,
                "dismiss_id": dismiss_id,
                "title": child.get("title") or frame.get("title") or "",
                "class": child.get("class") or frame.get("class") or [],
                "geometry": {
                    "w": int(geometry.get("w") or 0),
                    "h": int(geometry.get("h") or 0),
                    "x": int(geometry.get("x") or 0),
                    "y": int(geometry.get("y") or 0),
                },
            })
    return windows


def _press_pair(dpy, xlib, xtst, kc_modifier, kc_key, action_name):
    """Press modifier+key, release, and return the action name + keycodes."""
    if kc_modifier is not None:
        xtst.XTestFakeKeyEvent(dpy, kc_modifier, True, 0)
    xtst.XTestFakeKeyEvent(dpy, kc_key, True, 0)
    xtst.XTestFakeKeyEvent(dpy, kc_key, False, 0)
    if kc_modifier is not None:
        xtst.XTestFakeKeyEvent(dpy, kc_modifier, False, 0)
    xlib.XFlush(dpy)
    return action_name


def dismiss_window(display, win_id_str, action, title="", target_is_explicit=False):
    """Dismiss a window via XTest.

    `action` is one of 'enter' (default), 'escape', 'alt-y', 'alt-n'.
    Raises RuntimeError on display/X11/lib loading failure.

    When `target_is_explicit=True` (i.e., called from `--dismiss-window <ID>`),
    `win_id_str` is the caller-provided target and is used directly. Otherwise
    we resolve the WM frame to its first named child (the actual app window).
    """
    if action not in VALID_ACTIONS:
        raise ValueError("action must be one of %s" % (VALID_ACTIONS,))
    os.environ["DISPLAY"] = display
    xlib_path = ctypes.util.find_library("X11")
    xtst_path = ctypes.util.find_library("Xtst")
    if not xlib_path or not xtst_path:
        raise RuntimeError("libX11 or libXtst not found on remote host")

    xlib = ctypes.cdll.LoadLibrary(xlib_path)
    xtst = ctypes.cdll.LoadLibrary(xtst_path)
    dpy = xlib.XOpenDisplay(None)
    if not dpy:
        raise RuntimeError("cannot open display %s" % display)

    try:
        # If caller passed an explicit window id, use it directly; otherwise
        # resolve the WM frame to its app child.
        if target_is_explicit:
            child_id_str = win_id_str
        else:
            child_id_str = _find_app_child(display, win_id_str)
        child_id = int(child_id_str, 16) if child_id_str.startswith("0x") else int(child_id_str)
        xlib.XRaiseWindow(dpy, child_id)
        xlib.XSetInputFocus(dpy, child_id, 1, 0)  # RevertToParent
        xlib.XFlush(dpy)
        time.sleep(0.15)

        kc_alt = xlib.XKeysymToKeycode(dpy, KEYSYM_ALT_L)
        if action == "enter":
            keycode = xlib.XKeysymToKeycode(dpy, KEYSYM_RETURN)
            xtst.XTestFakeKeyEvent(dpy, keycode, True, 0)
            xtst.XTestFakeKeyEvent(dpy, keycode, False, 0)
            xlib.XFlush(dpy)
            return {
                "dismissed": win_id_str, "child": child_id_str, "title": title,
                "action": "enter", "keycode": int(keycode),
            }
        if action == "escape":
            keycode = xlib.XKeysymToKeycode(dpy, KEYSYM_ESCAPE)
            xtst.XTestFakeKeyEvent(dpy, keycode, True, 0)
            xtst.XTestFakeKeyEvent(dpy, keycode, False, 0)
            xlib.XFlush(dpy)
            return {
                "dismissed": win_id_str, "child": child_id_str, "title": title,
                "action": "escape", "keycode": int(keycode),
            }
        if action == "alt-y":
            kc_y = xlib.XKeysymToKeycode(dpy, KEYSYM_Y)
            _press_pair(dpy, xlib, xtst, kc_alt, kc_y, "alt-y")
            return {
                "dismissed": win_id_str, "child": child_id_str, "title": title,
                "action": "alt-y", "keycode_alt": int(kc_alt), "keycode_y": int(kc_y),
            }
        if action == "alt-n":
            kc_n = xlib.XKeysymToKeycode(dpy, KEYSYM_N)
            _press_pair(dpy, xlib, xtst, kc_alt, kc_n, "alt-n")
            return {
                "dismissed": win_id_str, "child": child_id_str, "title": title,
                "action": "alt-n", "keycode_alt": int(kc_alt), "keycode_n": int(kc_n),
            }
        raise AssertionError("unreachable: action=%r" % action)
    finally:
        xlib.XCloseDisplay(dpy)


def main():
    args = sys.argv[1:]
    display = None
    do_dismiss = False
    list_windows = False
    dismiss_target = None
    action = "enter"

    i = 0
    while i < len(args):
        a = args[i]
        if a == "--dismiss":
            do_dismiss = True
        elif a == "--list-windows":
            list_windows = True
        elif a == "--dismiss-window":
            if i + 1 >= len(args):
                print(json.dumps({"error": "--dismiss-window requires a window id"}))
                sys.exit(2)
            dismiss_target = args[i + 1]
            i += 1
        elif a == "--action" and i + 1 < len(args):
            action = args[i + 1]
            i += 1
        elif a in ("-h", "--help"):
            print(
                "usage: x11_dismiss_dialog.py [DISPLAY] [options]\n"
                "  --list-windows                 enumerate Virtuoso-related X11 windows\n"
                "  --dismiss                      dismiss all detected dialogs\n"
                "  --dismiss-window <ID>          dismiss a specific window id\n"
                "  --action <a>                   enter|escape|alt-y|alt-n (default: enter)\n"
                "  --json                         reserved (always JSON)\n",
                file=sys.stderr,
            )
            sys.exit(0)
        elif not a.startswith("-"):
            display = a
        i += 1

    if action not in VALID_ACTIONS:
        print(json.dumps({"error": "invalid action: %s" % action}))
        sys.exit(2)

    if not display:
        x11_env = find_x11_env()
        display = x11_env.get("DISPLAY")
        if not display:
            print(json.dumps({"error": "cannot detect DISPLAY"}))
            sys.exit(2)
        xauth = x11_env.get("XAUTHORITY")
        if isinstance(xauth, str) and xauth:
            os.environ["XAUTHORITY"] = xauth

    if dismiss_target:
        # Single explicit window dismiss. Does NOT require dialog-size filter.
        try:
            result = dismiss_window(
                display, dismiss_target, action=action, target_is_explicit=True
            )
        except (RuntimeError, ValueError) as exc:
            result = {"error": str(exc), "window_id": dismiss_target}
        print(json.dumps(result))
        sys.exit(0 if "dismissed" in result else 1)

    if list_windows:
        windows = discover_windows(display)
        for w in windows:
            print(json.dumps(w))
        sys.exit(0 if windows else 1)

    dialogs = find_dialogs(display)
    for d in dialogs:
        print(json.dumps(d))

    if not dialogs:
        sys.exit(1)

    if do_dismiss:
        for d in dialogs:
            if "window_id" in d:
                try:
                    result = dismiss_window(
                        display, d["window_id"], action, d.get("title", "")
                    )
                except (RuntimeError, ValueError) as exc:
                    result = {"error": str(exc), "window_id": d["window_id"]}
                print(json.dumps(result))
    sys.exit(0)


if __name__ == "__main__":
    main()
