#!/usr/bin/env python3
"""Validate an artist-style SVG symbol before OA conversion."""
from __future__ import annotations

import argparse
import json
import re
import sys
import xml.etree.ElementTree as ET

SIDES = {"left", "right", "top", "bottom"}
DIRECTIONS = {"input", "output", "inputOutput"}
SHAPE_TAGS = {"rect", "circle", "ellipse", "line", "polyline", "polygon", "path"}


def local_tag(tag: str) -> str:
    return tag.split("}")[-1]


def parse_viewbox(text: str) -> list[float] | None:
    if not text:
        return None
    nums = [float(n) for n in re.findall(
        r"-?\d*\.?\d+(?:e[-+]?\d+)?", text)]
    return nums if len(nums) == 4 else None


def check_svg(path: str) -> dict:
    errors: list[str] = []
    warnings: list[str] = []
    root = ET.parse(path).getroot()
    vb = parse_viewbox(root.get("viewBox"))
    if vb is None:
        errors.append("viewBox is missing or does not contain 4 numbers")

    ports: list[dict] = []
    seen: set[str] = set()
    for el in root.iter():
        name = el.get("data-port")
        if not name:
            continue
        side = el.get("data-side", "")
        direction = el.get("data-direction", "")
        ports.append({"name": name, "side": side, "direction": direction,
                      "tag": local_tag(el.tag)})
        if name in seen:
            errors.append(f"duplicate data-port: {name}")
        seen.add(name)
        if side not in SIDES:
            errors.append(f"{name}: invalid data-side {side!r}")
        if direction not in DIRECTIONS:
            errors.append(f"{name}: invalid data-direction {direction!r}")
        if local_tag(el.tag) != "g":
            warnings.append(f"{name}: data-port is on <{local_tag(el.tag)}>, expected <g>")
        child_tags = [local_tag(c.tag) for c in el]
        if not any(t in SHAPE_TAGS for t in child_tags):
            warnings.append(f"{name}: no pin shape inside the port group")
        if "text" not in child_tags:
            warnings.append(f"{name}: no text label inside the port group")

    if not ports:
        errors.append("no data-port groups found")

    desc = ""
    metadata: dict = {}
    for el in root.iter():
        if local_tag(el.tag) == "desc" and "".join(el.itertext()).strip():
            desc = "".join(el.itertext()).strip()
        if local_tag(el.tag) == "metadata" and el.text:
            try:
                metadata = json.loads(el.text)
            except (ValueError, TypeError) as exc:
                errors.append(f"metadata is not valid JSON: {exc}")
    if not desc:
        errors.append("non-empty <desc> is required")
    order = metadata.get("terminal_order")
    if not isinstance(order, list) or not all(isinstance(n, str) for n in order):
        errors.append("metadata.terminal_order must be a list of strings")
        order = []
    elif len(order) != len(set(order)):
        errors.append("metadata.terminal_order contains duplicate names")
    elif set(order) != seen:
        errors.append("metadata.terminal_order does not match data-port names")

    return {"ok": not errors, "svg": path, "viewBox": vb,
            "ports": ports, "terminal_order": order,
            "desc_present": bool(desc), "errors": errors,
            "warnings": warnings}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("svg")
    ap.add_argument("--json", action="store_true", help="emit JSON report")
    args = ap.parse_args()
    try:
        report = check_svg(args.svg)
    except (ET.ParseError, OSError) as exc:
        report = {"ok": False, "svg": args.svg, "errors": [str(exc)],
                  "warnings": [], "ports": [], "terminal_order": []}
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        print(f"{args.svg}: {'OK' if report['ok'] else 'FAIL'}")
        for item in report["errors"]:
            print(f"  error: {item}")
        for item in report["warnings"]:
            print(f"  warning: {item}")
        print(f"  ports: {', '.join(p['name'] for p in report['ports'])}")
        print(f"  terminal_order: {', '.join(report['terminal_order'])}")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
