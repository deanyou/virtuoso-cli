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
from scipy.ndimage import maximum_filter
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
# Candidate rows of the composer input line, counted from the bottom of the
# client area. The exact row depends on the composer layout, so the guidance
# delivery tries them in order until the text shows up in a screenshot.
COMPOSER_CLICK_OFFSETS = (68, 74, 60)
# The command line is printed in a code block right above the option rows.
# The band is searched for in the rows above the Allow label: this far above,
# and this tall. The command line is then picked out of that region by ink.
COMMAND_REGION_ABOVE = 170
COMMAND_REGION_HIGH = 140
# The band spans the whole client area, minus the scrollbar on the right.
COMMAND_BAND_MAX = 1000
SCROLLBAR_MARGIN = 24
# Two bands of the same command match at least this fraction of their text.
# A re-rendered glyph still matches within a 3x3 neighbourhood, a changed
# character does not. Measured on the captured frames: the same command scores
# 1.0, the most similar pair of different commands 0.91.
COMMAND_SIMILARITY = 0.95
# How far two bands may be displaced and still be compared to each other.
BAND_SHIFT = 3
# Grey level from which a pixel counts as text, and the slack allowed when
# two composer crops are compared (the placeholder re-renders slightly).
COMPOSER_INK_THRESHOLD = 120
COMPOSER_INK_TOLERANCE = 20
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


def _text_groups(rows):
    """Contiguous runs of rows that hold ink, as (first, last) pairs."""
    ink_rows = np.nonzero(rows > 0)[0]
    if len(ink_rows) == 0:
        return []
    groups = []
    start = prev = int(ink_rows[0])
    for r in ink_rows[1:]:
        r = int(r)
        if r - prev > 3:
            groups.append((start, prev))
            start = r
        prev = r
    groups.append((start, prev))
    return groups


def _row_background(row):
    """The most common grey level of a row: the colour behind the text."""
    values, counts = np.unique(row, return_counts=True)
    return int(values[counts.argmax()])


def _band_width(screenshot):
    """Width of the band crop.

    The command has to be cut off at the scrollbar, but not before it: a
    command that runs past the crop would lose its tail and two different
    commands would share a fingerprint.
    """
    width = Image.open(screenshot).size[0]
    return max(200, min(width - SCROLLBAR_MARGIN, COMMAND_BAND_MAX))


def _card_background(crop, groups):
    """The grey level of the card the prompt is drawn on.

    It is the colour of the rows between the last text line and the option
    rows, not the most common colour of the whole region: a long command
    fills the region with the code block colour, and the command would then
    be mistaken for the background.
    """
    below = crop[groups[-1][1] + 1:]
    if len(below) >= 3:
        return _row_background(below.reshape(-1))
    return _row_background(crop[-3:].reshape(-1))


def command_band(screenshot, found):
    """The band of the screenshot that holds the command line of the prompt.

    The permission card shows the command in a code block, followed by the
    status line ("Running script...") and the option rows. The command is
    therefore *not* the last text line above the options — that is the status
    line, which is the same for every prompt and would make every request
    look like a repeat of the same one.

    The code block is the only part of the card with its own background, so
    the command is the last text line that does not sit on the background of
    the card itself. That background is read from the rows *below* the last
    text line — the gap between the card and the option rows — and not from
    the whole region: a long command fills the region with the code block
    colour, and the command would then be taken for the background.
    """
    _, y, _ = found
    crop = crop_region(screenshot, (
        0,
        max(0, y - COMMAND_REGION_ABOVE),
        _band_width(screenshot),
        COMMAND_REGION_HIGH,
    ))
    if crop is None:
        return None
    rows = (crop >= COMPOSER_INK_THRESHOLD).sum(axis=1)
    groups = _text_groups(rows)
    if not groups:
        return None
    card_bg = _card_background(crop, groups)
    candidates = [g for g in groups if _row_background(crop[g[0]]) != card_bg]
    if not candidates:
        # No code block in sight: fall back to the last text line.
        first, last = groups[-1]
        return crop[first:last + 1]
    first, last = candidates[-1]
    command_bg = _row_background(crop[first])
    # A command that wraps onto several lines: take the whole block.
    for earlier in reversed(candidates[:-1]):
        if _row_background(crop[earlier[0]]) == command_bg \
                and earlier[1] >= first - 30:
            first = earlier[0]
        else:
            break
    return crop[first:last + 1]


def band_text_pixels(band):
    if band is None:
        return None
    return band >= COMPOSER_INK_THRESHOLD


def _shift(mask, dy, dx):
    """Copy of `mask` moved by (dy, dx), without wrapping around."""
    out = np.zeros_like(mask)
    h, w = mask.shape
    y0, y1 = max(0, dy), min(h, h + dy)
    x0, x1 = max(0, dx), min(w, w + dx)
    if y0 >= y1 or x0 >= x1:
        return out
    out[y0:y1, x0:x1] = mask[y0 - dy:y1 - dy, x0 - dx:x1 - dx]
    return out


def band_similarity(a, b):
    """How much of the text of two bands has a counterpart in the other.

    A glyph that was re-rendered a pixel away still matches within a 3x3
    neighbourhood, so the score stays high; a changed character leaves a
    blob of unmatched pixels. Comparing the raw pixels instead would call a
    one-pixel re-render a new command, and the repeat counter would never
    reach the threshold. The bands are also aligned first: the command line
    moves with the transcript, and a displacement is not a new command.

    Bands of different size are compared on a common canvas — a command whose
    last line gained or lost a row (a descender) must still count as the same
    command.
    """
    ba, bb = band_text_pixels(a), band_text_pixels(b)
    if ba is None or bb is None:
        return 0.0
    if ba.shape != bb.shape:
        h = max(ba.shape[0], bb.shape[0])
        w = max(ba.shape[1], bb.shape[1])
        ba = np.pad(ba, ((0, h - ba.shape[0]), (0, w - ba.shape[1])))
        bb = np.pad(bb, ((0, h - bb.shape[0]), (0, w - bb.shape[1])))
    ink_a, ink_b = int(ba.sum()), int(bb.sum())
    if not ink_a and not ink_b:
        return 1.0
    if not ink_a or not ink_b:
        return 0.0
    dil_a = maximum_filter(ba.astype(np.uint8), size=3) > 0
    dil_b = maximum_filter(bb.astype(np.uint8), size=3) > 0
    best = 0.0
    for dy in range(-BAND_SHIFT, BAND_SHIFT + 1):
        for dx in range(-BAND_SHIFT, BAND_SHIFT + 1):
            shifted_b = _shift(bb, dy, dx)
            shifted_dil_b = _shift(dil_b, dy, dx)
            matched_a = int((ba & shifted_dil_b).sum()) / ink_a
            matched_b = int((shifted_b & dil_a).sum()) / ink_b
            best = max(best, min(matched_a, matched_b))
    return best


def same_command(a, b):
    """True when two command bands show the same command."""
    return band_similarity(a, b) >= COMMAND_SIMILARITY


def format_guide_message(template, repeats):
    """Substitute the repeat count without %-formatting.

    A custom message may contain a literal '%' ("100% of the time");
    %-formatting would raise and take the unattended loop down with it, so
    only the %d placeholder is replaced.
    """
    return template.replace("%d", str(repeats))


def client_size(vcli, session, window_id, display):
    """Client-area size of the window (the coordinate space of the actions)."""
    rc, out, err = run([
        vcli, "--session", session, "window", "geometry",
        "--window-id", window_id, "--display", display, "--format", "json",
    ])
    if rc != 0:
        return None
    try:
        geom = (json.loads(out) or {}).get("geometry") or {}
    except ValueError:
        return None
    w, h = geom.get("w"), geom.get("h")
    if not w or not h:
        return None
    return int(w), int(h)


def composer_crop(args, workdir):
    """Grayscale crop of the chat composer input area."""
    size = client_size(args.vcli, args.session, args.window_id, args.display)
    if not size:
        return None
    _, h = size
    try:
        shot = capture(args.vcli, args.session, args.window_id, args.display, workdir)
    except Exception:  # noqa: BLE001
        return None
    # The composer box is anchored to the bottom of the client area; this crop
    # covers its input line only, so the disclaimer above it and the model
    # selector below it cannot change it.
    return crop_region(shot, (0, max(0, h - 96), 400, 56))


def composer_ink(crop):
    """Number of text pixels in a composer crop."""
    if crop is None:
        return None
    return int((crop >= COMPOSER_INK_THRESHOLD).sum())


def try_send_guidance(args, message, workdir, ours_in_composer=False):
    """Type a guidance message into the chat composer and verify it landed.

    While a permission prompt is pending, the composer is an answer field for
    that prompt ("pick an option above") — writing there would answer the
    prompt instead of talking to the agent. Not every prompt renders the
    "Allow / Don't Allow" block this skill matches (the plan approval renders
    "Approve / Adjust / Reject"), so the composer itself is the guard: while
    a prompt is pending it shows the answer placeholder, and a composer that
    already holds text is never typed into. The composer is also disabled
    while the agent is busy, which is why the delivery is verified with a
    screenshot instead of assumed.

    Returns (delivered, typed): `typed` tells the caller that the message is
    still sitting in the composer, so the next attempt may clear it.
    """
    size = client_size(args.vcli, args.session, args.window_id, args.display)
    if not size:
        print("guidance skipped: window geometry unavailable")
        return False, False
    _, h = size

    def action(*cmd):
        rc, out, err = run([
            args.vcli, "--session", args.session, "window", "action-x11",
            "--window-id", args.window_id, "--display", args.display,
        ] + list(cmd) + ["--direct"])
        if rc != 0:
            print("guidance %s failed: %s %s" % (cmd[1], out.strip(), err.strip()))
        return rc == 0

    before = composer_crop(args, workdir)
    if before is None:
        print("guidance skipped: composer not visible")
        return False, False
    ink_before = composer_ink(before)
    if not ours_in_composer and ink_before > COMPOSER_INK_TOLERANCE:
        # The composer holds something: the answer placeholder of a pending
        # prompt, or text the user typed. Writing here would answer the prompt
        # instead of talking to the agent, or destroy the user's message.
        print("guidance skipped: the composer is not empty (a prompt may be pending)")
        return False, False

    landed = False
    ink_empty = ink_before
    # The input line sits a few pixels above the bottom of the client area and
    # its exact row depends on the composer layout, so a few candidates are
    # tried; each attempt clears the composer first, so a failed attempt
    # cannot leave a half-typed message behind.
    for offset in COMPOSER_CLICK_OFFSETS:
        y = max(0, h - offset)
        if not action("--operation", "click-rel", "--x", "30", "--y", str(y)):
            return False, False
        action("--operation", "key", "--text", "ctrl+a")
        action("--operation", "key", "--text", "BackSpace")
        time.sleep(0.3)
        # The composer is empty now: this is the baseline the typed text has
        # to exceed, no matter what was in the composer before.
        cleared = composer_crop(args, workdir)
        ink_empty = composer_ink(cleared)
        if ink_empty is None:
            ink_empty = 0
        if not action("--operation", "type", "--text", message):
            return False, False
        time.sleep(0.4)
        after = composer_crop(args, workdir)
        ink_after = composer_ink(after)
        if ink_after is not None and ink_after > ink_empty + COMPOSER_INK_TOLERANCE:
            landed = True
            break

    if not landed:
        # The composer did not accept the text — the agent is busy. Keep the
        # message and try again on the next poll.
        still = composer_crop(args, workdir)
        ink_still = composer_ink(still)
        if ink_still is not None and ink_still > COMPOSER_INK_TOLERANCE:
            # The text is in the composer but was not confirmed: the next
            # attempt may clear it and try again.
            return False, True
        return False, False

    # Submitting needs the input to have settled: a Return sent too early is
    # swallowed, so the emptied composer is verified and Return is retried.
    for _ in range(3):
        rc, out, err = run([
            args.vcli, "--session", args.session, "window", "action-x11",
            "--window-id", args.window_id, "--display", args.display,
            "--operation", "key", "--text", "Return", "--direct",
        ])
        if rc != 0:
            print("guidance Return failed: %s %s" % (out.strip(), err.strip()))
            return False, True
        time.sleep(1.0)
        submitted = composer_crop(args, workdir)
        ink_submitted = composer_ink(submitted)
        if ink_submitted is None or ink_submitted <= ink_empty + COMPOSER_INK_TOLERANCE:
            print("guided the agent: %s" % message)
            return True, False
    print("guidance typed but not submitted — keeping it for the next poll")
    return False, True


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
    ap.add_argument("--timeout", type=float, default=600.0,
                    help="overall wall-clock budget in seconds; 0 = run until stopped")
    ap.add_argument("--idle-polls", type=int, default=3,
                    help="stop after this many consecutive polls without a prompt; 0 = never stop on idle")
    ap.add_argument("--settle-polls", type=int, default=5,
                    help="polls to wait for the prompt to vanish after a click")
    ap.add_argument("--title-prefix", default="ViraStack",
                    help="window title prefix used to re-resolve the target after a restart")
    ap.add_argument("--guide-threshold", type=int, default=3,
                    help="guide the agent after this many consecutive approvals of the same command; 0 = never")
    ap.add_argument("--guide-message",
                    default="Auto-approve notice: the same command has been requested %d "
                            "times in a row. Please diagnose the root cause first (paste the "
                            "error output, check the dependencies and the paths) instead of "
                            "running the same command again.",
                    help="message typed into the chat when the repeat limit is hit; %%d is the "
                         "repeat count. ASCII only — the type action rejects other characters")
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

    deadline = time.time() + args.timeout if args.timeout else float("inf")
    clicks = 0
    idle = 0
    failed = 0
    capture_failures = 0
    last_band = None
    repeats = 0
    guidance_pending = None
    guidance_typed = False
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
            # No prompt pending: the agent is either idle or busy. If a
            # guidance message is queued, this is the moment to deliver it.
            if guidance_pending and not args.dry_run:
                delivered, typed = try_send_guidance(
                    args, guidance_pending, workdir, ours_in_composer=guidance_typed
                )
                guidance_typed = typed
                if delivered:
                    guidance_pending = None
            if idle_limit_reached(args.idle_polls, idle):
                print("no pending prompt after %d idle polls" % idle)
                return 0
        else:
            idle = 0
            x, y, score = found
            print("prompt detected (ncc=%.3f) -> Allow at (%d, %d)" % (score, x, y))
            # The same command being approved over and over means the agent is
            # stuck in a retry loop. Count the repeats and, once there are
            # too many, queue a message telling it to diagnose the cause.
            band = command_band(shot, found)
            if band is not None:
                if last_band is not None and same_command(last_band, band):
                    repeats += 1
                else:
                    last_band = band
                    repeats = 1
                if (args.guide_threshold and repeats >= args.guide_threshold
                        and repeats % args.guide_threshold == 0):
                    guidance_pending = format_guide_message(args.guide_message, repeats)
                    guidance_typed = False
                    print("same command %d times in a row — guidance queued" % repeats)
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
