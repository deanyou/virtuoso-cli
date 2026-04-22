#!/usr/bin/env python3
"""
Read Maestro output expression values directly from PSF binary files.
No Virtuoso GUI or bridge required.

Usage:
  python3 read_results.py <maestro_dir>
  python3 read_results.py <maestro_dir> --run ExplorerRun.0
  python3 read_results.py <maestro_dir> --test <test_name>
  python3 read_results.py <maestro_dir> --list   # list available runs

Output: JSON to stdout.
"""
import argparse
import json
import math
import os
import re
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path


PSF_TOOL_CANDIDATES = [
    "/opt/cadence/IC231/bin/psf",
    "/opt/cadence/IC23.1/bin/psf",
    "/opt/cadence/ICADVM20.1/bin/psf",
]


# ---------------------------------------------------------------------------
# XML parsing helpers for Cadence's non-standard XML dialects
# ---------------------------------------------------------------------------

def parse_sdb(path):
    """Parse a Cadence .sdb XML file. Returns the root Element."""
    text = Path(path).read_text(errors="replace")
    if not text.strip():
        return ET.fromstring('<empty/>')
    # Remove bare text immediately after opening tags (the "section name" tokens)
    text = re.sub(r'(<(?:setupdb|statedb)[^>]*>)\s*\w[\w\s]*\n', r'\1\n', text)
    text = re.sub(r'(<(?:active|history|Test|component|partition)(?:\s[^>]*)?>)\s*\w[\w .]+\n', r'\1\n', text)
    try:
        return ET.fromstring(text)
    except ET.ParseError:
        # Fall back: strip ALL bare text between tags
        text = re.sub(r'>([^<\n]*)\n', lambda m: '>\n' if not m.group(1).strip().startswith('<') else m.group(0), text)
        return ET.fromstring(text)


def field_text(element, name):
    """Find a <field Name="name"> and return its text content."""
    for f in element.iter('field'):
        if f.get('Name') == name:
            return (f.text or '').strip()
    return ''


def fields_dict(element):
    """Return all <field> children as {name: text}."""
    result = {}
    for f in element:
        if f.tag == 'field':
            result[f.get('Name', '')] = (f.text or '').strip()
    return result


# ---------------------------------------------------------------------------
# Parse maestro.sdb
# ---------------------------------------------------------------------------

def parse_maestro_sdb(maestro_dir):
    path = Path(maestro_dir) / 'maestro.sdb'
    root = parse_sdb(str(path))
    active = root.find('active')
    if active is None:
        return {}

    result = {
        'tests': [],
        'corners': [],
        'vars': {},
        'overwrite_history_name': field_text(active, 'overwritehistoryname') or 'ExplorerRun.0',
    }

    for test in active.findall('.//tests/test'):
        test_name = test.get('Name') or (test.text or '').strip().split('\n')[0].strip()
        opts = {}
        for opt in test.findall('.//tooloptions/option'):
            opt_name = opt.get('Name') or (opt.text or '').strip().split('\n')[0].strip()
            val = field_text(opt, 'value')
            if opt_name and val:
                opts[opt_name] = val
        result['tests'].append({'name': test_name, **opts})

    for corner in active.findall('.//corners/corner'):
        result['corners'].append((corner.text or '').strip())

    for var in active.findall('.//vars/var'):
        var_name = var.get('Name') or (var.text or '').strip().split('\n')[0].strip()
        val = field_text(var, 'value')
        if var_name:
            result['vars'][var_name] = val

    return result


# ---------------------------------------------------------------------------
# Parse active.state — output expression definitions
# ---------------------------------------------------------------------------

def parse_active_state(maestro_dir):
    path = Path(maestro_dir) / 'active.state'
    if not path.exists():
        return []

    text = path.read_text(errors="replace")
    outputs = []

    # Find all outputList_N start positions (outer defstruct block).
    # Child <field> tags all close with </field>, so we can't use a greedy/lazy
    # span across the outer block. Instead: locate each block start, then extract
    # name/expression/evalType with targeted regexes within the next window.
    for m in re.finditer(r'<field Name="outputList_\d+" Type="defstruct">sevOutputStruct', text):
        start = m.end()
        # Scan forward to find the closing </field> of the outer block.
        # Each child <field> is self-closing (no nesting), so the first </field>
        # NOT preceded by a child tag close is the outer one. Simpler: find the
        # position of the next <field Name="outputList_ OR end of outputsCommon.
        end_m = re.search(r'</field>\s*(?=\s*(?:<field Name="outputList_\d+"|</field>|<partition))', text[start:])
        block = text[start: start + end_m.start() + 8] if end_m else text[start: start + 4096]

        name_m = re.search(r'<field Name="name"[^>]*>"([^"]+)"</field>', block)
        expr_m = re.search(r'<field Name="expression"[^>]*>([^<]+)</field>', block)
        eval_m = re.search(r'<field Name="evalType"[^>]*>(\w+)</field>', block)

        if not name_m or not expr_m:
            continue

        expr_raw = expr_m.group(1).strip()
        if expr_raw in ('nil', ''):
            continue

        outputs.append({
            'name': name_m.group(1),
            'expression': expr_raw,
            'eval_type': eval_m.group(1) if eval_m else 'point',
        })

    return outputs


# ---------------------------------------------------------------------------
# Parse history.sdb — run entries
# ---------------------------------------------------------------------------

def parse_history_sdb(maestro_dir):
    path = Path(maestro_dir) / 'history' / 'history.sdb'
    if not path.exists():
        return []

    root = parse_sdb(str(path))
    runs = []
    for entry in root.findall('.//history/historyentry'):
        name_elem = entry
        run_name = entry.get('Name') or ''
        # The run name is sometimes the element text before first child
        if not run_name:
            raw = (entry.text or '').strip()
            run_name = raw.split('\n')[0].strip() if raw else ''

        psf_dir_raw = ''
        for child in entry:
            if child.tag == 'psfdir':
                psf_dir_raw = (child.text or '').strip()
            elif child.tag == 'timestamp':
                ts = (child.text or '').strip()

        if psf_dir_raw:
            # Substitute $AXL_HISTORY_NAME with the run name
            psf_dir = psf_dir_raw.replace('$AXL_HISTORY_NAME', run_name)
            runs.append({'name': run_name, 'psf_base': psf_dir, 'timestamp': ts if 'ts' in dir() else ''})

    return runs


# ---------------------------------------------------------------------------
# PSF reading
# ---------------------------------------------------------------------------

def find_psf_tool():
    for candidate in PSF_TOOL_CANDIDATES:
        if Path(candidate).exists():
            return candidate
    # Try PATH
    try:
        result = subprocess.run(['which', 'psf'], capture_output=True, text=True)
        if result.returncode == 0:
            return result.stdout.strip()
    except Exception:
        pass
    return None


def psf_to_ascii(psf_file, psf_tool):
    """Run Cadence psf tool and return stdout as string."""
    result = subprocess.run(
        [psf_tool, '-i', str(psf_file), '-o', '/dev/stdout'],
        capture_output=True, text=True, timeout=30
    )
    return result.stdout


def parse_psf_ascii(ascii_text, analysis_type):
    """
    Parse psf ASCII output into:
      { 'sweep': [x0, x1, ...],
        'signals': { signal_name: [(re, im), ...] or [y0, y1, ...] } }
    For DC (no sweep): signals = { name: scalar }
    """
    lines = ascii_text.split('\n')
    in_value = False
    sweep = []
    signals = {}
    current_x = None

    for line in lines:
        stripped = line.strip()

        if stripped == 'VALUE':
            in_value = True
            continue
        if not in_value:
            continue

        # Sweep variable line: "freq" 100.0  or "time" 1e-9
        m = re.match(r'^"(?:freq|time|sweep)"\s+([\d.e+\-]+)', stripped)
        if m:
            current_x = float(m.group(1))
            sweep.append(current_x)
            continue

        # DC scalar: "name" "type" value
        m = re.match(r'^"([^"]+)"\s+"[^"]+"\s+([\d.e+\-]+)\s*$', stripped)
        if m and analysis_type == 'dc':
            signals.setdefault(m.group(1), []).append(float(m.group(2)))
            continue

        # Complex value: "name" (re im)
        m = re.match(r'^"([^"]+)"\s+\(([-\d.e+]+)\s+([-\d.e+]+)\)', stripped)
        if m:
            re_val, im_val = float(m.group(2)), float(m.group(3))
            signals.setdefault(m.group(1), []).append(complex(re_val, im_val))
            continue

        # Real value (noise, tran): "name" scalar
        m = re.match(r'^"([^"]+)"\s+([-\d.e+]+)\s*$', stripped)
        if m and current_x is not None:
            signals.setdefault(m.group(1), []).append(float(m.group(2)))
            continue

    # For DC: collapse single-element lists to scalars
    if analysis_type == 'dc':
        signals = {k: v[0] if len(v) == 1 else v for k, v in signals.items()}

    return {'sweep': sweep, 'signals': signals}


def load_psf(psf_dir, analysis, psf_tool):
    """Load PSF data for a given analysis type from psf_dir."""
    analysis_map = {
        'ac': 'ac.ac',
        'dc': 'dc.dc',
        'tran': 'tran.tran',
        'noise': 'noise.noise',
        'stb': 'stb.stb',
        'pss': 'pss.fd',
    }
    fname = analysis_map.get(analysis, f'{analysis}.{analysis}')
    psf_file = Path(psf_dir) / fname
    if not psf_file.exists():
        # Try glob
        candidates = list(Path(psf_dir).glob(f'{analysis}.*'))
        if not candidates:
            return None
        psf_file = candidates[0]

    ascii_text = psf_to_ascii(str(psf_file), psf_tool)
    return parse_psf_ascii(ascii_text, analysis)


# ---------------------------------------------------------------------------
# Expression evaluator
# ---------------------------------------------------------------------------

class Waveform(list):
    """List of (x, y) tuples with Ocean-style indexing: wave[N] → y at index N."""

    def __getitem__(self, n):
        item = super().__getitem__(n)
        if isinstance(n, int):
            return item[1]  # Ocean: wave[N] returns the y value, not the (x,y) pair
        return item

    def __truediv__(self, other):
        if isinstance(other, Waveform):
            return Waveform((x, a / b) for (x, a), (_, b) in zip(self, other))
        return Waveform((x, y / other) for x, y in self)

    def __rtruediv__(self, other):
        return Waveform((x, other / y) for x, y in self)


def ocean_to_python(expr):
    """
    Transform Ocean expression syntax to Python-evaluable syntax.
    ?keyword "value" → , keyword="value"   (adds the missing comma)
    ?keyword symbol  → , keyword="symbol"
    """
    # ?keyword "string" — add comma before keyword arg
    expr = re.sub(r'\s*\?(\w+)\s+"([^"]*)"', r', \1="\2"', expr)
    # ?keyword symbol (unquoted) — add comma before keyword arg
    expr = re.sub(r'\s*\?(\w+)\s+\'?([A-Za-z]\w*)', r', \1="\2"', expr)
    return expr


def make_ocean_env(psf_cache, psf_tool):
    """
    Return a dict usable as eval() globals mapping Ocean functions to Python.
    psf_cache: dict {analysis_type -> psf_data} (lazily populated)
    """

    def get_signal(net, analysis):
        net_key = net.lstrip('/')
        if analysis not in psf_cache:
            psf_cache[analysis] = psf_cache.get('_load_fn')(analysis)
        data = psf_cache.get(analysis)
        if data is None:
            return None
        return data['signals'].get(net_key)

    def get_sweep(analysis):
        if analysis not in psf_cache:
            psf_cache[analysis] = psf_cache.get('_load_fn')(analysis)
        data = psf_cache.get(analysis)
        return data['sweep'] if data else []

    def getData(net, result='dc'):
        sig = get_signal(net, result)
        if sig is None:
            return None
        sweep = get_sweep(result)
        if sweep:
            return Waveform(zip(sweep, sig))  # Ocean waveform: supports [N] → y
        return sig  # scalar for DC

    def VF(net):
        return getData(net, 'ac')

    def VT(net):
        return getData(net, 'tran')

    def dB20(wave):
        if isinstance(wave, list):
            return [(x, 20 * math.log10(abs(y)) if abs(y) > 1e-300 else -300.0)
                    for x, y in wave]
        if wave is None:
            return None
        return 20 * math.log10(abs(wave)) if abs(wave) > 1e-300 else -300.0

    def db(wave):
        return dB20(wave)

    def mag(wave):
        if isinstance(wave, list):
            return [(x, abs(y)) for x, y in wave]
        return abs(wave) if wave is not None else None

    def phase_deg(c):
        return math.degrees(math.atan2(c.imag, c.real))

    def phaseDeg(wave):
        if isinstance(wave, list):
            return [(x, phase_deg(y) if isinstance(y, complex) else y) for x, y in wave]
        return phase_deg(wave) if isinstance(wave, complex) else wave

    def phaseDegUnwrapped(wave):
        # Unwrap phase: remove 360° jumps
        if not isinstance(wave, list):
            return phaseDeg(wave)
        degrees = [(x, phase_deg(y) if isinstance(y, complex) else y) for x, y in wave]
        unwrapped = []
        prev = None
        offset = 0.0
        for x, d in degrees:
            if prev is not None:
                diff = d - prev
                if diff > 180:
                    offset -= 360
                elif diff < -180:
                    offset += 360
            unwrapped.append((x, d + offset))
            prev = d
        return unwrapped

    def ymax(wave):
        if isinstance(wave, list):
            vals = [abs(y) for _, y in wave]
            return max(vals) if vals else None
        return wave

    def ymin(wave):
        if isinstance(wave, list):
            vals = [abs(y) for _, y in wave]
            return min(vals) if vals else None
        return wave

    def bandwidth(wave, db_drop=3, direction='low', start=None):
        """Find -db_drop dB bandwidth from the peak."""
        if not isinstance(wave, list):
            return None
        db_wave = dB20(wave)
        peak = max(y for _, y in db_wave)
        threshold = peak - db_drop
        # Find first crossing below threshold
        for i in range(len(db_wave) - 1):
            x0, y0 = db_wave[i]
            x1, y1 = db_wave[i + 1]
            if y0 >= threshold >= y1:
                # Linear interpolation
                if y0 != y1:
                    frac = (threshold - y0) / (y1 - y0)
                    return x0 + frac * (x1 - x0)
                return x0
        return db_wave[-1][0]  # beyond sweep

    def slewRate(wave, *args, **kwargs):
        """Slew rate: approximate as max dV/dt."""
        if not isinstance(wave, list):
            return None
        if len(wave) < 2:
            return None
        rates = [(abs(wave[i+1][1] - wave[i][1]) / max(wave[i+1][0] - wave[i][0], 1e-30))
                 for i in range(len(wave) - 1)]
        return max(rates) if rates else None

    return {
        'getData': getData,
        'VF': VF,
        'VT': VT,
        'dB20': dB20,
        'db': db,
        'mag': mag,
        'phase': phaseDeg,
        'phaseDeg': phaseDeg,
        'phaseDegUnwrapped': phaseDegUnwrapped,
        'ymax': ymax,
        'ymin': ymin,
        'bandwidth': bandwidth,
        'slewRate': slewRate,
        'math': math,
        'abs': abs,
        'nil': None,
    }


def eval_expression(expr_raw, psf_dir, psf_tool):
    """
    Evaluate one Maestro output expression against PSF data.
    Returns a scalar, waveform summary, or error string.
    """
    psf_cache = {}

    def load_fn(analysis):
        return load_psf(psf_dir, analysis, psf_tool)

    psf_cache['_load_fn'] = load_fn
    env = make_ocean_env(psf_cache, psf_tool)

    # Transform Ocean syntax to Python
    expr_py = ocean_to_python(expr_raw)

    try:
        result = eval(expr_py, {"__builtins__": {}}, env)  # noqa: S307
    except Exception as e:
        return {'error': str(e), 'expression': expr_raw}

    return summarize_result(result)


def summarize_result(result):
    """Convert a waveform or scalar to a JSON-serializable dict."""
    if result is None:
        return {'value': None}

    if isinstance(result, (int, float)):
        return {'value': result}

    if isinstance(result, complex):
        m = abs(result)
        return {
            'value': m,
            'real': result.real,
            'imag': result.imag,
            'dB': 20 * math.log10(m) if m > 1e-300 else -300.0,
            'phase_deg': math.degrees(math.atan2(result.imag, result.real)),
        }

    if isinstance(result, list) and result:
        # Waveform: [(x, y), ...]
        x_vals = [p[0] for p in result]
        y_vals = [p[1] for p in result]
        real_y = [abs(y) for y in y_vals]

        peak_idx = real_y.index(max(real_y))
        return {
            'type': 'waveform',
            'points': len(result),
            'x_range': [x_vals[0], x_vals[-1]],
            'peak': {'x': x_vals[peak_idx], 'y': real_y[peak_idx]},
            'peak_dB': 20 * math.log10(real_y[peak_idx]) if real_y[peak_idx] > 1e-300 else -300.0,
            'at_x0': summarize_result(y_vals[0])['value'] if y_vals else None,
        }

    return {'value': str(result)}


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def find_psf_dir(psf_base, test_name, corner_idx=1):
    """Locate the actual PSF directory under psf_base."""
    # Standard layout: psf_base / corner_idx / test_name / psf /
    candidate = Path(psf_base) / str(corner_idx) / test_name / 'psf'
    if candidate.exists():
        return str(candidate)

    # Sometimes under psf_base / test_name / psf /
    candidate2 = Path(psf_base) / test_name / 'psf'
    if candidate2.exists():
        return str(candidate2)

    # psf_base itself might be the psf dir
    if (Path(psf_base) / 'dc.dc').exists() or (Path(psf_base) / 'ac.ac').exists():
        return str(psf_base)

    return None


def main():
    parser = argparse.ArgumentParser(description='Read Maestro outputs from PSF files')
    parser.add_argument('maestro_dir', help='Path to maestro directory (contains maestro.sdb)')
    parser.add_argument('--run', help='Run name (e.g. ExplorerRun.0); default: latest')
    parser.add_argument('--test', help='Test name; default: first test')
    parser.add_argument('--list', action='store_true', help='List available runs and exit')
    parser.add_argument('--psf-tool', help='Path to Cadence psf binary')
    args = parser.parse_args()

    maestro_dir = args.maestro_dir

    # Find psf tool
    psf_tool = args.psf_tool or find_psf_tool()
    if not psf_tool:
        print(json.dumps({'error': 'Cadence psf tool not found. Check PSF_TOOL_CANDIDATES or use --psf-tool'}))
        sys.exit(1)

    # Parse maestro files
    sdb = parse_maestro_sdb(maestro_dir)
    outputs = parse_active_state(maestro_dir)
    runs = parse_history_sdb(maestro_dir)

    if args.list:
        print(json.dumps({
            'tests': sdb.get('tests', []),
            'runs': runs,
            'outputs': [{'name': o['name'], 'expression': o['expression']} for o in outputs],
        }, indent=2))
        return

    # Select run
    if args.run:
        run = next((r for r in runs if r['name'] == args.run), None)
        if not run:
            print(json.dumps({'error': f"Run '{args.run}' not found", 'available': [r['name'] for r in runs]}))
            sys.exit(1)
    else:
        # Use latest (last in history)
        run = runs[-1] if runs else None
        if not run:
            # Fall back to overwritehistoryname from maestro.sdb
            hist_name = sdb.get('overwrite_history_name', 'ExplorerRun.0')
            # Reconstruct psf_base from maestro_dir convention
            sim_results = Path(maestro_dir) / 'results' / 'maestro' / hist_name
            run = {'name': hist_name, 'psf_base': str(sim_results), 'timestamp': ''}

    # Select test
    tests = sdb.get('tests', [])
    if args.test:
        test = next((t for t in tests if t['name'] == args.test), None)
        if not test:
            print(json.dumps({'error': f"Test '{args.test}' not found", 'available': [t['name'] for t in tests]}))
            sys.exit(1)
    else:
        test = tests[0] if tests else {'name': ''}

    test_name = test.get('name', '')

    # Locate PSF directory
    psf_dir = find_psf_dir(run['psf_base'], test_name)
    if not psf_dir:
        print(json.dumps({
            'error': 'PSF directory not found',
            'psf_base': run['psf_base'],
            'test': test_name,
        }))
        sys.exit(1)

    # Check simulation completed
    eval_done = Path(psf_dir).parent / '.evalDone'
    completed = eval_done.exists() or any(Path(psf_dir).glob('*.dc')) or any(Path(psf_dir).glob('*.ac'))

    if not completed:
        print(json.dumps({'error': 'Simulation not completed or PSF not found', 'psf_dir': psf_dir}))
        sys.exit(1)

    # Evaluate outputs
    results = []
    for out in outputs:
        evaluated = eval_expression(out['expression'], psf_dir, psf_tool)
        results.append({
            'name': out['name'],
            'expression': out['expression'],
            'eval_type': out['eval_type'],
            **evaluated,
        })

    print(json.dumps({
        'test': test_name,
        'run': run['name'],
        'timestamp': run.get('timestamp', ''),
        'psf_dir': psf_dir,
        'outputs': results,
    }, indent=2))


if __name__ == '__main__':
    main()
