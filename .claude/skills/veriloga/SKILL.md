---
name: veriloga
description: Design, write, and debug Verilog-A behavioral models for Cadence Virtuoso/Spectre simulation. Use when creating Verilog-A modules (voltage sources, behavioral models, testbench stimuli, ideal components), debugging Spectre simulation errors with Verilog-A, or when the user mentions veriloga, behavioral model, or ideal component.
argument-hint: [module type to build or error to debug, e.g. "LDO model", "comparator", "convergence error"]
allowed-tools: Bash(virtuoso *) Bash(vcli *) Bash(spectre *) Read Write Edit
---

# Verilog-A Design & Debug

Write Verilog-A behavioral models and debug them in Virtuoso/Spectre.

**If `$ARGUMENTS` names a module type** (e.g. "LDO", "comparator", "VCO", "PFD", "S/H"), or if the user asks for a specific template, read the reference file first:

```
Read ${CLAUDE_SKILL_DIR}/reference.md
```

Available templates in reference.md: vsrc (1), cmirror (2), opamp (3), bandgap (4), tb_stimulus (5), comparator (6), RLC (7), VCCS/VCVS (8), LDO (9), PFD (10), VCO (11), S/H (12).

## Quick Start

```bash
# Compile a .va file into a Virtuoso cellview
virtuoso skill exec 'ahdlCompile("myLib" "myModel" "veriloga")'

# Check compilation log
virtuoso skill exec 'ahdlGetLog("myLib" "myCell" "veriloga")'

# Standalone: Spectre reads .va directly (no Virtuoso needed)
spectre /tmp/tb.scs -format psfascii -raw /tmp/psf +mt
tail -2 /tmp/psf/spectre.out   # confirm "0 errors"
```

## Language Reference

### Key Analog Operators

| Operator | Usage | Description |
|----------|-------|-------------|
| `V(p,n)` | Access/Contribute | Voltage between nodes |
| `I(p,n)` | Access/Contribute | Current branch |
| `<+` | Contribute | Analog contribution |
| `ddt(x)` | Time derivative | d/dt |
| `idt(x,ic)` | Time integral | ∫dt with initial condition |
| `laplace_nd(x,n,d)` | Transfer function | N(s)/D(s) |
| `transition(x,td,tr,tf)` | Smooth transition | With delay, rise, fall |
| `slew(x,sr+,sr-)` | Slew rate limit | |
| `absdelay(x,td)` | Pure delay | td must be constant |
| `limexp(x)` | Limited exponential | Convergence-safe exp() |
| `white_noise(pwr)` | White noise | Power spectral density |
| `flicker_noise(pwr,exp)` | 1/f noise | |
| `$temperature` | System | Temperature in Kelvin |
| `$abstime` | System | Absolute simulation time |
| `$vt` | System | Thermal voltage kT/q |

### Events, Time-Step Control, Debug

| Function | Usage | Notes |
|----------|-------|-------|
| `cross(expr, dir)` | Edge detect | `dir=+1/-1/0` rising/falling/both |
| `timer(t0, period)` | Timed event | `period=0` = single-shot |
| `initial_step` | Event | First sim step — init variables |
| `final_step` | Event | Last sim step — print results |
| `bound_step(dt)` | Step limit | Force step ≤ dt |
| `$discontinuity(n)` | Notify discontinuity | `n=0` force reconvergence |
| `$simparam("name", def)` | Read sim param | Get temperature, freq from Ocean |
| `$strobe("fmt", args)` | Debug print | Per-step printf |

```verilog
// Edge detection → record delay
@(cross(V(out) - 0.5*V(vdd), +1))
  $strobe("rising at t=%e", $abstime);

// Force fine steps during fast edge
bound_step(tr / 5);

// Read simulation temperature
real tc;
tc = $simparam("temperature", 27.0);
```

### Constants (`constants.vams`)

```
`M_PI    3.14159265...
`P_K     1.3806226e-23    // Boltzmann (J/K)
`P_Q     1.6021918e-19    // Electron charge (C)
`P_EPS0  8.8541878e-12    // Permittivity (F/m)
```

### Parameter Types

```verilog
parameter real    r = 1e3  from (0:inf);
parameter integer n = 4    from [1:16];
parameter real    v = 0.0  from [-10:10];
```

## Convergence Best Practices

```verilog
// BAD: discontinuous if/else on analog signal
if (V(inp) > V(inn))  V(out) <+ V(vdd);
else                  V(out) <+ V(vss);

// GOOD: tanh smooth transition (continuous derivative)
V(out) <+ V(vss) + (V(vdd)-V(vss)) * (tanh(1000*(V(inp)-V(inn))) + 1) / 2;

// BAD: exp() overflows
I(d,s) <+ Is * (exp(V(d,s)/$vt) - 1);

// GOOD: limexp() clamps
I(d,s) <+ Is * (limexp(V(d,s)/$vt) - 1);
```

### Soft Clamp (continuous min/max)

```verilog
// soft-min(a, b) — ε=1e-6 controls sharpness
real soft_min;
soft_min = (a + b - sqrt((a-b)*(a-b) + 1e-6)) / 2.0;

// soft-max(0, x) — ReLU approximation
real soft_relu;
soft_relu = (x + sqrt(x*x + 1e-6)) / 2.0;
```

### Thevenin Output (implicit equation)

```verilog
// V_out = V_oc - I_load * Rout — Spectre solves implicitly
V(out, gnd) <+ vout_oc - I(out, gnd) * rout;
```

### laplace_nd Coefficient Order

```verilog
// H(s) = N(s)/D(s), coefficients in ascending power of s
// Single-pole low-pass:  H(s) = gain / (1 + s/ωp)
laplace_nd(x, {gain}, {1, 1.0/(2*`M_PI*fp)})

// PSRR high-pass (good DC, degrades at high freq):
// H(s) = (k + s·τ) / (1 + s·τ)
vripple = laplace_nd(vin_ripple, {psrr_lin, tau_p}, {1.0, tau_p});
```

## Debugging

### Common Errors

| Error | Cause | Fix |
|-------|-------|-----|
| `Undefined variable` | Missing `include` | Add `` `include "disciplines.vams" `` |
| `Contribution to non-branch` | Wrong LHS of `<+` | Use `V(p,n) <+` not `V(p) <+` |
| `Convergence failure` | Discontinuous function | Use `transition()`, `limexp()`, avoid `if` on analog |
| `Time step too small` | Sharp discontinuity | Add `transition()` with rise/fall time |
| `Multiple contributions` | Two `<+` to same branch | Combine into single expression |

### Debug Commands

```bash
virtuoso skill exec 'ahdlCompile("myLib" "myCell" "veriloga")'
virtuoso skill exec 'ahdlGetLog("myLib" "myCell" "veriloga")'

# Spectre convergence options
virtuoso skill exec 'option(quote(spectre) quote(reltol) 1e-4)'
virtuoso skill exec 'option(quote(spectre) quote(gmin) 1e-14)'
```

## End-to-End Flow (standalone spectre)

```bash
# Write .va → standalone testbench → simulate → check
cat > /tmp/model.va << 'EOF'
`include "constants.vams"
`include "disciplines.vams"
// ... module code ...
EOF

cat > /tmp/tb.scs << 'EOF'
simulator lang=spectre
ahdl_include "/tmp/model.va"   // Spectre compiles .va directly
// ... netlist ...
dcop dc
tran1 tran stop=100n
EOF

spectre /tmp/tb.scs -format psfascii -raw /tmp/psf +mt
tail -2 /tmp/psf/spectre.out        # check "0 errors"
awk '/^V\(out\)/{print}' /tmp/psf/dcop.dc
```

`ahdl_include` = standalone Spectre reads .va directly; `ahdlCompile()` = Virtuoso cellview mode.
