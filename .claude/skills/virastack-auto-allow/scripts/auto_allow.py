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
# Slack allowed when the ink of the composer is compared with the ink of an
# empty one: the caret blinks and the descenders of the disclaimer above it
# bleed into the crop, so the two never match exactly. Whether the composer
# holds text is decided by the shape of the ink, not by this amount.
COMPOSER_INK_TOLERANCE = 40
# The rows at the top of the composer crop that hold the descenders of the
# disclaimer above the input line, and the shape of the blinking caret: a
# solid vertical line of at most two columns. Everything else in the crop is
# a glyph.
COMPOSER_DESCENDER_ROWS = 2
CARET_MAX_WIDTH = 2
CARET_MIN_HEIGHT = 6
# How much the composer may differ from the text the loop typed and still be
# taken for it: the ink has to match within this ratio and the glyphs within
# this similarity.
COMPOSER_INK_RATIO = 0.15
COMPOSER_SIMILARITY = 0.95
# Margin around the detected prompt block that the post-click guard compares.
GUARD_MARGIN = 20
# How often the post-click guard samples the prompt area. The card is
# re-rendered in place, so the change has to be caught while it is there.
GUARD_POLL_INTERVAL = 0.3
# Consecutive rejected clicks before the loop gives up.
MAX_CLICK_FAILURES = 3


def display_args(display):
    """The `--display` flag of a vcli call.

    Omitted when the display is empty: vcli then detects the DISPLAY of the
    Virtuoso process itself.  A hard-coded ":0" is wrong on a host whose
    Virtuoso runs on another display — the clicks would go nowhere.
    """
    return ["--display", display] if display else []


def run(cmd):
    proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return proc.returncode, proc.stdout.decode("utf-8", "replace"), proc.stderr.decode("utf-8", "replace")


def capture(vcli, session, window_id, display, workdir):
    rc, out, err = run([
        vcli, "--session", session, "window", "action-x11",
        "--window-id", window_id,
    ] + display_args(display) + [
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


def load_screenshot(path):
    """Grayscale pixels of a screenshot, or None when the file is unusable.

    A capture can leave a truncated file behind (the daemon was interrupted
    while the image was written). The loop has to survive that: an unreadable
    capture means "no prompt this time", not the end of the run.
    """
    try:
        return np.array(Image.open(path).convert("L"))
    except Exception:  # noqa: BLE001 — an unreadable capture is not fatal
        return None


def find_prompt(screenshot, template):
    """Return (x, y, score) of the Allow label, or None when the prompt is absent."""
    image = load_screenshot(screenshot)
    if image is None:
        return None
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
        "--window-id", window_id,
    ] + display_args(display) + [
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


def _band_width(image):
    """Width of the band crop.

    The command has to be cut off at the scrollbar, but not before it: a
    command that runs past the crop would lose its tail and two different
    commands would share a fingerprint.
    """
    return max(200, min(image.shape[1] - SCROLLBAR_MARGIN, COMMAND_BAND_MAX))


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
    image = load_screenshot(screenshot)
    if image is None:
        return None
    crop = crop_from(image, (
        0,
        max(0, y - COMMAND_REGION_ABOVE),
        _band_width(image),
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
        "--window-id", window_id,
    ] + display_args(display) + [
        "--format", "json",
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


def composer_holds_text(crop):
    """Whether the composer holds anything but the caret.

    An empty composer is not free of ink: the descenders of the disclaimer
    above the input line bleed into the top rows of the crop and the blinking
    caret is a vertical line of its own. Neither is text — but a character
    the user typed is, and it can be as small as a single narrow glyph. The
    ink is therefore judged by its shape and not by its amount: anything
    outside the descender rows that is not a narrow vertical line is text,
    so a one-character input is protected as well.
    """
    if crop is None:
        return False
    body = crop >= COMPOSER_INK_THRESHOLD
    body = body.copy()
    body[:COMPOSER_DESCENDER_ROWS, :] = False
    if not body.any():
        return False
    columns = np.nonzero(body.any(axis=0))[0]
    start = prev = int(columns[0])
    for column in columns[1:]:
        column = int(column)
        if column == prev + 1:
            prev = column
            continue
        if not _is_caret(body, start, prev):
            return True
        start = prev = column
    return not _is_caret(body, start, prev)


def _is_caret(mask, first, last):
    """Whether the ink in these columns is the blinking caret.

    The caret is a solid vertical line. A glyph is not: a lowercase "i" has
    a dot above its stem and leaves a gap, and a period is far too short.
    """
    if last - first + 1 > CARET_MAX_WIDTH:
        return False
    block = mask[:, first:last + 1]
    rows = np.nonzero(block.any(axis=1))[0]
    if len(rows) == 0:
        return False
    height = int(rows[-1]) - int(rows[0]) + 1
    if height < CARET_MIN_HEIGHT:
        return False
    # A gap in the middle means the ink is not one solid line.
    return height == len(rows)


def composer_is_ours(crop, snapshot):
    """Whether the composer still holds the text the loop typed.

    The loop only clears the composer when its own message is still there.
    When the user has typed over it, the pixels differ and the text belongs
    to somebody else.
    """
    if crop is None or snapshot is None:
        return False
    ink, ink_snapshot = composer_ink(crop), composer_ink(snapshot)
    if not ink or not ink_snapshot:
        return False
    if abs(ink - ink_snapshot) > COMPOSER_INK_RATIO * ink_snapshot:
        return False
    return band_similarity(crop, snapshot) >= COMPOSER_SIMILARITY


def try_send_guidance(args, message, workdir, ours_snapshot=None):
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

    Returns (delivered, snapshot): `snapshot` is the composer as it holds the
    message the loop typed. The next attempt compares the composer with that
    snapshot before it clears it, so text somebody else typed is never wiped.
    """
    size = client_size(args.vcli, args.session, args.window_id, args.display)
    if not size:
        print("guidance skipped: window geometry unavailable")
        return False, None
    _, h = size

    def action(*cmd):
        rc, out, err = run([
            args.vcli, "--session", args.session, "window", "action-x11",
            "--window-id", args.window_id,
        ] + display_args(args.display) + list(cmd) + ["--direct"])
        if rc != 0:
            print("guidance %s failed: %s %s" % (cmd[1], out.strip(), err.strip()))
        return rc == 0

    before = composer_crop(args, workdir)
    if before is None:
        print("guidance skipped: composer not visible")
        return False, None
    ink_before = composer_ink(before)
    if ours_snapshot is not None:
        if composer_holds_text(before) \
                and not composer_is_ours(before, ours_snapshot):
            # The loop left its message in the composer, but the text that is
            # there now is not the message the loop typed: the user has typed
            # over it. Clearing the composer would wipe their text.
            print("guidance skipped: the composer holds text that is not ours")
            return False, None
    elif composer_holds_text(before):
        # The composer holds something: the answer placeholder of a pending
        # prompt, or text the user typed. Writing here would answer the prompt
        # instead of talking to the agent, or destroy the user's message.
        print("guidance skipped: the composer is not empty (a prompt may be pending)")
        return False, None

    landed = False
    typed_crop = None
    ink_empty = ink_before
    # The input line sits a few pixels above the bottom of the client area and
    # its exact row depends on the composer layout, so a few candidates are
    # tried; each attempt clears the composer first, so a failed attempt
    # cannot leave a half-typed message behind.
    for offset in COMPOSER_CLICK_OFFSETS:
        y = max(0, h - offset)
        if not action("--operation", "click-rel", "--x", "30", "--y", str(y)):
            return False, None
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
            return False, None
        time.sleep(0.4)
        after = composer_crop(args, workdir)
        ink_after = composer_ink(after)
        if ink_after is not None and ink_after > ink_empty + COMPOSER_INK_TOLERANCE:
            landed = True
            typed_crop = after
            break

    if not landed:
        # The composer did not accept the text — the agent is busy. Keep the
        # message and try again on the next poll.
        still = composer_crop(args, workdir)
        if composer_holds_text(still):
            # The text is in the composer but was not confirmed: the next
            # attempt may clear it and try again.
            return False, still
        return False, None

    # Submitting needs the input to have settled: a Return sent too early is
    # swallowed, so the emptied composer is verified and Return is retried.
    for _ in range(3):
        rc, out, err = run([
            args.vcli, "--session", args.session, "window", "action-x11",
            "--window-id", args.window_id,
        ] + display_args(args.display) + [
            "--operation", "key", "--text", "Return", "--direct",
        ])
        if rc != 0:
            print("guidance Return failed: %s %s" % (out.strip(), err.strip()))
            return False, typed_crop
        time.sleep(1.0)
        submitted = composer_crop(args, workdir)
        ink_submitted = composer_ink(submitted)
        if ink_submitted is None or ink_submitted <= ink_empty + COMPOSER_INK_TOLERANCE:
            print("guided the agent: %s" % message)
            return True, None
    print("guidance typed but not submitted — keeping it for the next poll")
    return False, typed_crop


def wait_until_progress(args, workdir, before, region, polls):
    """Poll until the area around the prompt changes.

    The click is considered consumed when the pixels around the prompt block
    differ from the pre-click snapshot: the old prompt was dismissed (and a
    new one may already have taken its place). If the area stays identical,
    the click never landed on "Allow" — the caller stops instead of hammering
    the same coordinates.

    The client re-renders the permission card in place: the card is consumed
    and the next request appears at the very same place a second later. A
    check that waits several seconds before its first sample therefore sees
    the card back exactly as it was and mistakes a working click for a miss
    — it aborted the loop while the agent was simply asking again. So the
    area is sampled right away and several times a second: any sample that
    differs from the pre-click snapshot counts as progress.
    """
    deadline = time.time() + args.interval * polls
    while time.time() < deadline:
        try:
            shot = capture(args.vcli, args.session, args.window_id, args.display, workdir)
        except Exception:  # noqa: BLE001 — treat a capture failure as "no progress"
            time.sleep(GUARD_POLL_INTERVAL)
            continue
        current = crop_region(shot, region)
        if current is not None and not np.array_equal(current, before):
            return True
        time.sleep(GUARD_POLL_INTERVAL)
    return False


def crop_region(path, region):
    """Crop (x, y, w, h) out of a screenshot, clamped to the image.

    Clamping instead of bailing out matters: the region is a fixed 700x260
    block around the prompt, and a narrower window would otherwise make the
    post-click guard silently skip instead of checking anything.

    A truncated capture yields None instead of raising: the loop must survive
    a bad screenshot, not die on it.
    """
    image = load_screenshot(path)
    if image is None:
        return None
    return crop_from(image, region)


def crop_from(image, region):
    """Crop (x, y, w, h) out of a loaded screenshot, clamped to the image."""
    x, y, w, h = region
    height, width = image.shape[:2]
    left = max(0, min(x, width))
    top = max(0, min(y, height))
    right = max(left, min(x + w, width))
    bottom = max(top, min(y + h, height))
    if right <= left or bottom <= top:
        return None
    return image[top:bottom, left:right]


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
    """Window id and DISPLAY of the first window whose title starts with `title_prefix`.

    The DISPLAY comes out of the same answer: `list-windows-x11` reports the
    display it queried, and that is the display the action calls have to
    use.  A hard-coded ":0" is wrong on a host whose Virtuoso runs on
    another display.
    """
    rc, out, err = run([
        vcli, "--session", session, "window", "list-windows-x11", "--format", "json",
    ])
    if rc != 0:
        return None, None
    try:
        data = json.loads(out)
    except ValueError:
        return None, None
    display = data.get("display")
    for w in data.get("windows") or []:
        if (w.get("title") or "").startswith(title_prefix):
            return w.get("window_id"), display or w.get("display")
    return None, display


def recover_target(args):
    """Re-resolve session, window id and DISPLAY after a Virtuoso or daemon restart.

    A restart invalidates both the daemon session id and the X11 window id.
    Without this the loop keeps failing to capture forever and silently
    stops approving.  The DISPLAY is re-resolved too: Virtuoso can come back
    on another display.  An explicitly passed `--display` always wins.
    """
    session = resolve_session(args.vcli, args.session)
    window_id, display = resolve_window(args.vcli, session, args.title_prefix)
    if not window_id:
        return False
    changed = session != args.session or window_id != args.window_id
    args.session = session
    args.window_id = window_id
    if display and not getattr(args, "display_explicit", False):
        changed = changed or args.display != display
        args.display = display
    if not changed:
        return False
    print("re-resolved target: session=%s window=%s display=%s"
          % (session, window_id, args.display))
    return True


def guard_region(found, template, screenshot_size):
    """The region of the screenshot that holds the permission prompt block.

    Derived from where the prompt was detected and how big the template is:
    the client renders the prompt wherever it fits, and a fixed block at the
    left of the window compared unrelated pixels when the prompt rendered
    further right — a false "unchanged" abort, or a false pass when other
    content moved instead.

    The region is clamped to the screenshot: a block that runs past the edge
    compares nothing useful.
    """
    x, y, _ = found
    th, tw = template.shape[0], template.shape[1]
    sw, sh = screenshot_size
    left = max(0, min(x - ALLOW_OFFSET[0] - GUARD_MARGIN, max(0, sw - 1)))
    top = max(0, min(y - ALLOW_OFFSET[1] - GUARD_MARGIN, max(0, sh - 1)))
    width = max(1, min(tw + 2 * GUARD_MARGIN, sw - left))
    height = max(1, min(th + 2 * GUARD_MARGIN, sh - top))
    return (left, top, width, height)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--window-id", default="",
                    help="X11 window id of the ViraStack client; resolved from the title when omitted")
    ap.add_argument("--display", default="",
                    help="X display of the ViraStack client; detected from the Virtuoso "
                         "process when omitted")
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

    # An explicitly passed --display always wins; a detected one may change
    # after a restart and is re-resolved then.
    args.display_explicit = bool(args.display)

    if not args.session:
        args.session = resolve_session(args.vcli, "")
        if not args.session:
            print("no live vcli session — start virtuoso-daemon first")
            return 2
        print("resolved session=%s" % args.session)

    if not args.window_id or not args.display:
        # The same answer carries the DISPLAY the Virtuoso runs on: the
        # action calls have to name it, and ":0" is wrong on a host whose
        # Virtuoso runs on another display.
        window_id, display = resolve_window(args.vcli, args.session, args.title_prefix)
        if not args.window_id:
            if not window_id:
                print("no window titled %r — is ViraStack running?" % args.title_prefix)
                return 2
            args.window_id = window_id
            print("resolved window=%s" % args.window_id)
        if not args.display and display:
            args.display = display
            print("resolved display=%s" % args.display)
    if not args.display:
        print("could not determine the DISPLAY of the ViraStack window — "
              "pass --display")
        return 2

    deadline = time.time() + args.timeout if args.timeout else float("inf")
    clicks = 0
    idle = 0
    failed = 0
    capture_failures = 0
    last_band = None
    repeats = 0
    guidance_pending = None
    guidance_snapshot = None
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
                delivered, snapshot = try_send_guidance(
                    args, guidance_pending, workdir, ours_snapshot=guidance_snapshot
                )
                guidance_snapshot = snapshot
                if delivered:
                    guidance_pending = None
                    guidance_snapshot = None
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
                    guidance_snapshot = None
                    print("same command %d times in a row — guidance queued" % repeats)
            if click(args.vcli, args.session, args.window_id, args.display, x, y, args.dry_run):
                clicks += 1
                progressed = True
                # The click must have consumed the prompt. Compare the area
                # around the prompt block with the pre-click snapshot: if it
                # never changes, the click did not land on "Allow" and we stop
                # instead of hammering the same coordinates. A dry run never
                # clicks, so there is nothing to verify.
                if not args.dry_run:
                    # The guard has to look at the prompt block itself: it is
                    # rendered wherever the client puts it, and a fixed block
                    # would compare unrelated pixels.
                    image = load_screenshot(shot)
                    if image is not None:
                        region = guard_region(found, template,
                                              (image.shape[1], image.shape[0]))
                        before = crop_region(shot, region)
                        if before is not None and not wait_until_progress(
                            args, workdir, before, region, polls=args.settle_polls
                        ):
                            progressed = False
                            print("prompt area unchanged after the click — "
                                  "the click may have missed")
                if progressed:
                    failed = 0
                else:
                    # The card is re-rendered in place, so the change can be
                    # over before the first sample catches it. One such miss
                    # is not proof that the click went nowhere: the click is
                    # retried, and only a run of them aborts the loop.
                    failed += 1
                    if failed >= MAX_CLICK_FAILURES:
                        print("%d consecutive clicks that left the prompt "
                              "unchanged — aborting" % failed)
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
