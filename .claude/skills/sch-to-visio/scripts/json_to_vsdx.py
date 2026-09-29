#!/usr/bin/env python3
"""Render the limited vcli schematic export as editable generic Visio shapes."""
from __future__ import annotations

import argparse
import json
import math
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

VISIO = "http://schemas.microsoft.com/office/visio/2011/1/core"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
CT = "http://schemas.openxmlformats.org/package/2006/content-types"
ET.register_namespace("", VISIO)
ET.register_namespace("r", REL)


def elem(parent, tag, **attrs):
    return ET.SubElement(parent, f"{{{VISIO}}}{tag}", {k: str(v) for k, v in attrs.items()})


def cell(parent, name, value):
    elem(parent, "Cell", N=name, V=f"{value:g}" if isinstance(value, (float, int)) else value)


def rectangle(parent, sid, name, label, x, y, width, height):
    sh = elem(parent, "Shape", ID=sid, NameU=name, Name=name, Type="Shape", LineStyle="0", FillStyle="0", TextStyle="0")
    for key, value in (("PinX", x), ("PinY", y), ("Width", width), ("Height", height),
                       ("LocPinX", width / 2), ("LocPinY", height / 2), ("FillForegnd", "RGB(240,246,255)"),
                       ("LineColor", "RGB(32,55,84)"), ("LineWeight", 0.015)):
        cell(sh, key, value)
    geom = elem(sh, "Section", N="Geometry", IX="0")
    for ix, (kind, gx, gy) in enumerate((("MoveTo", 0, 0), ("LineTo", width, 0),
                                         ("LineTo", width, height), ("LineTo", 0, height),
                                         ("LineTo", 0, 0))):
        row = elem(geom, "Row", T=kind, IX=ix)
        cell(row, "X", gx)
        cell(row, "Y", gy)
    elem(sh, "Text").text = label


def xml(root):
    return ET.tostring(root, encoding="utf-8", xml_declaration=True)


def package(data):
    if data.get("format") != "virtuoso-cli.schematic-export" or data.get("version") != 1:
        raise ValueError("expected virtuoso-cli.schematic-export version 1")
    for key in ("instances", "nets", "pins"):
        if not isinstance(data.get(key), list):
            raise ValueError(f"{key} must be an array")
    instances, pins, nets = data["instances"], data["pins"], data["nets"]
    coords = []
    for inst in instances:
        x, y = float(inst["x"]), float(inst["y"])
        if not math.isfinite(x) or not math.isfinite(y):
            raise ValueError("instance coordinates must be finite")
        coords.append((x, y))
    min_x = min((x for x, _ in coords), default=0)
    min_y = min((y for _, y in coords), default=0)
    # Preserve relative placement with a fixed scale; avoid stretching short axes.
    scale = min(0.5, 7 / max((max((x for x, _ in coords), default=0) - min_x), 1),
                7 / max((max((y for _, y in coords), default=0) - min_y), 1))
    page = ET.Element(f"{{{VISIO}}}PageContents")
    shapes = elem(page, "Shapes")
    sid = 1
    for inst in instances:
        x = 1.5 + (float(inst["x"]) - min_x) * scale
        y = 2 + (float(inst["y"]) - min_y) * scale
        rectangle(shapes, sid, str(inst["name"]), f'{inst["name"]}\n{inst["master"]}', x, y, 1.4, 0.7)
        sid += 1
    # Pins and net names remain independent editable shapes. The source schema
    # has no terminal positions or wire paths from which to draw connections.
    for index, pin in enumerate(pins):
        rectangle(shapes, sid, f'pin_{pin["name"]}', f'{pin["name"]} ({pin["direction"]})',
                  1.5 + (index % 5) * 1.6, 10.5 - (index // 5) * 0.5, 1.4, 0.3)
        sid += 1
    for index, net in enumerate(nets):
        rectangle(shapes, sid, f'net_{net}', str(net), 9.3, 10.5 - index * 0.4, 1.3, 0.3)
        sid += 1
    doc = ET.Element(f"{{{VISIO}}}VisioDocument")
    styles = elem(doc, "StyleSheets")
    style = elem(styles, "StyleSheet", ID="0", NameU="No Style", Name="No Style")
    for key, value in (("LineColor", "RGB(0,0,0)"), ("FillForegnd", "RGB(255,255,255)"),
                       ("LinePattern", 1), ("FillPattern", 1), ("VerticalAlign", 1)):
        cell(style, key, value)
    pages = ET.Element(f"{{{VISIO}}}Pages")
    p = elem(pages, "Page", ID="0", NameU="Schematic", Name="Schematic")
    sheet = elem(p, "PageSheet", LineStyle="0", FillStyle="0", TextStyle="0")
    for key, value in (("PageWidth", 11), ("PageHeight", 12), ("PageScale", 1), ("DrawingScale", 1)):
        cell(sheet, key, value)
    elem(p, "Rel", **{f"{{{REL}}}id": "rId1"})
    types = ET.Element(f"{{{CT}}}Types")
    for ext, content in (("rels", "application/vnd.openxmlformats-package.relationships+xml"),
                         ("xml", "application/xml")):
        ET.SubElement(types, f"{{{CT}}}Default", Extension=ext, ContentType=content)
    for path, content in (("/visio/document.xml", "application/vnd.ms-visio.document.main+xml"),
                          ("/visio/pages/pages.xml", "application/vnd.ms-visio.pages+xml"),
                          ("/visio/pages/page1.xml", "application/vnd.ms-visio.page+xml")):
        ET.SubElement(types, f"{{{CT}}}Override", PartName=path, ContentType=content)
    def relationships(items):
        root = ET.Element(f"{{{PKG}}}Relationships")
        for rid, kind, target in items:
            ET.SubElement(root, f"{{{PKG}}}Relationship", Id=rid,
                          Type=f"http://schemas.microsoft.com/visio/2010/relationships/{kind}", Target=target)
        return xml(root)
    return {
        "[Content_Types].xml": xml(types),
        "_rels/.rels": relationships([("rId1", "document", "visio/document.xml")]),
        "visio/document.xml": xml(doc),
        "visio/_rels/document.xml.rels": relationships([("rId1", "pages", "pages/pages.xml")]),
        "visio/pages/pages.xml": xml(pages),
        "visio/pages/_rels/pages.xml.rels": relationships([("rId1", "page", "page1.xml")]),
        "visio/pages/page1.xml": xml(page),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.output.suffix.lower() != ".vsdx":
        parser.error("output path must end in .vsdx")
    try:
        files = package(json.loads(args.input.read_text(encoding="utf-8")))
    except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as exc:
        parser.exit(1, f"error: {exc}\n")
    with zipfile.ZipFile(args.output, "w", zipfile.ZIP_DEFLATED) as out:
        for name, contents in files.items():
            out.writestr(name, contents)


if __name__ == "__main__":
    main()
