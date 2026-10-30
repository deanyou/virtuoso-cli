#!/usr/bin/env python3
"""Auto-approve the ViraStack (Claude Code UI) permission prompt.

Polls the target X11 window, locates the "1 Allow / 2 Don't Allow" prompt by
template matching, and clicks "Allow" through `vcli window action-x11`.

Detection uses the ImageMagick-free path (PIL + numpy + scipy); every GUI
*input* goes through the Rust binary boundary (`vcli window action-x11`).

Usage:
    python3 auto_allow.py --window-id 0x2400330 --display :0 \
        --vcli /path/to/vcli --session SESSION_ID [--max-clicks 20] [--dry-run]
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time

import numpy as np
from PIL import Image
from scipy.signal import fftconvolve

DEFAULT_TEMPLATE = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "data",
    "allow_prompt_template.png",
)

# Offset of the "Allow" label inside the template. The template spans BOTH
# option rows ("1 Allow" and "2 Don't Allow"), so a match can never be the
# "Don't Allow" row on its own — that row alone scored 0.77 against the
# single-row template, far too close to the threshold.
ALLOW_OFFSET = (32, 16)
MATCH_THRESHOLD = 0.90
# Consecutive rejected clicks before the loop gives up.
MAX_CLICK_FAILURES = 3


def run(cmd):
    proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return proc.returncode, proc.stdout.decode("utf-8", "replace"), proc.stderr.decode("utf-8", "replace")


def capture(vcli, session, window_id, display, workdir):
    rc, out, err = run([
        vcli, "--session", session, "window", "action-x11",
        "--window-id", window_id, "--display", display,
        "--operation", "screenshot", "--output-dir", workdir,
    ])
    if rc != 0:
        raise RuntimeError("screenshot failed: %s %s" % (out.strip(), err.strip()))
    data = json.loads(out)
    path = (data.get("artifact") or {}).get("local_path")
    if not path or not os.path.exists(path):
        raise RuntimeError("screenshot artifact missing: %s" % out.strip())
    return path


def ncc_map(image, template):
    """Normalized cross-correlation of `template` over `image` (valid positions)."""
    img = image.astype(np.float64)
    tpl = template.astype(np.float64)
    tpl = tpl - tpl.mean()
    t_norm = np.sqrt((tpl ** 2).sum())
    h, w = tpl.shape
    corr = fftconvolve(img, tpl[::-1, ::-1], mode="valid")

    integ = np.zeros((img.shape[0] + 1, img.shape[1] + 1))
    integ[1:, 1:] = img.cumsum(axis=0).cumsum(axis=1)
    integ2 = np.zeros_like(integ)
    integ2[1:, 1:] = (img ** 2).cumsum(axis=0).cumsum(axis=1)

    def window_sum(i):
        return i[h:, w:] - i[:-h, w:] - i[h:, :-w] + i[:-h, :-w]

    s1 = window_sum(integ)
    s2 = window_sum(integ2)
    n = float(h * w)
    mean = s1 / n
    var = np.maximum(s2 / n - mean ** 2, 0.0)
    denom = t_norm * np.sqrt(var * n)
    with np.errstate(divide="ignore", invalid="ignore"):
        return np.where(denom > 0, corr / denom, 0.0)


def load_template(template_path):
    """Load the detection template once — the loop must not depend on the
    file still being there on every poll."""
    return np.array(Image.open(template_path).convert("L"))


def find_prompt(screenshot, template):
    """Return (x, y, score) of the Allow label, or None when the prompt is absent."""
    image = np.array(Image.open(screenshot).convert("L"))
    tpl = template
    if image.shape[0] < tpl.shape[0] or image.shape[1] < tpl.shape[1]:
        return None
    scores = ncc_map(image, tpl)
    idx = int(np.argmax(scores))
    score = float(scores.flat[idx])
    if score < MATCH_THRESHOLD:
        return None
    y, x = np.unravel_index(idx, scores.shape)
    return x + ALLOW_OFFSET[0], y + ALLOW_OFFSET[1], score


def click(vcli, session, window_id, display, x, y, dry_run):
    if dry_run:
        print("dry-run: would click Allow at (%d, %d)" % (x, y))
        return True
    rc, out, err = run([
        vcli, "--session", session, "window", "action-x11",
        "--window-id", window_id, "--display", display,
        "--operation", "click-rel", "--x", str(x), "--y", str(y), "--direct",
    ])
    if rc != 0:
        print("click failed: %s %s" % (out.strip(), err.strip()))
        return False
    print("clicked Allow at (%d, %d)" % (x, y))
    return True


def wait_until_progress(args, workdir, before, region, polls):
    """Poll until the area around the prompt changes.

    The click is considered consumed when the pixels around the prompt block
    differ from the pre-click snapshot: the old prompt was dismissed (and a
    new one may already have taken its place). If the area stays identical,
    the click never landed on "Allow" — the caller stops instead of hammering
    the same coordinates.
    """
    for _ in range(polls):
        time.sleep(args.interval)
        try:
            shot = capture(args.vcli, args.session, args.window_id, args.display, workdir)
        except Exception:  # noqa: BLE001 — treat a capture failure as "no progress"
            continue
        current = crop_region(shot, region)
        if current is not None and not np.array_equal(current, before):
            return True
    return False


def crop_region(path, region):
    """Crop (x, y, w, h) out of a screenshot, clamped to the image.

    Clamping instead of bailing out matters: the region is a fixed 700x260
    block around the prompt, and a narrower window would otherwise make the
    post-click guard silently skip instead of checking anything.
    """
    x, y, w, h = region
    img = Image.open(path).convert("L")
    left = max(0, min(x, img.size[0]))
    top = max(0, min(y, img.size[1]))
    right = max(left, min(x + w, img.size[0]))
    bottom = max(top, min(y + h, img.size[1]))
    if right <= left or bottom <= top:
        return None
    return np.array(img.crop((left, top, right, bottom)))


def idle_limit_reached(idle_polls, idle):
    """Whether the loop should stop because no prompt showed up.

    `idle_polls == 0` disables the idle stop entirely: a cellview run
    (apRunDeviceGeneration and friends) can keep the agent busy for far
    longer than any idle budget without asking for permission, and the
    loop must survive that instead of quitting unnoticed.
    """
    return bool(idle_polls) and idle >= idle_polls


def list_sessions(vcli):
    """Ids of the live daemon sessions."""
    rc, out, err = run([vcli, "session", "list", "--format", "json"])
    if rc != 0:
        return []
    try:
        data = json.loads(out)
    except ValueError:
        return []
    return [s.get("id") for s in (data.get("sessions") or []) if s.get("id")]


def resolve_session(vcli, session):
    """Keep the current session if it is alive, else adopt the only live one."""
    ids = list_sessions(vcli)
    if session and session in ids:
        return session
    if len(ids) == 1:
        return ids[0]
    return session


def resolve_window(vcli, session, title_prefix):
    """Window id of the first window whose title starts with `title_prefix`."""
    rc, out, err = run([
        vcli, "--session", session, "window", "list-windows-x11", "--format", "json",
    ])
    if rc != 0:
        return None
    try:
        data = json.loads(out)
    except ValueError:
        return None
    for w in data.get("windows") or []:
        if (w.get("title") or "").startswith(title_prefix):
            return w.get("window_id")
    return None


def recover_target(args):
    """Re-resolve session and window id after a Virtuoso or daemon restart.

    A restart invalidates both the daemon session id and the X11 window id.
    Without this the loop keeps failing to capture forever and silently
    stops approving.
    """
    session = resolve_session(args.vcli, args.session)
    window_id = resolve_window(args.vcli, session, args.title_prefix)
    if not window_id:
        return False
    if session == args.session and window_id == args.window_id:
        return False
    print("re-resolved target: session=%s window=%s" % (session, window_id))
    args.session = session
    args.window_id = window_id
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--window-id", default="",
                    help="X11 window id of the ViraStack client; resolved from the title when omitted")
    ap.add_argument("--display", default=":0")
    ap.add_argument("--vcli", required=True)
    ap.add_argument("--session", default="")
    ap.add_argument("--template", default=DEFAULT_TEMPLATE)
    ap.add_argument("--interval", type=float, default=2.0)
    ap.add_argument("--max-clicks", type=int, default=0, help="0 = unlimited")
    ap.add_argument("--timeout", type=float, default=600.0)
    ap.add_argument("--idle-polls", type=int, default=3,
                    help="stop after this many consecutive polls without a prompt; 0 = never stop on idle")
    ap.add_argument("--settle-polls", type=int, default=5,
                    help="polls to wait for the prompt to vanish after a click")
    ap.add_argument("--title-prefix", default="ViraStack",
                    help="window title prefix used to re-resolve the target after a restart")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    if not os.path.exists(args.template):
        print("template not found: %s" % args.template)
        return 2
    template = load_template(args.template)

    if not args.session:
        args.session = resolve_session(args.vcli, "")
        if not args.session:
            print("no live vcli session — start virtuoso-daemon first")
            return 2
        print("resolved session=%s" % args.session)

    if not args.window_id:
        args.window_id = resolve_window(args.vcli, args.session, args.title_prefix) or ""
        if not args.window_id:
            print("no window titled %r — is ViraStack running?" % args.title_prefix)
            return 2
        print("resolved window=%s" % args.window_id)

    deadline = time.time() + args.timeout
    clicks = 0
    idle = 0
    failed = 0
    capture_failures = 0
    workdir = tempfile.mkdtemp(prefix="auto_allow_")

    while time.time() < deadline:
        try:
            shot = capture(args.vcli, args.session, args.window_id, args.display, workdir)
            capture_failures = 0
        except Exception as exc:  # noqa: BLE001
            capture_failures += 1
            # A Virtuoso or daemon restart invalidates the session id and the
            # window id. Re-resolve them instead of failing forever.
            if capture_failures % 10 == 1 and recover_target(args):
                continue
            print("capture error: %s" % exc)
            time.sleep(args.interval)
            continue
        found = find_prompt(shot, template)
        if found is None:
            idle += 1
            if idle_limit_reached(args.idle_polls, idle):
                print("no pending prompt after %d idle polls" % idle)
                return 0
        else:
            idle = 0
            x, y, score = found
            print("prompt detected (ncc=%.3f) -> Allow at (%d, %d)" % (score, x, y))
            if click(args.vcli, args.session, args.window_id, args.display, x, y, args.dry_run):
                clicks += 1
                failed = 0
                # The click must have consumed the prompt. Compare the area
                # around the prompt block with the pre-click snapshot: if it
                # never changes, the click did not land on "Allow" and we stop
                # instead of hammering the same coordinates. A dry run never
                # clicks, so there is nothing to verify.
                if not args.dry_run:
                    block_y = y - ALLOW_OFFSET[1]
                    region = (20, max(0, block_y - 160), 700, 260)
                    before = crop_region(shot, region)
                    if before is not None and not wait_until_progress(
                        args, workdir, before, region, polls=args.settle_polls
                    ):
                        print("prompt area unchanged after click — aborting")
                        return 1
            else:
                # The click itself was rejected (window gone, action refused).
                # Retrying the same coordinates will not help — stop instead of
                # spinning until the timeout.
                failed += 1
                if failed >= MAX_CLICK_FAILURES:
                    print("%d consecutive click failures — aborting" % failed)
                    return 1
            if args.max_clicks and clicks >= args.max_clicks:
                print("reached --max-clicks=%d" % args.max_clicks)
                return 0
        time.sleep(args.interval)

    print("timeout reached after %d clicks" % clicks)
    return 0


if __name__ == "__main__":
    sys.exit(main())
