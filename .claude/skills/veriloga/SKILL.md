---
name: veriloga
description: Design, write, and debug Verilog-A behavioral models for Cadence Virtuoso/Spectre simulation. Use when creating Verilog-A modules (voltage sources, behavioral models, testbench stimuli, ideal components), debugging Spectre simulation errors with Verilog-A, or when the user mentions veriloga, behavioral model, or ideal component.
allowed-tools: Bash(*/virtuoso *) Read Write Edit
---

# Verilog-A Design & Debug

Write Verilog-A behavioral models and debug them in Virtuoso/Spectre.

## Quick Start: Create and Simulate a Verilog-A Module

```bash
# 1. Write the .va file
# 2. Create a veriloga view in Virtuoso
virtuoso skill exec '
  let((cv)
    cv = dbOpenCellViewByType("myLib" "myModel" "veriloga" "text" "w")
    when(cv
      dbSave(cv)
      printf("created veriloga view: myLib/myModel/veriloga\n")
    )
  )
'

# 3. Or load via ahdlCompile
virtuoso skill exec 'ahdlCompile("myLib" "myModel" "veriloga")'

# 4. Instantiate in schematic and simulate
```

## Verilog-A Module Templates

### 1. Ideal Voltage Source (DC + AC + Pulse)

```verilog
`include "constants.vams"
`include "disciplines.vams"

module ideal_vsrc(p, n);
  inout p, n;
  electrical p, n;

  parameter real vdc = 0.0;        // DC voltage
  parameter real vac = 1.0;        // AC magnitude
  parameter real freq = 1e6;       // Frequency for transient
  parameter real vamp = 0.0;       // Transient amplitude (0=DC only)
  parameter real trise = 1e-9;     // Rise time
  parameter real tfall = 1e-9;     // Fall time
  parameter real tdelay = 0.0;     // Delay
  parameter real twidth = 5e-7;    // Pulse width

  analog begin
    if (vamp == 0.0)
      V(p, n) <+ vdc;
    else
      V(p, n) <+ vdc + vamp * pulse(tdelay, trise, twidth, tfall, 1.0/freq);
  end
endmodule
```

### 2. Ideal Current Mirror (behavioral)

```verilog
`include "constants.vams"
`include "disciplines.vams"

module ideal_cmirror(iin, iout, vdd);
  inout iin, iout, vdd;
  electrical iin, iout, vdd;

  parameter real ratio = 1.0;      // Mirror ratio
  parameter real vsat = 0.2;       // Min output headroom

  real i_ref;

  analog begin
    i_ref = I(vdd, iin);
    if (V(vdd, iout) > vsat)
      I(vdd, iout) <+ ratio * i_ref;
    else
      I(vdd, iout) <+ ratio * i_ref * V(vdd, iout) / vsat;
  end
endmodule
```

### 3. Ideal Opamp (finite gain, GBW, slew)

```verilog
`include "constants.vams"
`include "disciplines.vams"

module ideal_opamp(inp, inn, out, vdd, vss);
  inout inp, inn, out, vdd, vss;
  electrical inp, inn, out, vdd, vss;

  parameter real gain = 1e4;       // DC gain (V/V)
  parameter real gbw = 10e6;       // Gain-bandwidth product (Hz)
  parameter real sr = 10e6;        // Slew rate (V/s)
  parameter real vos = 0.0;        // Input offset voltage
  parameter real rin = 1e12;       // Input resistance
  parameter real rout = 100;       // Output resistance

  real vin_diff, vout_ideal, fp;

  analog begin
    // Input stage
    I(inp, inn) <+ V(inp, inn) / rin;
    vin_diff = V(inp, inn) - vos;

    // Single-pole model: fp = GBW/gain
    fp = gbw / gain;
    vout_ideal = gain * laplace_nd(vin_diff, {1}, {1, 1.0/(2*`M_PI*fp)});

    // Slew rate limiting
    vout_ideal = slew(vout_ideal, sr, sr);

    // Output clamping to rails
    if (vout_ideal > V(vdd) - 0.05)
      vout_ideal = V(vdd) - 0.05;
    else if (vout_ideal < V(vss) + 0.05)
      vout_ideal = V(vss) + 0.05;

    // Output with resistance
    V(out) <+ vout_ideal;
    I(out) <+ V(out) / rout;
  end
endmodule
```

### 4. Bandgap Reference (behavioral)

```verilog
`include "constants.vams"
`include "disciplines.vams"

module bandgap_ref(vref, vdd, gnd);
  inout vref, vdd, gnd;
  electrical vref, vdd, gnd;

  parameter real vref_nom = 1.2;   // Nominal reference voltage
  parameter real tc1 = -10e-6;     // 1st order temp coeff (V/°C)
  parameter real tc2 = 0.1e-6;     // 2nd order temp coeff (V/°C²)
  parameter real psrr_dc = 1e-4;   // PSRR at DC (linear)
  parameter real rout = 1e3;       // Output resistance
  parameter real tnom = 27;        // Nominal temperature

  real dtemp, vref_t;

  analog begin
    dtemp = $temperature - (tnom + 273.15);
    vref_t = vref_nom + tc1 * dtemp + tc2 * dtemp * dtemp;

    // Add VDD dependency (PSRR)
    vref_t = vref_t + psrr_dc * (V(vdd, gnd) - 1.2);

    V(vref, gnd) <+ vref_t;
    I(vref, gnd) <+ V(vref, gnd) / rout;
  end
endmodule
```

### 5. Testbench Stimulus (PWL + Noise)

```verilog
`include "constants.vams"
`include "disciplines.vams"

module tb_stimulus(out, gnd);
  inout out, gnd;
  electrical out, gnd;

  parameter real v_initial = 0.0;
  parameter real v_final = 1.2;
  parameter real t_start = 1e-6;
  parameter real t_ramp = 1e-6;
  parameter real noise_density = 1e-9;  // V/√Hz

  analog begin
    V(out, gnd) <+ transition(
      ($abstime < t_start) ? v_initial : v_final,
      t_start, t_ramp
    );

    // Add white noise
    V(out, gnd) <+ white_noise(noise_density * noise_density, "thermal");
  end
endmodule
```

## Verilog-A Language Reference

### Key Analog Operators

| Operator | Usage | Description |
|----------|-------|-------------|
| `V(p,n)` | Access/Contribute | Voltage between nodes |
| `I(p,n)` | Access/Contribute | Current branch |
| `<+` | Contribute | Analog contribution |
| `ddt(x)` | Time derivative | d/dt |
| `idt(x,ic)` | Time integral | ∫dt with initial condition |
| `ddx(f,x)` | Partial derivative | ∂f/∂x |
| `laplace_nd(x,n,d)` | Transfer function | N(s)/D(s) |
| `zi_nd(x,n,d,T)` | Z-domain filter | N(z)/D(z) |
| `transition(x,td,tr,tf)` | Smooth transition | With delay, rise, fall |
| `slew(x,sr+,sr-)` | Slew rate limit | |
| `absdelay(x,td)` | Pure delay | |
| `limexp(x)` | Limited exponential | Convergence-safe exp() |
| `white_noise(pwr)` | White noise | Power spectral density |
| `flicker_noise(pwr,exp)` | 1/f noise | |
| `$temperature` | System | Temperature in Kelvin |
| `$abstime` | System | Absolute simulation time |
| `$vt` | System | Thermal voltage kT/q |

### Constants (`constants.vams`)

```
`M_PI      3.14159265358979...
`P_K       1.3806226e-23     // Boltzmann (J/K)
`P_Q       1.6021918e-19     // Electron charge (C)
`P_EPS0    8.8541878e-12     // Permittivity (F/m)
```

### Parameter Types

```verilog
parameter real    r = 1e3  from (0:inf);     // Positive real
parameter integer n = 4    from [1:16];       // Bounded integer
parameter real    v = 0.0  from [-10:10];     // Bounded real
parameter string  mode = "normal" from {"normal", "fast"};
```

## Debugging Verilog-A in Spectre

### Common Errors and Fixes

| Error | Cause | Fix |
|-------|-------|-----|
| `Undefined variable` | Missing `include` | Add `\`include "disciplines.vams"` |
| `Port not declared` | Missing `inout/input/output` | Declare port direction |
| `Contribution to non-branch` | Wrong LHS of `<+` | Use `V(p,n) <+` not `V(p) <+` for 2-terminal |
| `Convergence failure` | Discontinuous function | Use `transition()`, `limexp()`, avoid `if` on analog signals |
| `Time step too small` | Sharp discontinuity | Add `transition()` with rise/fall time |
| `Multiple contributions` | Two `<+` to same branch | Combine into single expression |

### Convergence Best Practices

```verilog
// BAD: Discontinuous
if (V(inp) > V(inn))
  V(out) <+ V(vdd);
else
  V(out) <+ V(vss);

// GOOD: Smooth transition
V(out) <+ V(vss) + (V(vdd) - V(vss)) * 
  (tanh(1000 * (V(inp) - V(inn))) + 1) / 2;

// BAD: exp() can overflow
I(d, s) <+ Is * (exp(V(d,s) / $vt) - 1);

// GOOD: limexp() prevents overflow
I(d, s) <+ Is * (limexp(V(d,s) / $vt) - 1);
```

### Debug with Virtuoso-CLI

```bash
# Compile and check syntax
virtuoso skill exec 'ahdlCompile("myLib" "myCell" "veriloga")'

# Check compilation log
virtuoso skill exec 'ahdlGetLog("myLib" "myCell" "veriloga")'

# Simulate with verbose spectre output
virtuoso sim setup --lib myLib --cell myTB
virtuoso sim run --analysis tran --stop 10u --timeout 300

# If convergence fails, add spectre options:
virtuoso skill exec 'option(quote(spectre) quote(reltol) 1e-4)'
virtuoso skill exec 'option(quote(spectre) quote(gmin) 1e-14)'
```

## Creating Verilog-A View in Virtuoso via CLI

```bash
# Method 1: Write .va file and load
cat > /tmp/my_model.va << 'EOF'
`include "disciplines.vams"
module my_model(p, n);
  inout p, n;
  electrical p, n;
  parameter real r = 1e3;
  analog V(p,n) <+ I(p,n) * r;
endmodule
EOF

# Upload to Virtuoso and compile
virtuoso skill exec 'ahdlCompile(parseString("/tmp/my_model.va"))'

# Method 2: Create veriloga cellview directly
virtuoso skill exec '
  let((cv)
    cv = dbOpenCellViewByType("myLib" "my_model" "veriloga" "text.editor" "w")
    when(cv dbSave(cv))
  )
'
```
