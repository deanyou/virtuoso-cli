#!/usr/bin/env python3
"""
inject_stimulus.py — Inject stimulus and analysis blocks into a bare Spectre netlist.

Usage:
    python inject_stimulus.py input.scs [options]

The script:
  1. Reads a Spectre netlist that contains only subcircuit definitions
  2. Auto-detects (or accepts --type) the circuit type
  3. Injects: global/include preamble, DUT instantiation, stimulus sources,
     parameters block, and analysis statements
  4. Writes the complete testbench .scs (default: input_tb.scs)

Circuit types (auto-detected or via --type):
  ota          Single-ended OTA / opamp
  diff_ota     Fully-differential OTA (two differential outputs)
  ldo          LDO / voltage regulator (VIN + VOUT + FB)
  comparator   Comparator (differential input + VOUT ± CLK)
  bandgap      Bandgap reference (VDD + VREF/VBG)
  mirror       Current mirror / bias generator (IBIAS + IOUT)
  filter       Active filter (VIN + VOUT, single-ended)
  vco          VCO / ring oscillator (VCTRL/VDD + OUT)
  lna          LNA / RF amplifier (port-based S-parameter)
"""

import argparse
import re
import sys
import os
from pathlib import Path
from textwrap import dedent

# ──────────────────────────────────────────────────────────────────────────────
# Netlist parser — extract subcircuit declarations
# ──────────────────────────────────────────────────────────────────────────────

def parse_subcircuits(text):
    """Return list of (name, [ports]) from all subckt declarations."""
    subcircuits = []
    # handle line continuations (+)
    joined = re.sub(r'\\\s*\n\s*\+\s*', ' ', text)
    for m in re.finditer(
        r'^\s*subckt\s+(\S+)\s+(.*?)$', joined, re.MULTILINE | re.IGNORECASE
    ):
        name = m.group(1)
        ports = m.group(2).split()
        subcircuits.append((name, ports))
    return subcircuits


def normalize(name):
    """Lowercase, strip leading/trailing underscores and numbers for matching."""
    return re.sub(r'[_\d]+', '', name.lower())


# ──────────────────────────────────────────────────────────────────────────────
# Circuit type detection heuristics
# ──────────────────────────────────────────────────────────────────────────────

# Each entry: (type_name, priority, port_patterns_that_must_match, port_patterns_forbidden)
# Patterns are regex matched against the NORMALIZED port name set (joined with space).

HEURISTICS = [
    # fully-differential OTA — two differential outputs
    ("diff_ota", 100,
     [r'v(in|ip|inn|inm|inp)', r'v(outp|outn|op|on)'],
     [r'clk', r'vctrl', r'(rfin|rfout|port\d)']),

    # LDO — must have FB/VFEED
    ("ldo", 90,
     [r'v(in|inp)', r'v(out|outp)', r'(fb|vfeed|vfeedback)'],
     [r'clk', r'vctrl']),

    # Comparator — differential + clocked or output named VOUT
    ("comparator", 80,
     [r'v(in|ip|inn|inm|inp)', r'v(out|outp)'],
     [r'(fb|vfeed)', r'vctrl', r'(rfin|rfout)']),

    # Bandgap — VREF/VBG + no differential input
    ("bandgap", 85,
     [r'v(ref|bg|bgr|bандгап)'],
     [r'v(in[pm+-]|ip|inm|inn)', r'clk', r'vctrl']),

    # Current mirror — IBIAS/IOUT
    ("mirror", 75,
     [r'(ibias|iref|iin)', r'(iout|ioutput)'],
     [r'v(in[pm+-]|ip)', r'vctrl']),

    # VCO / ring oscillator — VCTRL or named ring/osc
    ("vco", 70,
     [r'(vctrl|ctrl|vcont)'],
     [r'v(in[pm+-]|ip)', r'(fb|vfeed)', r'(rfin|rfout)']),

    # LNA / RF — port elements or RF_IN/RF_OUT naming
    ("lna", 95,
     [r'(rfin|rfout|port\d|rfinput|rfoutput)'],
     []),

    # Single-ended OTA — must have differential input, single output
    ("ota", 60,
     [r'v(in[pm+-]|ip|inm|inn|inp)', r'v(out|outp)'],
     [r'(fb|vfeed)', r'vctrl', r'(rfin|rfout)']),

    # Active filter — VIN + VOUT, no differential input pins
    ("filter", 50,
     [r'v(in|input)', r'v(out|output)'],
     [r'v(in[pm+-]|ip|inm|inn)', r'(fb|vfeed)', r'vctrl']),
]


def detect_type(ports, cell_name=""):
    """Return best-match circuit type string."""
    normalized_ports = ' '.join(normalize(p) for p in ports)
    cell_lower = cell_name.lower()

    # Override hints from cell name
    if re.search(r'(ldo|regul|vreg)', cell_lower):
        return "ldo"
    if re.search(r'(bandgap|bgap|bгap|vref)', cell_lower):
        return "bandgap"
    if re.search(r'(comp|cmp|comparator)', cell_lower):
        return "comparator"
    if re.search(r'(vco|osc|ring)', cell_lower):
        return "vco"
    if re.search(r'(lna|pa\b|rfamp)', cell_lower):
        return "lna"
    if re.search(r'(mirror|bias|casc)', cell_lower):
        return "mirror"

    best = (None, -1)
    for (ctype, priority, must_match, forbidden) in HEURISTICS:
        score = 0
        skip = False
        for pat in forbidden:
            if re.search(pat, normalized_ports):
                skip = True
                break
        if skip:
            continue
        for pat in must_match:
            if re.search(pat, normalized_ports):
                score += 1
        if score == len(must_match) and priority > best[1]:
            best = (ctype, priority)

    return best[0] if best[0] else "ota"  # fallback


# ──────────────────────────────────────────────────────────────────────────────
# Port role mapping — find which port serves which role
# ──────────────────────────────────────────────────────────────────────────────

def find_port(ports, *patterns, fallback=None):
    """Return first port matching any pattern (case-insensitive), or fallback."""
    for p in ports:
        pn = p.upper()
        for pat in patterns:
            if re.search(pat, pn, re.IGNORECASE):
                return p
    return fallback or (ports[0] if ports else "VDD")


def find_ports_multi(ports, *patterns):
    """Return all ports matching any of the patterns."""
    result = []
    for p in ports:
        for pat in patterns:
            if re.search(pat, p, re.IGNORECASE):
                result.append(p)
                break
    return result


def infer_supply_ports(ports):
    vdd = find_port(ports, r'^VDD', r'^VDDA', r'^VCC', r'^DVDD', fallback="VDD")
    vss = find_port(ports, r'^VSS', r'^GND', r'^GND_', r'^AGND', fallback="VSS")
    return vdd, vss


# ──────────────────────────────────────────────────────────────────────────────
# Template generators — one per circuit type
# ──────────────────────────────────────────────────────────────────────────────

def _header(cell_name, ports, include_file, ctype):
    port_str = " ".join(ports)
    return dedent(f"""\
        // Auto-generated testbench — {cell_name} ({ctype})
        // inject_stimulus.py | Cadence Spectre syntax (IC231 / Spectre 20.1)
        simulator lang=spectre
        global 0
        include "{include_file}"

        // ── DUT ──────────────────────────────────────────────────────────────
        XDUT ({port_str}) {cell_name}
        """)


def gen_ota(cell_name, ports, include_file):
    vdd, vss = infer_supply_ports(ports)
    vout = find_port(ports, r'VOUT', r'OUT$', fallback="VOUT")
    vip  = find_port(ports, r'VIN\+', r'VINP', r'VIP', r'INP', fallback="VINP")
    vin_ = find_port(ports, r'VIN[-N]', r'VINM', r'VINM', r'INN', fallback="VINN")

    tb = _header(cell_name, ports, include_file, "ota")
    tb += dedent(f"""\
        parameters VDD=1.8 VSS=0 VCM=0.9 CL=1p Rload=1T

        // ── Supplies ─────────────────────────────────────────────────────────
        VVDD ({vdd} 0) vsource dc=VDD
        VVSS ({vss} 0) vsource dc=VSS

        // ── Differential AC input (mag=±0.5 → 1V differential) ───────────────
        VIP  ({vip}  0) vsource dc=VCM mag=0.5
        VIN_ ({vin_} 0) vsource dc=VCM mag=-0.5

        // ── Load ─────────────────────────────────────────────────────────────
        CL     ({vout} 0) capacitor c=CL
        Rload  ({vout} 0) resistor  r=Rload

        // ── Noise oprobe (1TΩ parallel — do NOT use series resistor) ─────────
        Rprobe ({vout} 0) resistor r=1T

        // ── Analyses ─────────────────────────────────────────────────────────
        dcop   dc    oppoint=rawfile save=allpub
        ac1    ac    start=1 stop=1G dec=50
        stb1   stb   start=1 stop=1G dec=50 probe=Rprobe
        noise1 noise start=1 stop=1G dec=50 outputport=Rprobe inputport=VIP
        """)
    return tb


def gen_diff_ota(cell_name, ports, include_file):
    vdd, vss = infer_supply_ports(ports)
    voutp = find_port(ports, r'VOUTP', r'OUTP', r'VOUT\+', fallback="VOUTP")
    voutn = find_port(ports, r'VOUTN', r'OUTN', r'VOUT[-N]', fallback="VOUTN")
    vip   = find_port(ports, r'VIN\+', r'VINP', r'VIP', r'INP', fallback="VINP")
    vin_  = find_port(ports, r'VIN[-N]', r'VINM', r'INN', fallback="VINN")
    vcmfb = find_port(ports, r'VCMFB', r'CMFB', fallback=None)

    tb = _header(cell_name, ports, include_file, "diff_ota")
    tb += dedent(f"""\
        parameters VDD=1.8 VSS=0 VCM=0.9 CL=1p
        """)
    tb += f"VVDD  ({vdd}  0) vsource dc=VDD\n"
    tb += f"VVSS  ({vss}  0) vsource dc=VSS\n"
    if vcmfb:
        tb += f"VCMFB ({vcmfb} 0) vsource dc=VCM    ; ideal CMFB for open-loop test\n"
    tb += dedent(f"""\

        VIP   ({vip}  0) vsource dc=VCM mag=0.5
        VIN_  ({vin_} 0) vsource dc=VCM mag=-0.5

        CLp    ({voutp} 0) capacitor c=CL
        CLn    ({voutn} 0) capacitor c=CL
        Rprobep ({voutp} 0) resistor r=1T
        Rproben ({voutn} 0) resistor r=1T

        dcop   dc    oppoint=rawfile save=allpub
        ac1    ac    start=1 stop=1G dec=50
        stb1   stb   start=1 stop=1G dec=50 probe=Rprobep
        noise1 noise start=1 stop=1G dec=50 outputport=Rprobep inputport=VIP
        """)
    return tb


def gen_ldo(cell_name, ports, include_file):
    vdd, vss = infer_supply_ports(ports)
    vout = find_port(ports, r'VOUT', r'OUT$', fallback="VOUT")
    fb   = find_port(ports, r'FB', r'VFEED', r'FEEDBACK', fallback="FB")

    tb = _header(cell_name, ports, include_file, "ldo")
    tb += dedent(f"""\
        parameters VIN=3.3 VREF=1.2 ILOAD_DC=1m ILOAD_MAX=100m

        // ── Supplies ─────────────────────────────────────────────────────────
        VVIN ({vdd} 0) vsource dc=VIN

        // ── Feedback (bypass resistor divider for open-loop PSRR/stb) ────────
        VFB  ({fb}  0) vsource dc=VREF

        // ── Load: DC + load step ─────────────────────────────────────────────
        ILOAD ({vout} 0) isource dc=ILOAD_DC
        ISTEP ({vout} 0) isource type=pulse \\
            val0=ILOAD_DC val1=ILOAD_MAX delay=5u period=30u rise=100n fall=100n width=10u

        // ── Loop-break probe (insert in feedback path for stb) ───────────────
        Ibrk  (VOUT_SENSE {vout}) iprobe

        // ── Noise probe ───────────────────────────────────────────────────────
        Rprobe ({vout} 0) resistor r=1T

        // ── Analyses ─────────────────────────────────────────────────────────
        dcop    dc    oppoint=rawfile save=allpub
        linereg dc    dev=VVIN start=1.5 stop=4.0 lin=51         ; line regulation
        loadreg dc    dev=ILOAD start=0 stop=ILOAD_MAX lin=51    ; load regulation
        stb1    stb   start=1 stop=100Meg dec=50 probe=Ibrk
        psrr    ac    start=1 stop=100Meg dec=50                  ; needs mag=1 on VVIN
        tran1   tran  stop=100u maxstep=10n errpreset=moderate    ; load step
        noise1  noise start=1 stop=10Meg dec=50 outputport=Rprobe inputport=VVIN
        """)
    return tb


def gen_comparator(cell_name, ports, include_file):
    vdd, vss = infer_supply_ports(ports)
    vout = find_port(ports, r'VOUT', r'OUT$', r'DOUT', fallback="VOUT")
    vip  = find_port(ports, r'VIN\+', r'VINP', r'VIP', r'INP', fallback="VINP")
    vin_ = find_port(ports, r'VIN[-N]', r'VINM', r'INN', fallback="VINN")
    clk  = find_port(ports, r'^CLK', r'CLOCK', fallback=None)

    tb = _header(cell_name, ports, include_file, "comparator")
    tb += "parameters VDD=1.8 VSS=0 VCM=0.9 TCLK=2n TCLK_H=1n VDIFF=10m\n\n"
    tb += f"VVDD ({vdd} 0) vsource dc=VDD\n"
    tb += f"VVSS ({vss} 0) vsource dc=VSS\n"

    if clk:
        # Clocked comparator
        tb += dedent(f"""\

            // ── Clock ────────────────────────────────────────────────────────
            VCLK ({clk} 0) vsource type=pulse val0=0 val1=VDD \\
                period=TCLK rise=50p fall=50p width=TCLK_H delay=0

            // ── Differential input pulse ──────────────────────────────────────
            VPOS ({vip}  0) vsource dc=VCM type=pulse \\
                val0=VCM+VDIFF val1=VCM-VDIFF rise=10p fall=10p \\
                period=TCLK width=TCLK_H delay=TCLK_H
            VNEG ({vin_} 0) vsource dc=VCM

            // ── Analyses ─────────────────────────────────────────────────────
            dcop  dc   oppoint=rawfile save=allpub
            tran1 tran stop=20n maxstep=5p errpreset=moderate   ; prop delay
            """)
    else:
        # Static comparator — ramp for offset + hysteresis
        tb += dedent(f"""\

            // ── Static input ramp (find threshold / offset) ───────────────────
            VPOS ({vip}  0) vsource type=pwl \\
                wave=[0 0  1n 0  501n VDD]   ; slow ramp from 0 to VDD
            VNEG ({vin_} 0) vsource dc=VCM

            // ── Analyses ─────────────────────────────────────────────────────
            dcop      dc   oppoint=rawfile save=allpub
            threshold dc   dev=VPOS start=0 stop=VDD lin=201
            hysteresis dc  dev=VPOS start=0 stop=VDD lin=201 hysteresis=yes
            tran1     tran stop=500n maxstep=100p errpreset=moderate
            """)
    return tb


def gen_bandgap(cell_name, ports, include_file):
    vdd, vss = infer_supply_ports(ports)
    vref = find_port(ports, r'VREF', r'VBG', r'VBGR', r'VOUT', fallback="VREF")

    tb = _header(cell_name, ports, include_file, "bandgap")
    tb += dedent(f"""\
        parameters VDD=3.3 VDD_MAX=3.6

        // ── Supply ───────────────────────────────────────────────────────────
        VVDD ({vdd} 0) vsource dc=VDD

        // ── Supply ramp for startup test ─────────────────────────────────────
        // (replace VVDD with VRAMP for startup tran; use VVDD for DC analyses)

        // ── Noise probe ───────────────────────────────────────────────────────
        Rprobe ({vref} 0) resistor r=1T

        // ── PSRR: inject AC on supply ─────────────────────────────────────────
        // VVDD has dc=VDD + mag=1 for AC → re-declare for PSRR analysis
        // (run as separate analysis or use acsource parameter)

        // ── Analyses ─────────────────────────────────────────────────────────
        dcop    dc    oppoint=rawfile save=allpub
        linereg dc    dev=VVDD start=1.5 stop=VDD_MAX lin=51    ; line regulation
        // psrr: re-run with VVDD mag=1 after dcop
        // tempco: use parametric corner sweep (temp=-40 to 125)
        noise1  noise start=1 stop=10Meg dec=50 outputport=Rprobe inputport=VVDD

        // ── Startup transient ─────────────────────────────────────────────────
        // Uncomment and replace VVDD with ramp source:
        // VRAMP ({vdd} 0) vsource type=pwl wave=[0 0  100n 0  10u VDD]
        // tran1 tran stop=20u maxstep=10n errpreset=moderate
        """)
    return tb


def gen_mirror(cell_name, ports, include_file):
    vdd, vss = infer_supply_ports(ports)
    iref = find_port(ports, r'IBIAS', r'IREF', r'IRIN', fallback="IBIAS")
    iout = find_port(ports, r'IOUT', r'IOUTPUT', r'IOUT\d', fallback="IOUT")

    tb = _header(cell_name, ports, include_file, "mirror")
    tb += dedent(f"""\
        parameters VDD=1.8 VSS=0 IREF_DC=10u

        VVDD ({vdd} 0) vsource dc=VDD
        VVSS ({vss} 0) vsource dc=VSS

        // ── Reference current ─────────────────────────────────────────────────
        IREF ({vdd} {iref}) isource dc=IREF_DC

        // ── Compliance voltage sweep ─────────────────────────────────────────
        VCMP ({iout} 0) vsource dc=0.2     ; swept to check output compliance

        // ── Output impedance (AC injection) ──────────────────────────────────
        VTEST ({iout}_ac {iout}) vsource dc=0 mag=1

        // ── Analyses ─────────────────────────────────────────────────────────
        dcop       dc    oppoint=rawfile save=allpub
        compliance dc    dev=VCMP start=0.05 stop=VDD lin=51
        ac_rout    ac    start=1 stop=100Meg dec=50   ; Rout = VTEST/ITEST
        """)
    return tb


def gen_filter(cell_name, ports, include_file):
    vdd, vss = infer_supply_ports(ports)
    vin  = find_port(ports, r'^VIN', r'^IN$', fallback="VIN")
    vout = find_port(ports, r'^VOUT', r'^OUT$', fallback="VOUT")

    tb = _header(cell_name, ports, include_file, "filter")
    tb += dedent(f"""\
        parameters VDD=1.8 VSS=0 VCM=0.9 VSTEP=100m

        VVDD ({vdd} 0) vsource dc=VDD
        VVSS ({vss} 0) vsource dc=VSS

        // ── AC input ─────────────────────────────────────────────────────────
        VIN_ac ({vin} 0) vsource dc=VCM mag=1

        // ── Step input (tran) ─────────────────────────────────────────────────
        // VSTEP_SRC ({vin} 0) vsource type=pulse val0=VCM val1=VCM+VSTEP \\
        //     delay=100n rise=1p fall=1p width=10u period=20u

        // ── Noise probe ───────────────────────────────────────────────────────
        Rprobe ({vout} 0) resistor r=1T

        // ── Analyses ─────────────────────────────────────────────────────────
        dcop   dc    oppoint=rawfile save=allpub
        ac1    ac    start=1 stop=1G dec=100      ; fine freq for filter shape
        tran1  tran  stop=50u maxstep=1n errpreset=moderate
        noise1 noise start=1 stop=1G dec=50 outputport=Rprobe inputport=VIN_ac
        """)
    return tb


def gen_vco(cell_name, ports, include_file):
    vdd, vss = infer_supply_ports(ports)
    vctrl = find_port(ports, r'VCTRL', r'CTRL', r'VCONT', fallback="VCTRL")
    vout  = find_port(ports, r'VOUT', r'^OUT', r'OSC', fallback="VOUT")

    tb = _header(cell_name, ports, include_file, "vco")
    tb += dedent(f"""\
        parameters VDD=1.8 VSS=0 VCTRL_NOM=0.9 Tosc=2n

        VVDD  ({vdd}   0) vsource dc=VDD
        VVSS  ({vss}   0) vsource dc=VSS
        VCTRL ({vctrl} 0) vsource dc=VCTRL_NOM

        // ── Tran: measure oscillation period via zero-crossings ──────────────
        tran1 tran stop=100n maxstep=1p errpreset=moderate skipdc=no

        // ── KVCO sweep: frequency vs control voltage ──────────────────────────
        // (run tran per VCTRL point, or use PSS with fundname)
        kvco dc dev=VCTRL start=0.2 stop=1.6 lin=29

        // ── PSS: periodic steady-state → phase noise ─────────────────────────
        // Uncomment after confirming oscillation frequency from tran:
        // pss1 pss fund=1/Tosc harms=20 errpreset=moderate maxacfreq=100G
        """)
    return tb


def gen_lna(cell_name, ports, include_file):
    vdd, vss = infer_supply_ports(ports)
    rfin  = find_port(ports, r'RF_?IN', r'RFIN', r'IN$', fallback="RFIN")
    rfout = find_port(ports, r'RF_?OUT', r'RFOUT', r'OUT$', fallback="RFOUT")
    vbias = find_port(ports, r'VBIAS', r'BIAS', fallback=None)

    tb = _header(cell_name, ports, include_file, "lna")
    tb += "parameters VDD=1.2 VSS=0 VBIAS=0.6\n\n"
    tb += f"VVDD ({vdd} 0) vsource dc=VDD\n"
    tb += f"VVSS ({vss} 0) vsource dc=VSS\n"
    if vbias:
        tb += f"VBIAS_SRC ({vbias} 0) vsource dc=VBIAS\n"
    tb += dedent(f"""\

        // ── S-parameter ports (50Ω reference) ────────────────────────────────
        PORT1 ({rfin}  0) port r=50 num=1
        PORT2 ({rfout} 0) port r=50 num=2

        // ── Analyses ─────────────────────────────────────────────────────────
        dcop  dc    oppoint=rawfile save=allpub
        sp1   sp    start=100Meg stop=10G dec=20 ports=[PORT1 PORT2]
        noise1 noise start=100Meg stop=10G dec=20 \\
            outputport=PORT2 inputport=PORT1
        // tran1 tran stop=50n maxstep=1p errpreset=moderate   ; 1dB CP / IIP3
        """)
    return tb


# ──────────────────────────────────────────────────────────────────────────────
# Dispatch table
# ──────────────────────────────────────────────────────────────────────────────

GENERATORS = {
    "ota":        gen_ota,
    "diff_ota":   gen_diff_ota,
    "ldo":        gen_ldo,
    "comparator": gen_comparator,
    "bandgap":    gen_bandgap,
    "mirror":     gen_mirror,
    "filter":     gen_filter,
    "vco":        gen_vco,
    "lna":        gen_lna,
}


# ──────────────────────────────────────────────────────────────────────────────
# CLI
# ──────────────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="Inject stimulus and analysis blocks into a bare Spectre netlist."
    )
    parser.add_argument("input", help="Input .scs netlist (subcircuit only)")
    parser.add_argument("-o", "--output", help="Output testbench .scs (default: <input>_tb.scs)")
    parser.add_argument(
        "--type", choices=list(GENERATORS.keys()),
        help="Circuit type override (default: auto-detect)"
    )
    parser.add_argument(
        "--cell",
        help="DUT subcircuit name (default: first subckt found)"
    )
    parser.add_argument(
        "--list", action="store_true",
        help="List subcircuits found in the netlist and exit"
    )
    args = parser.parse_args()

    input_path = Path(args.input)
    if not input_path.exists():
        print(f"ERROR: File not found: {input_path}", file=sys.stderr)
        sys.exit(1)

    text = input_path.read_text(errors="replace")
    subcircuits = parse_subcircuits(text)

    if not subcircuits:
        print("ERROR: No subckt declarations found in the netlist.", file=sys.stderr)
        sys.exit(1)

    if args.list:
        print("Subcircuits found:")
        for name, ports in subcircuits:
            print(f"  {name:30s}  ports: {' '.join(ports)}")
        return

    # Select DUT
    if args.cell:
        match = [(n, p) for n, p in subcircuits if n.lower() == args.cell.lower()]
        if not match:
            print(f"ERROR: Cell '{args.cell}' not found. Available: "
                  + ", ".join(n for n, _ in subcircuits), file=sys.stderr)
            sys.exit(1)
        cell_name, ports = match[0]
    else:
        # Use last top-level subcircuit (usually the DUT, not a sub-sub-cell)
        cell_name, ports = subcircuits[-1]

    # Detect or use explicit type
    ctype = args.type or detect_type(ports, cell_name)

    print(f"Cell   : {cell_name}")
    print(f"Ports  : {' '.join(ports)}")
    print(f"Type   : {ctype}")

    gen_fn = GENERATORS[ctype]
    tb_text = gen_fn(cell_name, ports, str(input_path.name))

    out_path = Path(args.output) if args.output else input_path.with_name(
        input_path.stem + "_tb.scs"
    )
    out_path.write_text(tb_text)
    print(f"Written: {out_path}")


if __name__ == "__main__":
    main()
