#!/usr/bin/env python3
"""
plot_sim.py — Auto-detect and plot virtuoso-cli simulation output.

Reads JSON from stdin or --input file. Detects chart type from data structure.
Saves chart to --output path (default: sim_plot.png).

Chart types:
  sweep   : sim sweep output  → multi-line plot (sweep var on x)
  corner  : sim corner output → grouped bar chart (corners on x)
  measure : sim measure output → horizontal bar chart
  bode    : {"freq":[], "mag_db":[], "phase_deg":[]} → Bode plot
  lookup  : process_data nmos/pmos lookup JSON → gm/Id curves
"""

import argparse
import json
import sys
import os

import matplotlib
matplotlib.use("Agg")  # headless, no display needed
import matplotlib.pyplot as plt
import matplotlib.ticker as ticker
import numpy as np


# ──────────────────────────────────────────────
# Chart type detection
# ──────────────────────────────────────────────

def detect_chart_type(data: dict) -> str:
    if "freq" in data and ("mag_db" in data or "magnitude" in data):
        return "bode"
    if "variable" in data and "data" in data and isinstance(data["data"], list):
        return "sweep"
    if "corners" in data and "data" in data and isinstance(data["data"], list):
        return "corner"
    if "measures" in data and isinstance(data["measures"], list):
        return "measure"
    if "data" in data and isinstance(data["data"], list):
        d0 = data["data"][0] if data["data"] else {}
        if "l_values" in data or ("l" in d0 and "points" in d0):
            return "lookup"
    return "unknown"


# ──────────────────────────────────────────────
# Plot functions
# ──────────────────────────────────────────────

def _shorten(expr: str, maxlen: int = 30) -> str:
    return expr if len(expr) <= maxlen else "..." + expr[-(maxlen - 3):]


def plot_sweep(data: dict, ax: plt.Axes, title: str):
    var = data["variable"]
    rows = data["data"]
    headers = data.get("headers", [])
    measure_cols = [h for h in headers if h != var] if headers else []
    if not measure_cols and rows:
        measure_cols = [k for k in rows[0].keys() if k != var]

    try:
        xs = [float(r[var]) for r in rows]
    except (ValueError, KeyError):
        xs = list(range(len(rows)))

    # Scale x-axis to µm if it looks like a length (< 1e-3)
    x_scale, x_unit = 1.0, var
    if xs and max(abs(v) for v in xs) < 1e-3 and max(abs(v) for v in xs) > 0:
        x_scale, x_unit = 1e6, f"{var} (µm)"

    for col in measure_cols:
        try:
            ys = [float(r[col]) for r in rows]
        except (ValueError, KeyError):
            continue
        ax.plot([x * x_scale for x in xs], ys, marker="o", label=_shorten(col))

    ax.set_xlabel(x_unit)
    ax.set_ylabel("Value")
    ax.set_title(title or f"Sweep: {var}")
    ax.legend(fontsize=8)
    ax.grid(True, alpha=0.3)


def plot_corner(data: dict, fig: plt.Figure, title: str):
    rows = data["data"]
    headers = data.get("headers", [])
    if not headers and rows:
        headers = list(rows[0].keys())

    meta_cols = {"corner", "temp"}
    measure_cols = [h for h in headers if h.lower() not in meta_cols]
    corner_labels = [str(r.get("corner", i)) for i, r in enumerate(rows)]

    n_corners = len(rows)
    n_measures = len(measure_cols)
    if n_measures == 0:
        return

    x = np.arange(n_corners)
    width = 0.8 / n_measures
    ax = fig.add_subplot(1, 1, 1)

    for i, col in enumerate(measure_cols):
        try:
            vals = [float(r[col]) for r in rows]
        except (ValueError, KeyError):
            continue
        ax.bar(x + i * width, vals, width, label=_shorten(col))

    ax.set_xticks(x + width * (n_measures - 1) / 2)
    ax.set_xticklabels(corner_labels, rotation=20, ha="right")
    ax.set_ylabel("Value")
    ax.set_title(title or "Corner Analysis")
    ax.legend(fontsize=8)
    ax.grid(True, axis="y", alpha=0.3)


def plot_measure(data: dict, ax: plt.Axes, title: str):
    measures = data["measures"]
    labels, values = [], []
    for m in measures:
        v = m.get("value", "nil")
        if v == "nil" or v is None:
            continue
        try:
            values.append(float(v))
            labels.append(_shorten(m.get("expr", "?"), 25))
        except ValueError:
            pass

    if not values:
        ax.text(0.5, 0.5, "No numeric values", ha="center", va="center",
                transform=ax.transAxes)
        return

    colors = ["#2196F3" if v >= 0 else "#F44336" for v in values]
    ax.barh(range(len(labels)), values, color=colors)
    ax.set_yticks(range(len(labels)))
    ax.set_yticklabels(labels, fontsize=8)
    ax.set_xlabel("Value")
    ax.set_title(title or "Measurements")
    ax.grid(True, axis="x", alpha=0.3)
    ax.invert_yaxis()


def plot_bode(data: dict, fig: plt.Figure, title: str):
    freq = np.array(data["freq"])
    mag = np.array(data.get("mag_db", data.get("magnitude", [])))
    phase = np.array(data.get("phase_deg", data.get("phase", [])))

    ax1 = fig.add_subplot(2, 1, 1)
    ax2 = fig.add_subplot(2, 1, 2, sharex=ax1)

    ax1.semilogx(freq, mag, color="#1565C0", linewidth=1.5)
    ax1.set_ylabel("Magnitude (dB)")
    ax1.set_title(title or "Bode Plot")
    ax1.grid(True, which="both", alpha=0.3)
    ax1.axhline(0, color="gray", linestyle="--", linewidth=0.8)

    # Mark GBW (0 dB crossing)
    try:
        gbw_idx = np.where(np.diff(np.sign(mag)))[0]
        if len(gbw_idx) > 0:
            i = gbw_idx[0]
            gbw = np.interp(0, [mag[i + 1], mag[i]], [freq[i + 1], freq[i]])
            ax1.axvline(gbw, color="red", linestyle=":", alpha=0.7,
                        label=f"GBW={gbw/1e6:.1f}MHz")
            ax1.legend(fontsize=8)
    except Exception:
        pass

    if len(phase) > 0:
        ax2.semilogx(freq, phase, color="#E65100", linewidth=1.5)
        ax2.set_ylabel("Phase (°)")
        ax2.set_xlabel("Frequency (Hz)")
        ax2.grid(True, which="both", alpha=0.3)
        ax2.axhline(-180, color="gray", linestyle="--", linewidth=0.8)

        # Mark phase margin
        try:
            gbw_idx = np.where(np.diff(np.sign(mag)))[0]
            if len(gbw_idx) > 0:
                i = gbw_idx[0]
                gbw = np.interp(0, [mag[i + 1], mag[i]], [freq[i + 1], freq[i]])
                pm_phase = np.interp(gbw, freq, phase)
                pm = pm_phase + 180
                ax2.axvline(gbw, color="red", linestyle=":", alpha=0.7,
                            label=f"PM={pm:.1f}°")
                ax2.legend(fontsize=8)
        except Exception:
            pass

    fig.tight_layout()


def plot_lookup(data: dict, fig: plt.Figure, title: str):
    """Plot gm/Id lookup table: gain vs gm/Id for each L value."""
    device = data.get("device", "device")
    l_data = data.get("data", [])

    ax1 = fig.add_subplot(2, 2, 1)
    ax2 = fig.add_subplot(2, 2, 2)
    ax3 = fig.add_subplot(2, 2, 3)
    ax4 = fig.add_subplot(2, 2, 4)

    for entry in l_data:
        l_val = entry.get("l", 0)
        points = entry.get("points", [])
        if not points:
            continue
        label = f"L={l_val*1e9:.0f}n"
        gmid = [p.get("gmid", p.get("gm_id", 0)) for p in points]
        gain = [p.get("gain", 0) for p in points]
        gain_db = [p.get("gain_db", 20 * np.log10(abs(g)) if g > 0 else 0) for p in points]
        ft = [p.get("ft", p.get("fT", 0)) / 1e9 for p in points]  # GHz
        id_val = [abs(p.get("id", p.get("Id", 0))) * 1e6 for p in points]  # µA/µm

        ax1.plot(gmid, gain_db, marker=".", label=label)
        ax2.plot(gmid, ft, marker=".", label=label)
        ax3.plot(gmid, id_val, marker=".", label=label)
        ax4.semilogy(gmid, id_val, marker=".", label=label)

    for ax, ylabel, ttl in [
        (ax1, "Gain (dB)", "Gain vs gm/Id"),
        (ax2, "fT (GHz)", "fT vs gm/Id"),
        (ax3, "Id (µA/µm)", "Id vs gm/Id"),
        (ax4, "Id (µA/µm) log", "Id log vs gm/Id"),
    ]:
        ax.set_xlabel("gm/Id (S/A)")
        ax.set_ylabel(ylabel)
        ax.set_title(ttl)
        ax.legend(fontsize=7)
        ax.grid(True, alpha=0.3)

    fig.suptitle(title or f"{device} gm/Id Lookup", fontsize=11)
    fig.tight_layout()


# ──────────────────────────────────────────────
# Main
# ──────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Plot virtuoso-cli simulation results")
    parser.add_argument("--input", "-i", help="JSON input file (default: stdin)")
    parser.add_argument("--output", "-o", default="sim_plot.png", help="Output PNG path")
    parser.add_argument("--title", "-t", default="", help="Chart title override")
    parser.add_argument("--dpi", type=int, default=150, help="Output DPI (default: 150)")
    args = parser.parse_args()

    # Load JSON
    if args.input:
        with open(args.input) as f:
            data = json.load(f)
    else:
        data = json.load(sys.stdin)

    chart_type = detect_chart_type(data)

    plt.style.use("seaborn-v0_8-whitegrid")
    fig = plt.figure(figsize=(10, 6))

    if chart_type == "sweep":
        ax = fig.add_subplot(1, 1, 1)
        plot_sweep(data, ax, args.title)
    elif chart_type == "corner":
        plot_corner(data, fig, args.title)
    elif chart_type == "measure":
        ax = fig.add_subplot(1, 1, 1)
        plot_measure(data, ax, args.title)
    elif chart_type == "bode":
        plot_bode(data, fig, args.title)
    elif chart_type == "lookup":
        plt.close(fig)
        fig = plt.figure(figsize=(12, 9))
        plot_lookup(data, fig, args.title)
    else:
        print(f"Cannot detect chart type. Keys: {list(data.keys())}", file=sys.stderr)
        sys.exit(1)

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    fig.savefig(args.output, dpi=args.dpi, bbox_inches="tight")
    plt.close(fig)
    print(f"Chart saved: {args.output}")


if __name__ == "__main__":
    main()
