#!/usr/bin/env python3
"""Offline tests for the analog-artist-symbol helper scripts."""
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SKILL_DIR = Path(__file__).resolve().parents[1]
SCRIPTS = SKILL_DIR / "scripts"
EXAMPLES = SKILL_DIR / "examples"

MINI_SVG = """<?xml version='1.0' encoding='utf-8'?>
<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100">
  <title>MINI</title>
  <desc>Two-port functional symbol with a green signal wire.</desc>
  <metadata>{"terminal_order":["VIP","Q"],"ports":["VIP","Q"]}</metadata>
  <style>
    .wire{fill:none;stroke:#425d4a;stroke-width:0.1}
    .pin{fill:#df4638}
    .label{fill:#25613b}
    .port-label{fill:#c53e32}
  </style>
  <path class="wire" d="M20 50 H180"/>
  <text class="label" x="100" y="30">MINI</text>
  <g data-port="VIP" data-side="left" data-direction="input">
    <rect class="pin" x="16" y="46" width="8" height="8"/>
    <text class="port-label" x="30" y="44">VIP</text>
  </g>
  <g data-port="Q" data-side="right" data-direction="output">
    <rect class="pin" x="176" y="46" width="8" height="8"/>
    <text class="port-label" x="170" y="44" text-anchor="end">Q</text>
  </g>
</svg>
"""

BAD_SVG = """<?xml version='1.0' encoding='utf-8'?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <desc>Duplicate port test.</desc>
  <metadata>{"terminal_order":["VIP","Q"]}</metadata>
  <g data-port="VIP" data-side="left" data-direction="input">
    <rect x="0" y="50" width="8" height="8"/><text>VIP</text>
  </g>
  <g data-port="VIP" data-side="right" data-direction="output">
    <rect x="92" y="50" width="8" height="8"/><text>Q</text>
  </g>
</svg>
"""

ELLIPSE_SVG = """<?xml version='1.0' encoding='utf-8'?>
<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <title>ELLIPSE</title>
  <desc>Two-port functional symbol whose ports are marked with ellipses.</desc>
  <metadata>{"terminal_order":["VIP","Q"]}</metadata>
  <style>
    .wire{fill:none;stroke:#425d4a;stroke-width:0.1}
    .pin{fill:#df4638}
    .label{fill:#25613b}
    .port-label{fill:#c53e32}
  </style>
  <path class="wire" d="M64 64 H192"/>
  <text class="label" x="128" y="40">ELLIPSE</text>
  <g data-port="VIP" data-side="left" data-direction="input">
    <ellipse class="pin" cx="64" cy="64" rx="8" ry="8"/>
    <text class="port-label" x="80" y="60">VIP</text>
  </g>
  <g data-port="Q" data-side="right" data-direction="output">
    <ellipse class="pin" cx="192" cy="64" rx="8" ry="8"/>
    <text class="port-label" x="176" y="60" text-anchor="end">Q</text>
  </g>
</svg>
"""


def run_tool(script, *args, check=True):
    env = os.environ.copy()
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    return subprocess.run(
        [sys.executable, str(SCRIPTS / script), *map(str, args)],
        capture_output=True, text=True, env=env, check=check)


class SvgToolsTest(unittest.TestCase):

    def test_check_svg_accepts_valid_example(self):
        result = run_tool("check_svg.py", EXAMPLES / "cmp_strongarm_symbol.svg", "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertTrue(report["ok"], report["errors"])
        self.assertEqual(len(report["ports"]), 8)
        self.assertEqual(report["terminal_order"],
                         ["VIP", "VIN", "CLKB", "CLK", "QB", "Q", "VSS", "VDD"])

    def test_check_svg_rejects_duplicate_port(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "bad.svg"
            path.write_text(BAD_SVG, encoding="utf-8")
            result = run_tool("check_svg.py", path, "--json", check=False)
            self.assertNotEqual(result.returncode, 0)
            report = json.loads(result.stdout)
            self.assertFalse(report["ok"])
            self.assertTrue(any("duplicate" in e for e in report["errors"]))

    def test_svg2oa_emits_native_objects(self):
        with tempfile.TemporaryDirectory() as tmp:
            svg = Path(tmp) / "mini.svg"
            il = Path(tmp) / "mini.il"
            ports = Path(tmp) / "ports.json"
            svg.write_text(MINI_SVG, encoding="utf-8")
            result = run_tool(
                "svg2oa.py", svg, "--lib", "testLib", "--cell", "testCell",
                "--view", "symbol", "--pins", "--pin-order", "VIP,Q",
                "--pin-json", ports, "-o", il)
            self.assertEqual(result.returncode, 0, result.stderr)
            text = il.read_text(encoding="utf-8")
            self.assertIn('schCreateSymbolPin(cv symPin "VIP" "input"', text)
            self.assertIn('schCreateSymbolPin(cv symPin "Q" "output"', text)
            self.assertIn('"pin name" "VIP"', text)
            self.assertIn('"pin name" "Q"', text)
            self.assertIn('schGlueLabel(lab0 pin0)', text)
            self.assertIn('"[@instanceName]"', text)
            self.assertIn('"[@partName]"', text)
            self.assertIn('"NLPLabel"', text)
            self.assertIn('dbCreateRect(cv list("instance" "drawing")', text)
            self.assertIn('schEditPinOrder(cv list("VIP" "Q") nil)', text)
            self.assertIn('or(cv~>shapes nil)', text)
            self.assertIn('dbCreatePath(cv list("wire" "drawing")', text)
            self.assertIn(' 0.01)', text)
            self.assertNotIn('dbCreateLabel(cv list("pin" "drawing")', text)
            table = json.loads(ports.read_text(encoding="utf-8"))
            self.assertEqual(table["terminal_order"], ["VIP", "Q"])
            self.assertEqual(len(table["ports"]), 2)

    def test_svg2oa_converts_repository_example(self):
        with tempfile.TemporaryDirectory() as tmp:
            il = Path(tmp) / "example.il"
            ports = Path(tmp) / "ports.json"
            result = run_tool(
                "svg2oa.py", EXAMPLES / "cmp_strongarm_symbol.svg",
                "--lib", "testLib", "--cell", "cmp_strongarm", "--view", "symbol",
                "--pins", "--pin-json", ports, "-o", il)
            self.assertEqual(result.returncode, 0, result.stderr)
            text = il.read_text(encoding="utf-8")
            self.assertEqual(text.count('schCreateSymbolPin(cv symPin '), 8)
            self.assertEqual(text.count('schGlueLabel(lab'), 8)
            self.assertIn('schEditPinOrder(cv list("VIP" "VIN" "CLKB" "CLK" '
                          '"QB" "Q" "VSS" "VDD") nil)', text)
            table = json.loads(ports.read_text(encoding="utf-8"))
            self.assertEqual(table["terminal_order"],
                             ["VIP", "VIN", "CLKB", "CLK", "QB", "Q", "VSS", "VDD"])

    def test_pin_label_sits_above_its_pin(self):
        """The OA pin label is anchored by its vertical centre, the SVG text by
        its baseline. The converter must lift the anchor by half a text height,
        otherwise the red port label lands on top of the text underneath it."""
        with tempfile.TemporaryDirectory() as tmp:
            il = Path(tmp) / "example.il"
            ports = Path(tmp) / "ports.json"
            run_tool(
                "svg2oa.py", EXAMPLES / "cmp_strongarm_symbol.svg",
                "--lib", "testLib", "--cell", "cmp_strongarm", "--view", "symbol",
                "--pins", "--pin-json", ports, "-o", il)
            table = json.loads(ports.read_text(encoding="utf-8"))
            by_name = {p["name"]: p for p in table["ports"]}
            text = il.read_text(encoding="utf-8")
            for name, port in by_name.items():
                label = re.search(
                    r'schCreateSymbolLabel\(cv list\(([-\d.]+) ([-\d.]+)\) '
                    r'"pin name" "%s"' % re.escape(name), text)
                self.assertIsNotNone(label, "no pin label for %s" % name)
                label_y = float(label.group(2))
                # Lifted: the label anchor sits above the SVG baseline.
                self.assertGreater(label_y, port["label_y"],
                                   "%s label must be lifted above the baseline" % name)
                self.assertLessEqual(label_y - port["label_y"], 0.0625,
                                     "%s label lifted too far" % name)
                # Still beside its own pin, not drifting onto a neighbour.
                self.assertLess(abs(label_y - port["label_y"]), 0.0625)

    def test_svg2oa_clears_terminals_before_creating_pins(self):
        """Rewriting a live cellview must delete the terminals first and abort
        when any schCreateSymbolPin returns nil. A length(cv~>terminals) guard
        reads the pre-delete snapshot inside the same evaluation and misfires.
        """
        with tempfile.TemporaryDirectory() as tmp:
            il = Path(tmp) / "mini.il"
            svg = Path(tmp) / "mini.svg"
            svg.write_text(MINI_SVG, encoding="utf-8")
            run_tool(
                "svg2oa.py", svg, "--lib", "testLib",
                "--cell", "testCell", "--pins", "-o", il)
            text = il.read_text(encoding="utf-8")
            self.assertIn("foreach(term or(cv~>terminals nil) dbDeleteObject(term))", text)
            self.assertLess(
                text.index("dbDeleteObject(term)"),
                text.index("schCreateSymbolPin"))
            self.assertIn("when(not(pin0) pinfail = pinfail + 1)", text)
            self.assertIn('error("svg2oa: schCreateSymbolPin failed for some ports")', text)
            self.assertNotIn("when(length(cv~>terminals)", text)

    def test_origin_offset_keeps_the_existing_pin_frame(self):
        """--origin-x/--origin-y move the drawing onto the pin frame already
        used by FT0001A_SH/cmp_strongarm, so the parent schematic keeps its
        connections. The selection box must stay exactly the original bBox.
        """
        expected = {
            "VIP": "0 -0.375", "VIN": "0 -0.25",
            "CLKB": "0 -0.125", "CLK": "0 -0",
            "Q": "1.6875 -0.125", "QB": "1.6875 -0",
            "VDD": "0.8125 0.5", "VSS": "0.9375 0.5",
        }
        with tempfile.TemporaryDirectory() as tmp:
            il = Path(tmp) / "ft0001a.il"
            ports = Path(tmp) / "ports.json"
            run_tool(
                "svg2oa.py", EXAMPLES / "cmp_strongarm_ft0001a.svg",
                "--lib", "FT0001A_SH", "--cell", "cmp_strongarm",
                "--view", "symbol", "--pins", "--origin-x", "-0.025",
                "--origin-y", "-0.625", "--pin-json", ports, "-o", il)
            text = il.read_text(encoding="utf-8")
            self.assertIn(
                'dbCreateRect(cv list("instance" "drawing") '
                'list(list(-0.025 -0.625) list(2.16875 0.525)))', text)
            for name, xy in expected.items():
                self.assertIn(
                    'schCreateSymbolPin(cv symPin "%s"' % name, text)
                found = re.search(
                    r'schCreateSymbolPin\(cv symPin "%s" '
                    r'"(?:input|output|inputOutput)" list\(([-\d.]+) ([-\d.]+)\)'
                    % re.escape(name), text)
                self.assertIsNotNone(found, "no pin for %s" % name)
                self.assertAlmostEqual(float(found.group(1)), float(xy.split()[0]), places=6)
                self.assertAlmostEqual(float(found.group(2)), float(xy.split()[1]), places=6)
            table = json.loads(ports.read_text(encoding="utf-8"))
            self.assertEqual(table["origin"], [-0.025, -0.625])
            self.assertEqual(table["viewBox"], [0.0, 0.0, 561.6, 294.4])
            self.assertEqual(table["terminal_order"],
                             ["Q", "QB", "VDD", "VSS", "CLK", "CLKB", "VIN", "VIP"])

    def test_text_height_snaps_to_nearest_legal_value(self):
        """A 13px label must stay smaller than a 16px one after OA snapping."""
        spec = importlib.util.spec_from_file_location("svg2oa", SCRIPTS / "svg2oa.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        scale = 1.0 / 256.0
        self.assertEqual(module.snap_text_height(16 * scale), 0.0625)
        self.assertEqual(module.snap_text_height(19 * scale), 0.075)
        self.assertEqual(module.snap_text_height(13 * scale), 0.05)
        self.assertEqual(module.snap_text_height(0.001), 0.03125)

    def test_ellipse_port_markers_keep_their_terminal(self):
        """check_svg accepts an ellipse as a port marker, so the converter has
        to read one as well. An unread marker leaves the port without a centre,
        the port is dropped and the symbol loses that terminal entirely."""
        with tempfile.TemporaryDirectory() as tmp:
            svg = Path(tmp) / "ellipse.svg"
            il = Path(tmp) / "ellipse.il"
            ports = Path(tmp) / "ports.json"
            svg.write_text(ELLIPSE_SVG, encoding="utf-8")

            report = json.loads(run_tool("check_svg.py", svg, "--json").stdout)
            self.assertTrue(report["ok"], report["errors"])
            self.assertEqual([p["name"] for p in report["ports"]], ["VIP", "Q"])
            self.assertEqual(report["warnings"], [])

            run_tool("svg2oa.py", svg, "--lib", "testLib", "--cell", "testCell",
                     "--view", "symbol", "--pins", "--pin-json", ports, "-o", il)
            text = il.read_text(encoding="utf-8")
            self.assertEqual(text.count("schCreateSymbolPin(cv symPin "), 2)
            self.assertIn('schCreateSymbolPin(cv symPin "VIP" "input" '
                          "list(0.25 0.75)", text)
            self.assertIn('schCreateSymbolPin(cv symPin "Q" "output" '
                          "list(0.75 0.75)", text)
            self.assertIn('schEditPinOrder(cv list("VIP" "Q") nil)', text)

            table = json.loads(ports.read_text(encoding="utf-8"))
            self.assertEqual([p["name"] for p in table["ports"]], ["VIP", "Q"])
            by_name = {p["name"]: p for p in table["ports"]}
            # The pin sits on the ellipse centre, not on the group origin.
            self.assertAlmostEqual(by_name["VIP"]["svg_x"], 64.0, places=6)
            self.assertAlmostEqual(by_name["VIP"]["svg_y"], 64.0, places=6)
            self.assertAlmostEqual(by_name["Q"]["svg_x"], 192.0, places=6)
            self.assertAlmostEqual(by_name["Q"]["svg_y"], 64.0, places=6)

    def test_apply_skill_dry_run_builds_rpc_params(self):
        with tempfile.TemporaryDirectory() as tmp:
            il = Path(tmp) / "probe.il"
            il.write_text('dbSave(cv)\n"OK"\n', encoding="utf-8")
            result = run_tool("apply_skill.py", il, "--dry-run")
            self.assertEqual(result.returncode, 0, result.stderr)
            params = json.loads(result.stdout)
            self.assertIn("dbSave(cv)", params["code"])
            self.assertIn('"OK"', params["code"])


if __name__ == "__main__":
    unittest.main()
