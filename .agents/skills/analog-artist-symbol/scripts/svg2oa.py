#!/usr/bin/env python3
"""svg2oa — turn an artist-style SVG symbol into native Cadence OA geometry.

Reads an SVG (the drawing produced by a "functional symbol" workflow) and emits
a SKILL script that recreates the same picture inside a Virtuoso schematicSymbol
cellview: wires as dbCreatePath, filled shapes as dbCreatePolygon/dbCreateRect,
text as dbCreateLabel, external ports as real symbol pins
(schCreateSymbolPin), pin names glued to pins with schGlueLabel, dynamic
[@instanceName]/[@partName] labels, and an instance/drawing selection box.

Only the Python standard library is used.

Coordinate model
----------------
SVG y grows downward, OA y grows upward, so the viewBox is flipped and scaled:
    x_oa = origin_x + (x_svg - vb_x) * scale
    y_oa = origin_y + (vb_y + vb_h - y_svg) * scale
The drawing's lower-left corner lands on the symbol origin, which --origin-x
and --origin-y move. The offset places the drawing on a pin frame that already
exists in the target library, so the pins keep the coordinates a parent
schematic is already wired to.

Widths snap to what the technology accepts: anything below --min-width is
raised to it, because Virtuoso rejects path widths outside the tech snap table.

Usage
-----
    svg2oa.py symbol.svg --lib myLib --cell myCell --view symbol -o symbol.il
    svg2oa.py symbol.svg --lib myLib --cell myCell --view symbol --pins --pin-json ports.json
"""
from __future__ import annotations

import argparse
import json
import math
import re
import sys
import xml.etree.ElementTree as ET

SVG_NS = "http://www.w3.org/2000/svg"

# Font heights Virtuoso accepts for schematic text without complaining.
LEGAL_TEXT_HEIGHTS = (0.03125, 0.0375, 0.05, 0.0625, 0.075, 0.09375, 0.125, 0.1875)


# --------------------------------------------------------------------------- css


def parse_css(text: str) -> list[tuple[list[str], dict]]:
    """Parse a <style> block into [(selectors, {prop: value})]."""
    rules = []
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    for block in text.split("}"):
        if "{" not in block:
            continue
        sel, _, body = block.partition("{")
        decls = {}
        for decl in body.split(";"):
            if ":" not in decl:
                continue
            prop, _, value = decl.partition(":")
            decls[prop.strip().lower()] = value.strip()
        selectors = [s.strip() for s in sel.split(",") if s.strip()]
        if selectors and decls:
            rules.append((selectors, decls))
    return rules


def selector_matches(selector: str, tag: str, classes: list[str]) -> bool:
    sel = selector.strip()
    if sel.startswith("."):
        return sel[1:] in classes
    if sel.startswith("#"):
        return False
    if sel == "*":
        return True
    return sel.lower() == tag.lower()


def resolve_style(el, css_rules, inherited):
    tag = el.tag.split("}")[-1]
    classes = (el.get("class") or "").split()
    style = dict(inherited)

    for selectors, decls in css_rules:
        for sel in selectors:
            if selector_matches(sel, tag, classes):
                style.update(decls)

    for key in ("fill", "stroke", "stroke-width", "stroke-linecap",
                "stroke-linejoin", "font-size", "font-family", "text-anchor",
                "opacity", "fill-opacity", "stroke-opacity", "display", "visibility"):
        val = el.get(key)
        if val is not None:
            style[key] = val

    inline = el.get("style")
    if inline:
        for decl in inline.split(";"):
            if ":" in decl:
                prop, _, value = decl.partition(":")
                style[prop.strip().lower()] = value.strip()
    return style


# -------------------------------------------------------------------- transform


def parse_transform(text):
    """Return an affine matrix (a, b, c, d, e, f) for a transform attribute."""
    matrix = (1.0, 0.0, 0.0, 1.0, 0.0, 0.0)
    if not text:
        return matrix
    for name, args in re.findall(r"(\w+)\s*\(([^)]*)\)", text):
        nums = [float(n) for n in re.findall(r"-?\d*\.?\d+(?:e[-+]?\d+)?", args)]
        if name == "translate":
            tx = nums[0]
            ty = nums[1] if len(nums) > 1 else 0.0
            matrix = multiply(matrix, (1, 0, 0, 1, tx, ty))
        elif name == "scale":
            sx = nums[0]
            sy = nums[1] if len(nums) > 1 else sx
            matrix = multiply(matrix, (sx, 0, 0, sy, 0, 0))
        elif name == "rotate":
            ang = math.radians(nums[0])
            cx = nums[1] if len(nums) > 2 else 0.0
            cy = nums[2] if len(nums) > 2 else 0.0
            ca, sa = math.cos(ang), math.sin(ang)
            matrix = multiply(matrix, (ca, sa, -sa, ca,
                                       cx - ca * cx + sa * cy,
                                       cy - sa * cx - ca * cy))
        elif name == "matrix" and len(nums) >= 6:
            matrix = multiply(matrix, tuple(nums[:6]))
    return matrix


def multiply(m1, m2):
    a1, b1, c1, d1, e1, f1 = m1
    a2, b2, c2, d2, e2, f2 = m2
    return (a1 * a2 + c1 * b2,
            b1 * a2 + d1 * b2,
            a1 * c2 + c1 * d2,
            b1 * c2 + d1 * d2,
            a1 * e2 + c1 * f2 + e1,
            b1 * e2 + d1 * f2 + f1)


def apply_point(m, x, y):
    a, b, c, d, e, f = m
    return (a * x + c * y + e, b * x + d * y + f)


# ------------------------------------------------------------------ path parser


def parse_path_data(d: str) -> list[list[tuple[float, float]]]:
    """Flatten an SVG path into subpaths of points."""
    tokens = re.findall(r"[MmLlHhVvCcSsQqTtAaZz]|[-+]?\d*\.?\d+(?:e[-+]?\d+)?", d)
    subpaths: list[list[tuple[float, float]]] = []
    current: list[tuple[float, float]] = []
    x = y = 0.0
    start = (0.0, 0.0)
    prev_ctrl = None
    cmd = None
    i = 0
    while i < len(tokens):
        tok = tokens[i]
        if re.match(r"[A-Za-z]", tok):
            cmd = tok
            i += 1
            if cmd in "Zz":
                if current:
                    current.append(start)
                    subpaths.append(current)
                    current = []
                x, y = start
                prev_ctrl = None
                continue
        nums = []
        need = {"M": 2, "L": 2, "H": 1, "V": 1, "C": 6, "S": 4, "Q": 4, "T": 2, "A": 7}.get(cmd.upper(), 0)
        while len(nums) < need and i < len(tokens) and not re.match(r"[A-Za-z]", tokens[i]):
            nums.append(float(tokens[i]))
            i += 1
        if not nums:
            if cmd is not None and cmd.upper() in "Mm":
                i += 1
            continue
        rel = cmd.islower()
        if cmd.upper() == "M":
            nx, ny = nums[0], nums[1]
            if rel:
                nx, ny = x + nx, y + ny
            if current:
                subpaths.append(current)
            current = [(nx, ny)]
            start = (nx, ny)
            x, y = nx, ny
            cmd = "l" if rel else "L"
            prev_ctrl = None
        elif cmd.upper() == "L":
            nx, ny = nums[0], nums[1]
            if rel:
                nx, ny = x + nx, y + ny
            current.append((nx, ny))
            x, y = nx, ny
            prev_ctrl = None
        elif cmd.upper() == "H":
            nx = x + nums[0] if rel else nums[0]
            current.append((nx, y))
            x = nx
            prev_ctrl = None
        elif cmd.upper() == "V":
            ny = y + nums[0] if rel else nums[0]
            current.append((x, ny))
            y = ny
            prev_ctrl = None
        elif cmd.upper() == "C":
            p = list(zip(nums[0::2], nums[1::2]))
            c1 = (x + p[0][0], y + p[0][1]) if rel else p[0]
            c2 = (x + p[1][0], y + p[1][1]) if rel else p[1]
            end = (x + p[2][0], y + p[2][1]) if rel else p[2]
            current.extend(flatten_cubic((x, y), c1, c2, end))
            prev_ctrl = c2
            x, y = end
        elif cmd.upper() == "S":
            p = list(zip(nums[0::2], nums[1::2]))
            c1 = (2 * x - prev_ctrl[0], 2 * y - prev_ctrl[1]) if prev_ctrl else (x, y)
            c2 = (x + p[0][0], y + p[0][1]) if rel else p[0]
            end = (x + p[1][0], y + p[1][1]) if rel else p[1]
            current.extend(flatten_cubic((x, y), c1, c2, end))
            prev_ctrl = c2
            x, y = end
        elif cmd.upper() == "Q":
            p = list(zip(nums[0::2], nums[1::2]))
            q = (x + p[0][0], y + p[0][1]) if rel else p[0]
            end = (x + p[1][0], y + p[1][1]) if rel else p[1]
            current.extend(flatten_quad((x, y), q, end))
            prev_ctrl = q
            x, y = end
        elif cmd.upper() == "T":
            end = (x + nums[0], y + nums[1]) if rel else (nums[0], nums[1])
            q = (2 * x - prev_ctrl[0], 2 * y - prev_ctrl[1]) if prev_ctrl else (x, y)
            current.extend(flatten_quad((x, y), q, end))
            prev_ctrl = q
            x, y = end
        elif cmd.upper() == "A":
            end = (x + nums[5], y + nums[6]) if rel else (nums[5], nums[6])
            current.append(end)
            x, y = end
            prev_ctrl = None
    if current:
        subpaths.append(current)
    return subpaths


def flatten_cubic(p0, p1, p2, p3, steps=12):
    pts = []
    for k in range(1, steps + 1):
        t = k / steps
        mt = 1 - t
        bx = (mt ** 3 * p0[0] + 3 * mt * mt * t * p1[0]
              + 3 * mt * t * t * p2[0] + t ** 3 * p3[0])
        by = (mt ** 3 * p0[1] + 3 * mt * mt * t * p1[1]
              + 3 * mt * t * t * p2[1] + t ** 3 * p3[1])
        pts.append((bx, by))
    return pts


def flatten_quad(p0, p1, p2, steps=10):
    pts = []
    for k in range(1, steps + 1):
        t = k / steps
        mt = 1 - t
        bx = mt * mt * p0[0] + 2 * mt * t * p1[0] + t * t * p2[0]
        by = mt * mt * p0[1] + 2 * mt * t * p1[1] + t * t * p2[1]
        pts.append((bx, by))
    return pts


# --------------------------------------------------------------------- extractor


def raw_points(el) -> list[tuple[float, float]]:
    """Geometry of a single SVG shape element, in its own coordinates."""
    tag = el.tag.split("}")[-1]
    if tag == "rect":
        x = float(el.get("x", 0))
        y = float(el.get("y", 0))
        w = float(el.get("width", 0))
        h = float(el.get("height", 0))
        return [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
    if tag == "circle":
        cx = float(el.get("cx", 0))
        cy = float(el.get("cy", 0))
        r = float(el.get("r", 0))
        return [(cx - r, cy - r), (cx + r, cy + r)]
    if tag == "line":
        return [(float(el.get("x1", 0)), float(el.get("y1", 0))),
                (float(el.get("x2", 0)), float(el.get("y2", 0)))]
    if tag in ("polyline", "polygon"):
        nums = [float(n) for n in re.findall(r"-?\d*\.?\d+(?:e[-+]?\d+)?",
                                             el.get("points", ""))]
        return list(zip(nums[0::2], nums[1::2]))
    if tag == "path":
        subs = parse_path_data(el.get("d", ""))
        return subs[0] if subs else []
    return []


class Extractor:
    def __init__(self, css_rules, skip_port_labels=False):
        self.css = css_rules
        self.items = []
        self.ports = []
        self.skip_port_labels = skip_port_labels

    def record_port(self, el, transform, group_style):
        """A port is a <g data-port=...> group holding the pin square and its label."""
        name = el.get("data-port")
        side = el.get("data-side", "left")
        direction = el.get("data-direction", "inputOutput")
        px = py = None
        label_x = label_y = None
        label_transformed = False
        label_just = "centerLeft"
        label_orient = "R0"
        for child in el:
            tag = child.tag.split("}")[-1]
            if tag == "rect" and px is None:
                x = float(child.get("x", 0))
                y = float(child.get("y", 0))
                w = float(child.get("width", 0))
                h = float(child.get("height", 0))
                px, py = x + w / 2, y + h / 2
            elif tag == "circle" and px is None:
                px = float(child.get("cx", 0))
                py = float(child.get("cy", 0))
            elif tag in ("line", "polyline", "polygon", "path") and px is None:
                pts = raw_points(child)
                if pts:
                    xs = [p[0] for p in pts]
                    ys = [p[1] for p in pts]
                    px, py = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2
            elif tag == "text" and label_x is None:
                text_style = resolve_style(child, self.css, group_style)
                anchor = text_style.get("text-anchor", "start")
                label_just = {"start": "centerLeft", "middle": "centerCenter",
                              "end": "centerRight"}.get(anchor, "centerLeft")
                rot = re.search(r"rotate\(\s*(-?\d+(?:\.\d+)?)",
                                child.get("transform") or "")
                if rot:
                    deg = float(rot.group(1)) % 360.0
                    if abs(deg - 90) < 1 or abs(deg - 270) < 1:
                        label_orient = "R270" if abs(deg - 270) < 1 else "R90"
                    elif abs(deg - 180) < 1:
                        label_orient = "R180"
                local = multiply(transform, parse_transform(child.get("transform")))
                label_x, label_y = apply_point(local, float(child.get("x", 0)),
                                               float(child.get("y", 0)))
                label_transformed = True
        if px is None:
            return
        if label_x is None:
            label_x, label_y = px, py
            label_just = "centerRight" if side == "right" else "centerLeft"
        gx, gy = apply_point(transform, px, py)
        if label_transformed:
            lx, ly = label_x, label_y
        else:
            lx, ly = apply_point(transform, label_x, label_y)
        self.ports.append({"name": name, "side": side, "direction": direction,
                           "svg_x": gx, "svg_y": gy,
                           "label_svg_x": lx, "label_svg_y": ly,
                           "label_just": label_just,
                           "label_orient": label_orient})

    def walk(self, el, transform, style, inherited_style, in_port=False):
        tag = el.tag.split("}")[-1]
        if tag in ("defs", "title", "desc", "metadata", "style"):
            return
        if style.get("display") == "none" or style.get("visibility") == "hidden":
            return
        transform = multiply(transform, parse_transform(el.get("transform")))
        style = resolve_style(el, self.css, inherited_style)

        if tag == "g":
            is_port = bool(el.get("data-port"))
            if is_port:
                self.record_port(el, transform, style)
            for child in el:
                self.walk(child, transform, style, style, in_port or is_port)
            return
        if tag == "svg":
            for child in el:
                self.walk(child, transform, style, style, in_port)
            return

        if tag == "line":
            pts = [(float(el.get("x1", 0)), float(el.get("y1", 0))),
                   (float(el.get("x2", 0)), float(el.get("y2", 0)))]
            self.items.append({"kind": "path", "points": pts, "style": style,
                               "data": dict(el.attrib), "transform": transform})
        elif tag == "polyline" or tag == "polygon":
            nums = [float(n) for n in re.findall(r"-?\d*\.?\d+(?:e[-+]?\d+)?",
                                                 el.get("points", ""))]
            pts = list(zip(nums[0::2], nums[1::2]))
            if tag == "polygon" and pts:
                pts = pts + [pts[0]]
            self.items.append({"kind": "path", "points": pts, "style": style,
                               "data": dict(el.attrib), "transform": transform})
        elif tag == "path":
            subs = parse_path_data(el.get("d", ""))
            for pts in subs:
                self.items.append({"kind": "path", "points": pts, "style": style,
                                   "data": dict(el.attrib), "transform": transform})
        elif tag == "rect":
            x = float(el.get("x", 0))
            y = float(el.get("y", 0))
            w = float(el.get("width", 0))
            h = float(el.get("height", 0))
            pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)]
            self.items.append({"kind": "path", "points": pts, "style": style,
                               "data": dict(el.attrib), "closed": True,
                               "transform": transform})
        elif tag == "circle":
            cx = float(el.get("cx", 0))
            cy = float(el.get("cy", 0))
            r = float(el.get("r", 0))
            pts = [(cx + r * math.cos(t * math.pi / 18), cy + r * math.sin(t * math.pi / 18))
                   for t in range(37)]
            self.items.append({"kind": "path", "points": pts, "style": style,
                               "data": dict(el.attrib), "closed": True,
                               "transform": transform})
        elif tag == "ellipse":
            cx = float(el.get("cx", 0))
            cy = float(el.get("cy", 0))
            rx = float(el.get("rx", 0))
            ry = float(el.get("ry", 0))
            pts = [(cx + rx * math.cos(t * math.pi / 18), cy + ry * math.sin(t * math.pi / 18))
                   for t in range(37)]
            self.items.append({"kind": "path", "points": pts, "style": style,
                               "data": dict(el.attrib), "closed": True,
                               "transform": transform})
        elif tag == "text":
            if in_port and self.skip_port_labels:
                return
            text = "".join(el.itertext()).strip()
            self.items.append({
                "kind": "text",
                "text": text,
                "x": float(el.get("x", 0)),
                "y": float(el.get("y", 0)),
                "style": style,
                "data": dict(el.attrib),
                "transform": transform,
            })
        elif tag in ("use", "image"):
            return


# ------------------------------------------------------------------ skill emitter


def fmt_num(v: float) -> str:
    return f"{v:.5f}".rstrip("0").rstrip(".") if v else "0"


def skill_quote(s) -> str:
    return '"' + str(s).replace("\\", "\\\\").replace('"', '\\"') + '"'


def snap_width(width: float, min_width: float) -> float:
    if width < min_width:
        return min_width
    return round(width / min_width) * min_width


def snap_text_height(height: float) -> float:
    """Snap to the legal OA text height closest to the requested one.

    Rounding up loses the size difference between two label sizes: a 13px
    label (0.0508) would become 0.0625, the same as a 16px one, and the
    drawing would show both at the same size. The nearest legal height keeps
    the artist's relative sizes; the smallest legal height still acts as the
    floor for anything below it.
    """
    return min(LEGAL_TEXT_HEIGHTS, key=lambda legal: (abs(legal - height), -legal))


def color_to_oa(style: dict, key: str) -> str:
    """Map an SVG colour to an OA purpose choice (OA symbol views are 2-tone)."""
    return style.get(key, "none")


class Emitter:
    def __init__(self, scale, vb, min_width, text_height, font="stick",
                 color_map=None, skip_background=True,
                 origin_x=0.0, origin_y=0.0):
        self.scale = scale
        self.vb = vb
        self.min_width = min_width
        self.text_height = text_height
        self.font = font
        self.color_map = color_map or {}
        self.skip_background = skip_background
        self.origin_x = origin_x
        self.origin_y = origin_y
        self.lines: list[str] = []
        self.ports: list[dict] = []
        self.counts = {"path": 0, "polygon": 0, "label": 0, "pin": 0, "skipped": 0}
        self.default_lpp = 'list("wire" "drawing")'
        self.pin_lpp = 'list("pin" "drawing")'

    def to_oa(self, x, y):
        """Map an SVG point to OA coordinates.

        The SVG canvas' lower-left corner lands on the symbol origin, which is
        offset by --origin-x/--origin-y. The offset lets the drawing be placed
        on a frame that already exists in the target library, so the pins keep
        the coordinates the parent schematic is wired to.
        """
        vx, vy, vw, vh = self.vb
        return (self.origin_x + (x - vx) * self.scale,
                self.origin_y + (vy + vh - y) * self.scale)

    def lpp_for(self, color: str) -> str:
        """Map an ink colour to an OA layer/purpose pair.

        A symbol view is drawn in layers, not colours: the artist palette is
        two-tone (green graphics, red ports), so red ink goes to the pin layer
        and everything else to the wire layer.
        """
        if not color:
            return self.default_lpp
        key = color.strip().lower()
        if key in self.color_map:
            return self.color_map[key]
        hexval = re.sub(r"#", "", key)
        if len(hexval) == 3:
            hexval = "".join(c * 2 for c in hexval)
        if len(hexval) == 6:
            r = int(hexval[0:2], 16)
            g = int(hexval[2:4], 16)
            b = int(hexval[4:6], 16)
            if r > 120 and r > g * 1.4 and r > b * 1.4:
                return self.pin_lpp
        return self.default_lpp

    def is_ink(self, color: str) -> bool:
        """True when the colour is real ink (not none/white/transparent)."""
        if not color:
            return False
        key = color.strip().lower()
        if key in ("none", "transparent", "white", "#ffffff", "#fff"):
            return False
        return True

    def emit(self, item, transform):
        style = item["style"]
        stroke = style.get("stroke", "none")
        fill = style.get("fill", "none")
        opacity = style.get("opacity", "1")
        if opacity not in ("1", "1.0", ""):
            self.counts["skipped"] += 1
            return
        if stroke in ("none", None) and fill in ("none", None):
            self.counts["skipped"] += 1
            return

        if item["kind"] == "text":
            self.emit_text(item, transform)
            return

        pts = [apply_point(transform, px, py) for px, py in item["points"]]
        oa = [self.to_oa(px, py) for px, py in pts]
        # drop duplicate consecutive points
        clean = []
        for p in oa:
            if not clean or abs(p[0] - clean[-1][0]) > 1e-9 or abs(p[1] - clean[-1][1]) > 1e-9:
                clean.append(p)
        if len(clean) < 2:
            self.counts["skipped"] += 1
            return

        if self.skip_background and self.is_background(pts):
            self.counts["skipped"] += 1
            return

        stroke_ink = self.is_ink(stroke)
        fill_ink = self.is_ink(fill)
        ink_color = stroke if stroke_ink else fill
        lpp = self.lpp_for(ink_color)

        if fill_ink:
            self.emit_filled(clean, lpp)
        if stroke_ink:
            width = snap_width(self.scale * float(style.get("stroke-width", 1) or 1),
                               self.min_width)
            self.counts["path"] += 1
            pts_skill = "list(" + " ".join(f"list({fmt_num(p[0])} {fmt_num(p[1])})"
                                            for p in clean) + ")"
            self.lines.append(
                f'  dbCreatePath(cv {lpp} {pts_skill} {fmt_num(width)})')

    def is_background(self, pts) -> bool:
        """A shape covering (almost) the whole canvas is the SVG background."""
        vx, vy, vw, vh = self.vb
        xs = [p[0] for p in pts]
        ys = [p[1] for p in pts]
        w = max(xs) - min(xs)
        h = max(ys) - min(ys)
        return w >= vw * 0.95 and h >= vh * 0.95

    def emit_filled(self, pts, lpp):
        self.counts["polygon"] += 1
        pts_skill = "list(" + " ".join(f"list({fmt_num(p[0])} {fmt_num(p[1])})"
                                        for p in pts) + ")"
        self.lines.append(f'  dbCreatePolygon(cv {lpp} {pts_skill})')

    def emit_text(self, item, transform):
        style = item["style"]
        if style.get("fill", "black") in ("none", None) and \
                style.get("stroke", "none") in ("none", None):
            self.counts["skipped"] += 1
            return
        x, y = apply_point(transform, item["x"], item["y"])
        ox, oy = self.to_oa(x, y)
        height = self.text_height
        if style.get("font-size"):
            height = snap_text_height(self.scale * float(re.sub(r"[^\d.]", "", style["font-size"]) or 16))
        anchor = style.get("text-anchor", "start")
        just = {"start": "lowerLeft", "middle": "lowerCenter",
                "end": "lowerRight"}.get(anchor, "lowerLeft")
        text = item["text"].replace("\\", "\\\\").replace('"', '\\"')
        lpp = self.lpp_for(style.get("fill", "black"))
        self.counts["label"] += 1
        self.lines.append(
            f'  dbCreateLabel(cv {lpp} list({fmt_num(ox)} {fmt_num(oy)}) '
            f'"{text}" "{just}" "R0" "{self.font}" {fmt_num(height)})')


# ------------------------------------------------------------------------- main


def parse_viewbox(root):
    vb = root.get("viewBox")
    if vb:
        nums = [float(n) for n in re.findall(r"-?\d*\.?\d+(?:e[-+]?\d+)?", vb)]
        if len(nums) == 4:
            return (nums[0], nums[1], nums[2], nums[3])
    width = float(re.sub(r"[^\d.]", "", root.get("width", "100")) or 100)
    height = float(re.sub(r"[^\d.]", "", root.get("height", "100")) or 100)
    return (0.0, 0.0, width, height)


def build(args):
    tree = ET.parse(args.svg)
    root = tree.getroot()
    css_rules = []
    metadata = {}
    for style_el in root.iter():
        local_tag = style_el.tag.split("}")[-1]
        if local_tag == "style" and style_el.text:
            css_rules.extend(parse_css(style_el.text))
        if local_tag == "metadata" and style_el.text:
            try:
                metadata = json.loads(style_el.text)
            except (ValueError, TypeError):
                metadata = {}
    vb = parse_viewbox(root)
    ex = Extractor(css_rules, skip_port_labels=args.pins and not args.no_native_labels)
    base_style = {"fill": "none", "stroke": "none", "stroke-width": "1"}
    ex.walk(root, (1, 0, 0, 1, 0, 0), base_style, base_style)

    em = Emitter(args.scale, vb, args.min_width, args.text_height,
                 origin_x=args.origin_x, origin_y=args.origin_y)
    for item in ex.items:
        em.emit(item, item.get("transform", (1, 0, 0, 1, 0, 0)))

    ports = []
    for p in ex.ports:
        ox, oy = em.to_oa(p["svg_x"], p["svg_y"])
        lx, ly = em.to_oa(p["label_svg_x"], p["label_svg_y"])
        ports.append({"name": p["name"], "side": p["side"],
                      "direction": p["direction"], "x": ox, "y": oy,
                      "svg_x": p["svg_x"], "svg_y": p["svg_y"],
                      "label_x": lx, "label_y": ly,
                      "label_just": p["label_just"],
                      "label_orient": p["label_orient"]})

    return em, ports, metadata


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("svg")
    ap.add_argument("--lib", required=True)
    ap.add_argument("--cell", required=True)
    ap.add_argument("--view", default="symbol")
    ap.add_argument("-o", "--out", help="SKILL script to write (default: stdout)")
    ap.add_argument("--scale", type=float, default=1.0 / 256.0,
                    help="OA units per SVG unit (default 1/256)")
    ap.add_argument("--min-width", type=float, default=0.01,
                    help="smallest legal path width in OA units (default 0.01)")
    ap.add_argument("--text-height", type=float, default=0.0625,
                    help="default label height in OA units (default 0.0625)")
    ap.add_argument("--origin-x", type=float, default=0.0,
                    help="OA x coordinate of the drawing's lower-left corner "
                         "(default 0)")
    ap.add_argument("--origin-y", type=float, default=0.0,
                    help="OA y coordinate of the drawing's lower-left corner "
                         "(default 0)")
    ap.add_argument("--font", default="stick")
    ap.add_argument("--pins", action="store_true",
                    help="emit schCreateSymbolPin calls for the data-port markers")
    ap.add_argument("--pin-order",
                    help="comma-separated terminal order; overrides SVG metadata")
    ap.add_argument("--no-native-labels", action="store_true",
                    help="keep SVG text labels and omit native NLPLabel/pin labels")
    ap.add_argument("--keep", action="store_true",
                    help="keep existing shapes instead of clearing the view first")
    ap.add_argument("--pin-json", help="write the resolved port table to this JSON file")
    args = ap.parse_args()

    em, ports, metadata = build(args)
    order = None
    if args.pin_order:
        order = [s.strip() for s in args.pin_order.split(",") if s.strip()]
    elif isinstance(metadata.get("terminal_order"), list):
        order = [str(s) for s in metadata["terminal_order"]]
    if order:
        names = {p["name"] for p in ports}
        missing = [n for n in order if n not in names]
        extra = [n for n in names if n not in order]
        if missing or extra:
            sys.exit(f"pin order mismatch: missing={missing} extra={extra}")
        ports.sort(key=lambda p: order.index(p["name"]))

    vx, vy, vw, vh = em.vb
    width_oa, height_oa = vw * args.scale, vh * args.scale
    ox, oy = args.origin_x, args.origin_y
    lines = []
    lines.append("; generated by svg2oa.py — do not edit by hand")
    lines.append(f'; source: {args.svg}')
    lines.append(f'; scale: {args.scale} OA units per SVG unit, '
                 f'min width {args.min_width}, text height {args.text_height}')
    lines.append(f'; origin: ({fmt_num(ox)} {fmt_num(oy)})')
    lines.append(f'cv = dbOpenCellViewByType("{args.lib}" "{args.cell}" "{args.view}" '
                 f'"schematicSymbol" "a")')
    body = []
    if not args.keep:
        # The terminals own the pins, and the pins own the pin figures, so the
        # terminals go first: leaving them behind would duplicate every port
        # name when the new pins are created.
        body.append("  foreach(term or(cv~>terminals nil) dbDeleteObject(term))")
        body.append("  foreach(fig or(cv~>shapes nil) dbDeleteObject(fig))")
        body.append("  foreach(inst or(cv~>instances nil) dbDeleteObject(inst))")
    body.append(f'  dbCreateRect(cv list("instance" "drawing") '
                f'list(list({fmt_num(ox)} {fmt_num(oy)}) '
                f'list({fmt_num(ox + width_oa)} {fmt_num(oy + height_oa)})))')
    body.extend(em.lines)
    if not args.no_native_labels:
        ix, iy = em.to_oa(vx + 0.02 * vw, vy + 0.03 * vh)
        px, py = em.to_oa(vx + 0.98 * vw, vy + 0.03 * vh)
        dy = 0.3 * args.text_height
        body.append(
            f'  schCreateSymbolLabel(cv list({fmt_num(ix)} {fmt_num(iy - dy)}) '
            f'"instance label" "[@instanceName]" "centerLeft" "R0" '
            f'{skill_quote(args.font)} {fmt_num(args.text_height)} "NLPLabel")')
        body.append(
            f'  schCreateSymbolLabel(cv list({fmt_num(px)} {fmt_num(py - dy)}) '
            f'"logical label" "[@partName]" "centerRight" "R0" '
            f'{skill_quote(args.font)} {fmt_num(args.text_height)} "NLPLabel")')
    if args.pins:
        sympin = "symPin"
        body.append(f'  {sympin} = dbOpenCellViewByType("basic" "sympin" "symbolNN" nil "r")')
        for idx, p in enumerate(ports):
            orient = {"left": "R180", "right": "R0", "top": "R270",
                      "bottom": "R90"}.get(p["side"], "R0")
            direction = p.get("direction") or "inputOutput"
            pin_var, lab_var = f"pin{idx}", f"lab{idx}"
            body.append(
                f'  {pin_var} = schCreateSymbolPin(cv {sympin} '
                f'{skill_quote(p["name"])} {skill_quote(direction)} '
                f'list({fmt_num(p["x"])} {fmt_num(p["y"])}) "{orient}")')
            if not args.no_native_labels:
                # The SVG `y` of a <text> is its baseline; the OA justification
                # "centerLeft"/"centerRight" anchors the text's vertical
                # centre. Lift the anchor by roughly half a text height so the
                # OA label sits on the SVG baseline instead of half a line
                # below it, which would collide with whatever is under it.
                ly = p["label_y"] + 0.35 * args.text_height
                body.append(
                    f'  {lab_var} = schCreateSymbolLabel(cv '
                    f'list({fmt_num(p["label_x"])} {fmt_num(ly)}) "pin name" '
                    f'{skill_quote(p["name"])} "{p["label_just"]}" '
                    f'"{p["label_orient"]}" {skill_quote(args.font)} '
                    f'{fmt_num(args.text_height)} "normal")')
                body.append(f'  schGlueLabel({lab_var} {pin_var})')
        # schCreateSymbolPin returns nil when it refuses the port, which is
        # what happens when a terminal with that name survived the clear. The
        # terminal list cannot be re-read to detect that inside one script --
        # the attribute list keeps the deleted entries -- so the pin handles
        # are checked instead.
        if ports:
            body.append("  pinfail = 0")
            for idx in range(len(ports)):
                body.append(f"  when(not(pin{idx}) pinfail = pinfail + 1)")
            body.append('  when(pinfail > 0 '
                        'error("svg2oa: schCreateSymbolPin failed for some ports"))')
        if order:
            order_skill = "list(" + " ".join(skill_quote(n) for n in order) + ")"
            body.append(f'  schEditPinOrder(cv {order_skill} nil)')
    body.append("  dbSave(cv)")
    body.append("  dbClose(cv)")
    lines.append("if(cv")
    lines.append("  (progn")
    lines.extend(body)
    lines.append('    "OK")')
    lines.append('  "CANNOT OPEN")')

    text = "\n".join(lines) + "\n"
    if args.out:
        with open(args.out, "w") as fh:
            fh.write(text)
    else:
        sys.stdout.write(text)

    if args.pin_json:
        with open(args.pin_json, "w") as fh:
            json.dump({"viewBox": em.vb, "scale": args.scale,
                       "origin": [ox, oy],
                       "terminal_order": order or [p["name"] for p in ports],
                       "ports": ports}, fh, indent=2)

    print(json.dumps({"shapes": em.counts, "ports": len(ports),
                      "terminal_order": order or [p["name"] for p in ports],
                      "out": args.out or "-"}, ensure_ascii=False), file=sys.stderr)


if __name__ == "__main__":
    main()
