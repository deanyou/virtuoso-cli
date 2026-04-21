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

### 事件、时步控制与调试系统函数

| 函数 | 用法 | 说明 |
|------|------|------|
| `cross(expr, dir)` | 事件检测 | `dir=+1/-1/0` 上升/下降/双向过零 |
| `timer(t0, period)` | 周期事件 | `period=0` 单次触发 |
| `initial_step` | 事件 | 仿真第一步触发，用于初始化变量 |
| `final_step` | 事件 | 仿真最后一步触发，用于打印结果 |
| `bound_step(dt)` | 时步限制 | 强制 simulator 步长 ≤ dt，避免过冲 |
| `$discontinuity(n)` | 不连续通知 | `n=0` 强制重新收敛，`n>0` 提示 |
| `$simparam("name", def)` | 读仿真参数 | 从 Ocean/spectre 读 temperature、freq 等 |
| `$strobe("fmt", args)` | 调试打印 | 每个时步末尾打印，格式同 printf |

```verilog
// cross(): 检测输出过零，记录传播延迟
@(cross(V(out) - 0.5*V(vdd), +1)) begin
  $strobe("rising edge at t=%e", $abstime);
end

// bound_step(): 强制细化步长以分辨快速沿
bound_step(tr / 5);

// $simparam(): 读取仿真温度（比 $temperature 更灵活）
real temp_c;
temp_c = $simparam("temperature", 27.0);
```

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

### 6. Behavioral Comparator

`tanh()` 替代 `if/else` — 连续可微，convergence 友好；`transition()` 控制输出边沿；`bound_step()` 确保 simulator 能分辨上升沿。

```verilog
`include "constants.vams"
`include "disciplines.vams"

module beh_comp(inp, inn, out, vdd, vss);
  inout inp, inn, out, vdd, vss;
  electrical inp, inn, out, vdd, vss;

  parameter real vos  = 0.0;      // 失调电压 (V)
  parameter real gain = 1e4;      // 开环增益 (V/V)
  parameter real td   = 1e-9;     // 传播延迟 (s)
  parameter real tr   = 200e-12;  // 输出上升/下降时间 (s)

  real vd, vout_ideal;

  analog begin
    vd = V(inp, inn) - vos;
    vout_ideal = (tanh(gain * vd / V(vdd, vss)) + 1) / 2 * V(vdd, vss) + V(vss);
    V(out, vss) <+ transition(vout_ideal, td, tr, tr);
    bound_step(tr / 5);
  end
endmodule
```

### 7. RLC Passives（带温度系数）

```verilog
// 电阻（TC1/TC2）
module r_tc(p, n);
  inout p, n;  electrical p, n;
  parameter real r0   = 1e3;   // 标称值 (Ω)
  parameter real tc1  = 0.0;   // 一阶温度系数 (1/°C)
  parameter real tc2  = 0.0;   // 二阶温度系数 (1/°C²)
  parameter real tnom = 27.0;
  real dt;
  analog begin
    dt = $temperature - (tnom + 273.15);
    V(p, n) <+ I(p, n) * r0 * (1 + tc1*dt + tc2*dt*dt);
  end
endmodule

// 电容（带初始条件）— 用状态变量形式
module cap_ic(p, n);
  inout p, n;  electrical p, n;
  parameter real c  = 1e-12;
  parameter real ic = 0.0;    // 初始电压 (V)
  real vc;
  analog begin
    vc = idt(I(p, n) / c, ic);  // 从 ic 开始积分
    V(p, n) <+ vc;
  end
endmodule
```

> 普通电容用 `I(p,n) <+ c * ddt(V(p,n))`；IC 版本用 `idt(I/C, ic)` 状态变量形式。
> Spectre 外部 IC 语法：`.ic V(node_name)=1.2`（.scs 格式：`ic V(node)=1.2`）

### 8. 理想受控源（VCCS / VCVS）

```verilog
// VCCS: I_out = gm * V_in（建模跨导放大器增益级）
module vccs(vp, vn, ip, in_);
  inout vp, vn, ip, in_;
  electrical vp, vn, ip, in_;
  parameter real gm = 1e-3;
  analog I(ip, in_) <+ gm * V(vp, vn);
endmodule

// VCVS: V_out = A * V_in（建模误差放大器）
module vcvs(vp, vn, op, on);
  inout vp, vn, op, on;
  electrical vp, vn, op, on;
  parameter real gain = 1.0;
  analog V(op, on) <+ gain * V(vp, vn);
endmodule
```

### 9. LDO Behavioral Model（PSRR + Dropout + Load Regulation）

```verilog
`include "constants.vams"
`include "disciplines.vams"

module beh_ldo(vin, vout, en, gnd);
  inout vin, vout, en, gnd;
  electrical vin, vout, en, gnd;

  parameter real vout_nom = 1.2    from (0:inf);   // 额定输出 (V)
  parameter real vdo      = 0.2    from (0:inf);   // 最小压差 dropout (V)
  parameter real psrr_db  = 60.0   from (0:120);   // DC PSRR (dB，典型 40–80)
  parameter real f_psrr   = 1e3    from (0:inf);   // PSRR 劣化频率 (Hz)
  parameter real rout     = 50e-3  from (0:inf);   // 输出阻抗 → load regulation (Ω)
  parameter real ilim     = 500e-3 from (0:inf);   // 输出电流限制 (A)
  parameter real tr_en    = 1e-6   from (0:inf);   // Enable 上升时间 (s)
  parameter real vin_nom  = 1.8    from (0:inf);   // VIN 标称值（PSRR 基准）(V)

  real psrr_lin, tau_p, ven, vripple, vout_i, vdo_lim, iex;

  analog begin
    // Enable: smooth 0→1
    ven = transition((V(en, gnd) > 0.5) ? 1.0 : 0.0, 0, tr_en, tr_en);

    // PSRR: 高通特性（DC 好，高频劣化）
    // H(s) = (psrr_lin + s·τ)/(1 + s·τ)  DC:psrr_lin; f>>f_psrr: H→1
    psrr_lin = pow(10.0, -psrr_db / 20.0);
    tau_p    = 1.0 / (2.0 * `M_PI * f_psrr);
    vripple  = laplace_nd(V(vin, gnd) - vin_nom,
                          {psrr_lin, tau_p},
                          {1.0,      tau_p});

    // Dropout: soft-min(vout_nom, vin-vdo) — 连续可微
    vout_i  = vout_nom + vripple;
    vdo_lim = V(vin, gnd) - vdo;
    vout_i  = (vout_i + vdo_lim -
               sqrt((vout_i-vdo_lim)*(vout_i-vdo_lim) + 1e-6)) / 2.0;

    // 输出：Thevenin + 软电流限制（fold-back）
    iex = (I(vout,gnd) - ilim +
           sqrt((I(vout,gnd)-ilim)*(I(vout,gnd)-ilim) + 1e-6)) / 2.0;
    V(vout, gnd) <+ ven * (vout_i - I(vout,gnd)*rout - iex*10.0);

    // Disabled: 1Ω 下拉到地
    I(vout, gnd) <+ (1.0 - ven) * V(vout, gnd) / 1.0;
  end
endmodule
```

**测量要点**：
- Load regulation：`vcli maestro get-output-value Vout_reg DC_corner`
- Line regulation：扫 `vin` from `vout_nom+vdo` to `3.3V`，测 `vout`
- PSRR：AC 仿真，激励加在 `vin` 节点，测 `vout/vin` 的 dB

### 10. 三态 PFD（PLL Phase-Frequency Detector）

演示 `cross()` 边沿检测 + `timer()` 延时复位的**事件驱动状态机**模式。  
`t_rst` = 复位延迟 = 最小脉宽 — 消除 dead zone；`timer(t_reset, 0)` 在变量更新后自动重调度。

```verilog
`include "constants.vams"
`include "disciplines.vams"

module beh_pfd(ref_clk, fb_clk, up, dn, gnd);
  inout ref_clk, fb_clk, up, dn, gnd;
  electrical ref_clk, fb_clk, up, dn, gnd;

  parameter real icp   = 100e-6  from (0:inf);   // 电荷泵电流 (A)
  parameter real td    = 200e-12 from (0:inf);   // 传播延迟 (s)
  parameter real tr    = 100e-12 from (0:inf);   // 输出沿时间 (s)
  parameter real t_rst = 500e-12 from (0:inf);   // 复位延时 = 最小脉宽，消除 dead zone (s)
  parameter real vth   = 0.6;                     // 时钟阈值 (V)

  real up_q, dn_q;    // 锁存状态 (0.0 / 1.0)
  real t_reset;       // 计划复位时刻（1e30 = 未调度）

  analog begin
    @(initial_step) begin
      up_q    = 0.0;
      dn_q    = 0.0;
      t_reset = 1e30;
    end

    // REF 上升沿 → 置 UP
    @(cross(V(ref_clk, gnd) - vth, +1)) begin
      up_q = 1.0;
      if (dn_q > 0.5)
        t_reset = $abstime + t_rst;
    end

    // FB 上升沿 → 置 DN
    @(cross(V(fb_clk, gnd) - vth, +1)) begin
      dn_q = 1.0;
      if (up_q > 0.5)
        t_reset = $abstime + t_rst;
    end

    // 复位：UP/DN 同时清零（timer 变量变化时自动重调度）
    @(timer(t_reset, 0)) begin
      up_q    = 0.0;
      dn_q    = 0.0;
      t_reset = 1e30;   // 解除调度
    end

    // 电流输出 → 驱动外部电荷泵 / 滤波器
    I(up, gnd) <+ -icp * transition(up_q, td, tr, tr);
    I(dn, gnd) <+ -icp * transition(dn_q, td, tr, tr);
    I(up, gnd) <+ V(up, gnd) * 1e-9;   // 弱下拉，防悬空
    I(dn, gnd) <+ V(dn, gnd) * 1e-9;

    bound_step(tr / 5);
  end
endmodule
```

**仿真 testbench（standalone .scs）**：

```
simulator lang=spectre
ahdl_include "/path/to/beh_pfd.va"
parameters fref=100e6 dt=1/fref phase_err=500p

Vref  (ref_clk 0) vsource type=pulse val0=0 val1=1.2 \
      period=dt rise=100p fall=100p width='dt/2'
Vfb   (fb_clk  0) vsource type=pulse val0=0 val1=1.2 \
      period=dt delay=phase_err rise=100p fall=100p width='dt/2'

xpfd (ref_clk fb_clk up dn 0) beh_pfd icp=100u t_rst=500p

// 电荷泵 + 简单 RC 滤波（验证 UP/DN 电流输出）
Iup (vcp up)  isource dc=0        // 外部 CP 电流源占位
Ccp (vcp 0)   capacitor c=10p
Rcp (vcp 0)   resistor  r=10k

tran1 tran stop=200n
```

**设计要点**：
- Dead zone 根因：相位误差极小时，复位脉冲在 UP/DN 充分泵入电荷前就到来 → `t_rst` 保证最小重叠时间
- `timer(t_reset, 0)` 中 period=0 表示单次触发；`t_reset` 变量更新后 Spectre 会在新时刻重新调度
- `t_reset = 1e30` 用于"未调度"状态 — 仿真在此之前结束，不会意外触发

### 11. LC VCO 行为模型（相位噪声 + Kvco）

演示**相位噪声积分节点**模式：将白色/1/f FM 噪声电流注入内部 C=1F 节点，积分后得到 φ_noise(t)，叠加到相位上。

**推导**：白色 FM 噪声 S_f [Hz²/Hz] → 积分 → S_φ(fm) = S_f/fm² [rad²/Hz]  
在 fm=pn_off 处 L = S_φ/2 = pn_lin → `pn_si = 8π²·pn_lin·pn_off²`（注入电流 PSD）

```verilog
`include "constants.vams"
`include "disciplines.vams"

module beh_vco(vtune, out, gnd);
  inout vtune, out, gnd;
  electrical vtune, out, gnd;
  electrical phi_n;              // 内部相位噪声积分节点

  parameter real f0     = 2.4e9   from (0:inf);  // 中心频率 (Hz)
  parameter real kvco   = 100e6   from (0:inf);  // 调谐增益 (Hz/V)
  parameter real vamp   = 0.6;                    // 输出半摆幅 (V)
  parameter real pn_dbc = -120.0;                 // 相位噪声 (dBc/Hz) @ pn_off
  parameter real pn_off = 1e6    from (0:inf);   // 参考偏移 (Hz，典型 1M)
  parameter real pn_fc  = 0.0;                    // 1/f 拐角频率 (Hz)，0=不建模 1/f³

  real phi, f_inst, pn_si;

  analog begin
    // 瞬时频率 + 相位积分
    f_inst = f0 + kvco * V(vtune, gnd);
    phi    = 2.0 * `M_PI * idt(f_inst, 0);

    // 相位噪声注入：I_noise → C=1F → V(phi_n) = ∫I dt = φ_noise(t)
    pn_si = 8.0 * `M_PI * `M_PI *
            pow(10.0, pn_dbc / 10.0) * pn_off * pn_off;

    I(phi_n, gnd) <+ ddt(V(phi_n, gnd));               // 1F 归一化电容
    I(phi_n, gnd) <+ V(phi_n, gnd) * 1e-12;            // 极大电阻防 DC 悬空
    I(phi_n, gnd) <+ -white_noise(pn_si, "pn_white");  // 热噪声 → -20dB/dec PN

    // 1/f³ 区域（可选）：flicker FM 电流 → 积分后 -30dB/dec PN
    if (pn_fc > 0.0)
      I(phi_n, gnd) <+ -flicker_noise(pn_si * pn_fc / pn_off, 1, "pn_flicker");

    // 输出：平滑方波（tanh gain=50 → 过零沿约 2% 周期）
    V(out, gnd) <+ vamp * tanh(50.0 * sin(phi + V(phi_n, gnd)));

    bound_step(0.05 / f_inst);   // 每周期 ≥ 20 步
  end
endmodule
```

**验证 testbench**：

```
simulator lang=spectre
ahdl_include "/path/to/beh_vco.va"

Vtune (vtune 0) vsource dc=0.5          // 静态调谐电压
xvco  (vtune out 0) beh_vco f0=2.4e9 kvco=100e6 pn_dbc=-120 pn_off=1e6

tran1 tran stop=100n

// 验证频率：awk 统计过零次数除以仿真时间
// 验证 Kvco：改 Vtune dc 值，测输出频率差
// 验证 PN：noise analysis 或 jitter 测量（Spectre jitter 分析）
```

**设计要点**：
- `phi_n` 内部节点电压 = φ_noise(t)（Brownian motion，均值 0，方差随时间增长）
- DC 偏置 `1e-12` Ω⁻¹（约 1TΩ 等效电阻）只是为了 DC 求解不悬空，不影响 AC/tran
- 1/f 拐角 `pn_fc`：典型 LC VCO 100kHz–1MHz，Ring VCO 10MHz+
- 相位噪声核验：Spectre `pnoise` 分析直接给出 dBc/Hz，或用 tran + FFT

### 12. Sample-and-Hold（孔径延迟 + Droop + kT/C 噪声）

演示三个模式：`absdelay()` 实现孔径延迟；**tanh 开关 + 内部电容节点**实现模式切换；**开关热噪声**自动给出 kT/C 总噪声功率。

```verilog
`include "constants.vams"
`include "disciplines.vams"

module beh_sh(inp, out, clk, gnd);
  inout inp, out, clk, gnd;
  electrical inp, out, clk, gnd;
  electrical vc;                          // 内部采样电容节点

  parameter real c      = 1e-12  from (0:inf);  // 采样电容 (F)
  parameter real ron    = 200.0  from (0:inf);  // 开关导通电阻 (Ω)
  parameter real r_leak = 1e12   from (0:inf);  // 保持漏电阻 (Ω)，droop ≈ V/(r_leak·c)
  parameter real td_ap  = 100e-12 from (0:inf); // 孔径延迟 (s)：CLK 下降沿→实际采样时刻
  parameter real vth    = 0.6;                   // CLK 阈值 (V)
  parameter real sw_gn  = 1e4;                   // tanh 开关锐度（大→接近硬开关）
  parameter real rout   = 10.0   from (0:inf);  // 输出缓冲电阻 (Ω)

  real sw, gsw, clk_d;

  analog begin
    // 孔径延迟：采样动作在 CLK 下降后 td_ap 发生
    clk_d = absdelay(V(clk, gnd), td_ap);

    // 平滑开关：CLK 高→采样(sw≈1)，CLK 低→保持(sw≈0)
    sw  = (1.0 + tanh(sw_gn * (clk_d - vth))) / 2.0;
    gsw = sw / ron + (1.0 - sw) / r_leak;        // 采样:1/Ron, 保持:1/R_leak

    // 采样电容动态方程
    I(vc, gnd) <+ c * ddt(V(vc, gnd));
    I(vc, inp) <+ (V(vc, gnd) - V(inp, gnd)) * gsw;

    // kT/C 热噪声：开关电阻 4kT/Ron 经 Ron·C 带宽限制，总功率 = kT/C [V²]
    I(vc, gnd) <+ sw * white_noise(4.0 * `P_K * $temperature / ron, "ktc");

    // 输出：Thevenin 缓冲（高阻抗时 rout→0）
    V(out, gnd) <+ V(vc, gnd) - I(out, gnd) * rout;

    bound_step(ron * c / 5);
  end
endmodule
```

**关键模式解析**：

| 功能 | 实现方式 |
|------|---------|
| 孔径延迟 | `absdelay(V(clk), td_ap)` — 延迟后的时钟驱动开关 |
| 模式切换 | `tanh(sw_gn*(clk_d-vth))` — 连续开关，不需要 `cross()` 事件 |
| Droop | `(1-sw)/r_leak` 漏电导 — 保持相 vc 通过 R_leak 缓慢放电 |
| kT/C 噪声 | `sw * white_noise(4kT/Ron)` + C → 带宽 = 1/(2π·Ron·C)，总功率 ∫ = kT/C |
| DC 工作点 | CLK=0 → sw≈0 → Vc→Vin（通过 R_leak），无悬空 |

> `absdelay(x, td)` 要求 td > 0 且在仿真期间保持常数；对零延迟直接用 `clk_d = V(clk, gnd)`。

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
// BAD: Discontinuous if/else
if (V(inp) > V(inn))
  V(out) <+ V(vdd);
else
  V(out) <+ V(vss);

// GOOD: tanh smooth transition
V(out) <+ V(vss) + (V(vdd) - V(vss)) *
  (tanh(1000 * (V(inp) - V(inn))) + 1) / 2;

// BAD: exp() can overflow
I(d, s) <+ Is * (exp(V(d,s) / $vt) - 1);

// GOOD: limexp() prevents overflow
I(d, s) <+ Is * (limexp(V(d,s) / $vt) - 1);
```

#### Soft Clamp（连续可微的 min/max）

硬 `if` 造成不连续 → 用 soft 版本替代：

```verilog
// soft-min(a, b): 取两者较小值，在转折处连续
// ε = 1e-6 决定平滑程度（越小越接近硬限）
real a, b, soft_min_val;
soft_min_val = (a + b - sqrt((a-b)*(a-b) + 1e-6)) / 2.0;

// soft-max(0, x): ReLU 的连续可微近似
real x, soft_relu;
soft_relu = (x + sqrt(x*x + 1e-6)) / 2.0;

// 典型应用：Dropout 限制（vout ≤ vin - vdo）
real vout_ideal, vdo_limit;
vout_ideal = (vout_ideal + vdo_limit -
              sqrt((vout_ideal-vdo_limit)*(vout_ideal-vdo_limit) + 1e-6)) / 2.0;

// 典型应用：电流限制（折回式）
real iex;  // iex = soft-max(0, I - ilim)
iex = (I(out,gnd) - ilim +
       sqrt((I(out,gnd)-ilim)*(I(out,gnd)-ilim) + 1e-6)) / 2.0;
V(out, gnd) <+ vout_ideal - iex * rfold;  // 超限后折回
```

#### Thevenin 模式（输出阻抗建模）

`V(p,n) <+` 右侧可以包含 `I(p,n)`，simulator 隐式求解线性方程：

```verilog
// 等效：V_out = V_oc - I_load * Rout
// 不要用 if 判断 Iload 方向，直接写隐式方程
V(out, gnd) <+ vout_oc - I(out, gnd) * rout;

// 注：I(out,gnd) 读取的是流出 out 节点的电流（负载电流）
// Spectre 将此视为线性隐式方程，收敛稳定
```

#### laplace_nd 系数说明

```verilog
// H(s) = (n0 + n1*s + n2*s²) / (d0 + d1*s + d2*s²)
// 数组按 s 的幂次升序：{s^0项, s^1项, ...}
laplace_nd(x, {n0, n1}, {d0, d1})

// 单极点低通（-3dB at fp）:  H(s) = gain / (1 + s/ω_p)
laplace_nd(x, {gain}, {1, 1.0/(2*`M_PI*fp)})

// PSRR 高通（DC 好，高频劣化）:  H(s) = (k + s·τ) / (1 + s·τ)
// DC: H=k=psrr_lin; f>>fp: H→1
real tau_p, psrr_lin;
psrr_lin = pow(10.0, -psrr_db / 20.0);
tau_p    = 1.0 / (2.0 * `M_PI * f_psrr);
vripple  = laplace_nd(vin_ripple, {psrr_lin, tau_p}, {1.0, tau_p});
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

## 端到端仿真流程（standalone spectre）

无需 ADE，直接验证 Verilog-A 模型的完整流程：

```bash
# 1. 写 .va 文件
cat > /tmp/beh_comp.va << 'EOF'
`include "constants.vams"
`include "disciplines.vams"
module beh_comp(inp, inn, out, vdd, vss);
  ... (参见模板 6)
endmodule
EOF

# 2. 上传到 Virtuoso 并编译
vcli upload-text /tmp/beh_comp.va ~/cadence/myLib/beh_comp/veriloga/veriloga.scs
vcli skill exec 'ahdlCompile("myLib" "beh_comp" "veriloga")'
vcli skill exec 'ahdlGetLog("myLib" "beh_comp" "veriloga")'  # 确认无 Error

# 3. 写 standalone testbench .scs（不依赖 ADE/Maestro）
cat > /tmp/tb_comp.scs << 'EOF'
simulator lang=spectre
ahdl_include "/path/to/beh_comp.va"   // Spectre 直接读 .va

parameters vdd=1.2 vcm=0.6 vdiff=10m

Vvdd (vdd 0) vsource dc=vdd
Vcm  (inp 0) vsource dc=vcm
Vdiff (inp inn) vsource dc=vdiff ac=1

xcomp (inp inn out vdd 0) beh_comp gain=1e4 td=1n tr=200p

dcop dc
ac1  ac start=1 stop=1G dec=20
tran1 tran stop=100n
EOF

spectre /tmp/tb_comp.scs -format psfascii -raw /tmp/tb_psf +mt

# 4. 快速检查结果
tail -2 /tmp/tb_psf/spectre.out          # 确认 "0 errors"
awk '/^V.out/{print}' /tmp/tb_psf/dcop.dc  # DC op
```

**`ahdl_include` vs `include`**：standalone 模式用 `ahdl_include` 让 Spectre 直接编译 .va 文件；Virtuoso cellview 模式用 `ahdlCompile()`。

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
