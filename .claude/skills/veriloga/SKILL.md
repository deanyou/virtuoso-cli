---
name: veriloga
description: Design, write, and debug Verilog-A behavioral models for Cadence Virtuoso/Spectre simulation. Use when creating Verilog-A modules (voltage sources, behavioral models, testbench stimuli, ideal components), debugging Spectre simulation errors with Verilog-A, or when the user mentions veriloga, behavioral model, or ideal component.
argument-hint: '[module type to build or error to debug, e.g. "LDO model", "comparator", "convergence error"]'
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
VCLI_CAPABILITY=admin vcli skill exec 'ahdlCompile("myLib" "myModel" "veriloga")'

# Check compilation log
VCLI_CAPABILITY=admin vcli skill exec 'ahdlGetLog("myLib" "myCell" "veriloga")'

# Standalone: Spectre reads .va directly (no Virtuoso needed)
spectre /tmp/tb.scs -format psfascii -raw /tmp/psf +mt
tail -2 /tmp/psf/spectre.out   # confirm "0 errors"
```

> `VCLI_CAPABILITY=admin` 用**单命令前缀**，不要 `export`——export 会把 admin 能力留在
> 后续每一条命令的环境里，失去最小权限。裸 SKILL exec 需要 admin 能力，
> 所以每个 `vcli skill exec` 都要带上这个前缀。

### 生成 symbol：缺 symbol 时不要直接 `ahdlSaveFile`

`ahdlSaveFile` 在目标 cellview **还没有 symbol view** 时，会弹出一个模态
"Save Symbol" 对话框，要求你确认/编辑 pin。这个弹窗是 **GUI 模态**：它会把
SKILL channel **卡死**——命令既不返回也不报错，daemon 侧只表现为超时，
而不是一个清晰的失败。

**规则：缺 symbol 时不要直接 `ahdlSaveFile`。** 先用非交互的方式把 symbol 建出来，
再保存（此时 symbol 已存在，保存走"已存在"分支，不再弹框）：

```bash
# 1) 先非交互地由 schematic pin 列表生成 symbol
VCLI_CAPABILITY=admin vcli skill exec \
  'schPinListToSymbol("myLib" "myCell" "symbol" schSchemToPinList("myLib" "myCell" "schematic"))'

# 2) 再保存（symbol 已存在，不会再弹模态框）
VCLI_CAPABILITY=admin vcli skill exec 'ahdlSaveFile("myLib" "myCell" "veriloga")'
```

`schPinListToSymbol` 或等价的非交互 symbol 生成函数均可；关键是**先建后存**。
如果确实必须走"新建"分支，请在有 GUI 的人旁边执行，不要把它放进无人值守的
SKILL 通道。

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
| `bound_step(dt)` | Step limit | Force step ≤ dt — **not supported in SPECTRE231 (IC23.1)** |
| `$discontinuity(n)` | Notify discontinuity | `n=0` force reconvergence |
| `$simparam("name", def)` | Read sim param | Get temperature, freq from Ocean |
| `$strobe("fmt", args)` | Debug print | Per-step printf |

```verilog
// Edge detection → record delay
@(cross(V(out) - 0.5*V(vdd), +1))
  $strobe("rising at t=%e", $abstime);

// Force fine steps during fast edge (IC25+ only; omit on IC23.1/SPECTRE231)
// bound_step(tr / 5);

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
VCLI_CAPABILITY=admin vcli skill exec 'ahdlCompile("myLib" "myCell" "veriloga")'
VCLI_CAPABILITY=admin vcli skill exec 'ahdlGetLog("myLib" "myCell" "veriloga")'

# Spectre convergence options
VCLI_CAPABILITY=admin vcli skill exec 'option(quote(spectre) quote(reltol) 1e-4)'
VCLI_CAPABILITY=admin vcli skill exec 'option(quote(spectre) quote(gmin) 1e-14)'
```

## End-to-End Flow (standalone spectre)

```bash
# Write .va → standalone testbench → simulate → check
cat > /tmp/model.va << 'EOF'
`include "constants.vams"
`include "disciplines.vams"
module beh_comp(inp, inn, out, vdd, vss);
  inout inp, inn, out, vdd, vss;  electrical inp, inn, out, vdd, vss;
  parameter real gain = 1e4; parameter real td = 1e-9; parameter real tr = 200e-12;
  real vd;
  analog begin
    vd = V(inp, inn);
    V(out, vss) <+ transition((tanh(gain*vd/V(vdd,vss))+1)/2*V(vdd,vss)+V(vss), td, tr, tr);
    bound_step(tr / 5);
  end
endmodule
EOF

cat > /tmp/tb.scs << 'EOF'
simulator lang=spectre
ahdl_include "/tmp/model.va"
parameters vdd=1.2 vcm=0.6 vdiff=10m
Vvdd (vdd 0) vsource dc=vdd
Vcm  (inp 0) vsource dc=vcm
Vd   (inp inn) vsource dc=vdiff ac=1
xcomp (inp inn out vdd 0) beh_comp
dcop dc
ac1  ac start=1 stop=1G dec=20
EOF

spectre /tmp/tb.scs -format psfascii -raw /tmp/psf +mt
tail -2 /tmp/psf/spectre.out        # check "0 errors"
awk '/^V\(out\)/{print}' /tmp/psf/dcop.dc
```

`ahdl_include` = standalone Spectre reads .va directly; `ahdlCompile()` = Virtuoso cellview mode.
