#!/usr/bin/env python3
"""
Bandgap parameter sweep: parse spec YAML → expand combos → run Spectre → save results.

Usage:
    python run_bandgap_sweep.py run --spec bandgap.yaml --netlist template.scs [--timeout 600] [--corner tt]
    python run_bandgap_sweep.py status bg-a3c4f9
    python run_bandgap_sweep.py report bg-a3c4f9 [--output report.md]

State stored in: ~/.cache/virtuoso_bridge/optim/<id>.json
"""

import argparse
import itertools
import json
import os
import re
import subprocess
import sys
import uuid
from pathlib import Path

try:
    import yaml
except ImportError:
    print('{"error": "PyYAML not installed — run: pip install pyyaml"}', file=sys.stderr)
    sys.exit(1)


# ── Spec parsing ──────────────────────────────────────────────────────────────

def load_spec(path: str) -> dict:
    spec = yaml.safe_load(Path(path).read_text())
    _validate_spec(spec)
    return spec


def _validate_spec(spec: dict) -> None:
    target = spec.get("target", {})
    if "Vbg" not in target:
        raise ValueError("spec.target.Vbg is required")
    for name, r in spec.get("params", {}).items():
        if r["min"] <= 0 or r["max"] <= 0:
            raise ValueError(f"param '{name}': values must be positive")
        if r["min"] > r["max"]:
            raise ValueError(f"param '{name}': min ({r['min']:.3e}) > max ({r['max']:.3e})")


def param_combos(params: dict) -> list:
    if not params:
        return [{}]
    names = list(params.keys())
    ranges = []
    for r in params.values():
        steps = round((r["max"] - r["min"]) / r["step"])
        ranges.append([r["min"] + r["step"] * i for i in range(steps + 1)])
    return [dict(zip(names, combo)) for combo in itertools.product(*ranges)]


# ── Netlist template ──────────────────────────────────────────────────────────

def render_template(template: str, params: dict) -> str:
    result = template
    for key, val in params.items():
        placeholder = f"${{{key}}}"
        if placeholder not in result:
            raise ValueError(f"template missing placeholder for param '{key}'")
        result = result.replace(placeholder, f"{val:.6e}")
    m = re.search(r'\$\{[^}]+\}', result)
    if m:
        raise ValueError(f"unresolved placeholder in template: {m.group()}")
    return result


# ── Spectre execution ─────────────────────────────────────────────────────────

def run_spectre(netlist: str, workdir: Path, timeout: int) -> dict:
    workdir.mkdir(parents=True, exist_ok=True)
    psf_dir = workdir / "psf"
    psf_dir.mkdir(exist_ok=True)
    netlist_path = workdir / "input.scs"
    netlist_path.write_text(netlist)

    cmd = [
        "spectre", str(netlist_path),
        "+aps", "++aps",
        "-raw", str(psf_dir),
        "+log", str(psf_dir / "spectre.out"),
        "-format", "psfxl",
    ]
    try:
        subprocess.run(cmd, capture_output=True, timeout=timeout, cwd=workdir)
    except subprocess.TimeoutExpired:
        return {"status": "failed", "raw_dir": None, "error": f"timeout after {timeout}s"}
    except FileNotFoundError:
        return {"status": "failed", "raw_dir": None, "error": "spectre not found in PATH"}

    log_path = psf_dir / "spectre.out"
    log = log_path.read_text() if log_path.exists() else ""
    if "completes with 0 errors" in log:
        return {"status": "completed", "raw_dir": str(psf_dir), "error": None}
    tail = log[-500:] if len(log) > 500 else log
    return {"status": "failed", "raw_dir": str(psf_dir), "error": tail or "no log"}


# ── State persistence ─────────────────────────────────────────────────────────

def _optim_dir() -> Path:
    cache = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    d = cache / "virtuoso_bridge" / "optim"
    d.mkdir(parents=True, exist_ok=True)
    return d


def _save_state(state: dict) -> None:
    (_optim_dir() / f"{state['optim_id']}.json").write_text(json.dumps(state, indent=2))


def _load_state(optim_id: str) -> dict:
    path = _optim_dir() / f"{optim_id}.json"
    if not path.exists():
        raise FileNotFoundError(f"optim job '{optim_id}' not found")
    return json.loads(path.read_text())


# ── Commands ──────────────────────────────────────────────────────────────────

def cmd_run(spec_file: str, netlist_file: str, timeout: int, corner_override: str | None) -> dict:
    spec = load_spec(spec_file)
    if corner_override:
        spec["corner"] = corner_override
    corner = spec.get("corner", "tt")

    template = Path(netlist_file).read_text()
    combos = param_combos(spec.get("params", {}))
    if not combos:
        raise ValueError("spec produces no parameter combinations")

    optim_id = f"bg-{uuid.uuid4().hex[:6]}"
    workbase = _optim_dir() / optim_id

    jobs = []
    total = len(combos)
    for i, params in enumerate(combos):
        netlist = render_template(template, params)
        workdir = workbase / f"job_{i:03d}"
        result = run_spectre(netlist, workdir, timeout)
        job = {"job_idx": i, "params": params, **result}
        jobs.append(job)
        done = sum(1 for j in jobs if j["status"] == "completed")
        print(f"  [{i+1}/{total}] {params} → {result['status']}", file=sys.stderr)

    completed = sum(1 for j in jobs if j["status"] == "completed")
    failed = len(jobs) - completed
    status = "completed" if failed == 0 else ("failed" if completed == 0 else "partial")

    best_job = next((j for j in jobs if j["status"] == "completed"), None)
    best = ({"iteration": 1, "params": best_job["params"], "raw_dir": best_job["raw_dir"]}
            if best_job else None)

    state = {
        "optim_id": optim_id,
        "spec_file": spec_file,
        "netlist_path": netlist_file,
        "corner": corner,
        "status": status,
        "iteration": 1,
        "jobs": jobs,
        "best": best,
    }
    _save_state(state)

    return {
        "optim_id": optim_id,
        "spec_file": spec_file,
        "corner": corner,
        "iteration": 1,
        "status": status,
        "total_jobs": len(jobs),
        "completed": completed,
        "failed": failed,
        "best": best,
    }


def cmd_status(optim_id: str) -> dict:
    state = _load_state(optim_id)
    completed = sum(1 for j in state["jobs"] if j["status"] == "completed")
    failed = len(state["jobs"]) - completed
    return {
        "optim_id": optim_id,
        "status": state["status"],
        "iteration": state["iteration"],
        "corner": state["corner"],
        "total_jobs": len(state["jobs"]),
        "completed": completed,
        "failed": failed,
        "best": state["best"],
    }


def cmd_report(optim_id: str, output: str | None) -> dict:
    state = _load_state(optim_id)
    lines = [
        "# Bandgap Optimization Report\n",
        f"**Optim ID:** `{state['optim_id']}`  ",
        f"**Spec:** {state['spec_file']}  ",
        f"**Corner:** {state['corner']}  ",
        f"**Status:** {state['status']}  \n",
        "## Iteration Summary\n",
        f"Iteration {state['iteration']} — {len(state['jobs'])} jobs total\n",
        "## Parameter Sweep Results\n",
        "| Status | Params | Raw Dir |",
        "|--------|--------|--------|",
    ]
    for j in state["jobs"]:
        params_str = ", ".join(f"{k}={v:.3e}" for k, v in j["params"].items())
        raw = j.get("raw_dir") or "-"
        lines.append(f"| {j['status']} | {params_str} | {raw} |")

    if state["best"]:
        b = state["best"]
        lines += ["\n## Best Result\n", f"**Iteration:** {b['iteration']}  "]
        for k, v in b["params"].items():
            lines.append(f"**{k}:** {v:.3e}  ")
        lines.append(f"**Raw dir:** `{b['raw_dir']}`  ")

    md = "\n".join(lines) + "\n"
    if output:
        Path(output).write_text(md)
        return {"written": output, "optim_id": optim_id}
    return {"report": md, "optim_id": optim_id}


# ── CLI ───────────────────────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(description="Bandgap parameter sweep")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_run = sub.add_parser("run")
    p_run.add_argument("--spec", required=True)
    p_run.add_argument("--netlist", required=True)
    p_run.add_argument("--timeout", type=int, default=600)
    p_run.add_argument("--corner")

    p_st = sub.add_parser("status")
    p_st.add_argument("id")

    p_rp = sub.add_parser("report")
    p_rp.add_argument("id")
    p_rp.add_argument("--output", "-o")

    args = parser.parse_args()
    try:
        if args.cmd == "run":
            result = cmd_run(args.spec, args.netlist, args.timeout, args.corner)
        elif args.cmd == "status":
            result = cmd_status(args.id)
        else:
            result = cmd_report(args.id, args.output)
        print(json.dumps(result, indent=2))
    except (ValueError, FileNotFoundError) as e:
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
