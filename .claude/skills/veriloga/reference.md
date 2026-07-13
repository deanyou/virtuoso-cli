# Verilog-A Module Templates

## 1. Ideal Voltage Source (DC + AC + Pulse)

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

## 2. Ideal Current Mirror (behavioral)

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

## 3. Ideal Opamp (finite gain, GBW, slew)

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
    I(inp, inn) <+ V(inp, inn) / rin;
    vin_diff = V(inp, inn) - vos;

    fp = gbw / gain;
    vout_ideal = gain * laplace_nd(vin_diff, {1}, {1, 1.0/(2*`M_PI*fp)});

    vout_ideal = slew(vout_ideal, sr, sr);

    if (vout_ideal > V(vdd) - 0.05)
      vout_ideal = V(vdd) - 0.05;
    else if (vout_ideal < V(vss) + 0.05)
      vout_ideal = V(vss) + 0.05;

    V(out) <+ vout_ideal;
    I(out) <+ V(out) / rout;
  end
endmodule
```

## 4. Bandgap Reference (behavioral)

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
    vref_t = vref_t + psrr_dc * (V(vdd, gnd) - 1.2);
    V(vref, gnd) <+ vref_t;
    I(vref, gnd) <+ V(vref, gnd) / rout;
  end
endmodule
```

## 5. Testbench Stimulus (PWL + Noise)

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
    V(out, gnd) <+ white_noise(noise_density * noise_density, "thermal");
  end
endmodule
```

## 6. Behavioral Comparator

`tanh()` 替代 `if/else` — 连续可微，convergence 友好；`transition()` 控制输出边沿。`bound_step()` IC23.1/SPECTRE231 不支持，勿使用。

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
    // bound_step(tr / 5);  // IC25+ only; not supported in SPECTRE231
  end
endmodule
```

## 7. RLC Passives（带温度系数）

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
    vc = idt(I(p, n) / c, ic);
    V(p, n) <+ vc;
  end
endmodule
```

> 普通电容用 `I(p,n) <+ c * ddt(V(p,n))`；IC 版本用 `idt(I/C, ic)` 状态变量形式。

## 8. 理想受控源（VCCS / VCVS）

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

## 9. LDO Behavioral Model（PSRR + Dropout + Load Regulation）

```verilog
`include "constants.vams"
`include "disciplines.vams"

module beh_ldo(vin, vout, en, gnd);
  inout vin, vout, en, gnd;
  electrical vin, vout, en, gnd;

  parameter real vout_nom = 1.2    from (0:inf);   // 额定输出 (V)
  parameter real vdo      = 0.2    from (0:inf);   // 最小压差 dropout (V)
  parameter real psrr_db  = 60.0   from (0:120);   // DC PSRR (dB)
  parameter real f_psrr   = 1e3    from (0:inf);   // PSRR 劣化频率 (Hz)
  parameter real rout     = 50e-3  from (0:inf);   // 输出阻抗 (Ω)
  parameter real ilim     = 500e-3 from (0:inf);   // 输出电流限制 (A)
  parameter real tr_en    = 1e-6   from (0:inf);   // Enable 上升时间 (s)
  parameter real vin_nom  = 1.8    from (0:inf);   // VIN 标称值（PSRR 基准）(V)

  real psrr_lin, tau_p, ven, vripple, vout_i, vdo_lim, iex;

  analog begin
    ven = transition((V(en, gnd) > 0.5) ? 1.0 : 0.0, 0, tr_en, tr_en);

    psrr_lin = pow(10.0, -psrr_db / 20.0);
    tau_p    = 1.0 / (2.0 * `M_PI * f_psrr);
    vripple  = laplace_nd(V(vin, gnd) - vin_nom,
                          {psrr_lin, tau_p},
                          {1.0,      tau_p});

    vout_i  = vout_nom + vripple;
    vdo_lim = V(vin, gnd) - vdo;
    vout_i  = (vout_i + vdo_lim -
               sqrt((vout_i-vdo_lim)*(vout_i-vdo_lim) + 1e-6)) / 2.0;

    iex = (I(vout,gnd) - ilim +
           sqrt((I(vout,gnd)-ilim)*(I(vout,gnd)-ilim) + 1e-6)) / 2.0;
    V(vout, gnd) <+ ven * (vout_i - I(vout,gnd)*rout - iex*10.0);

    I(vout, gnd) <+ (1.0 - ven) * V(vout, gnd) / 1.0;
  end
endmodule
```

**测量要点**：
- Load regulation：`vcli maestro get-output-value Vout_reg DC_corner`
- PSRR：AC 仿真，激励加在 `vin` 节点，测 `vout/vin` 的 dB

## 10. 三态 PFD（PLL Phase-Frequency Detector）

事件驱动状态机：`cross()` 边沿检测 + `timer(var, 0)` 延时复位。  
`t_rst` = 最小脉宽，消除 dead zone；`t_reset = 1e30` 表示未调度。

```verilog
`include "constants.vams"
`include "disciplines.vams"

module beh_pfd(ref_clk, fb_clk, up, dn, gnd);
  inout ref_clk, fb_clk, up, dn, gnd;
  electrical ref_clk, fb_clk, up, dn, gnd;

  parameter real icp   = 100e-6  from (0:inf);   // 电荷泵电流 (A)
  parameter real td    = 200e-12 from (0:inf);   // 传播延迟 (s)
  parameter real tr    = 100e-12 from (0:inf);   // 输出沿时间 (s)
  parameter real t_rst = 500e-12 from (0:inf);   // 复位延时，消除 dead zone (s)
  parameter real vth   = 0.6;                     // 时钟阈值 (V)

  real up_q, dn_q;
  real t_reset;

  analog begin
    @(initial_step) begin
      up_q    = 0.0;
      dn_q    = 0.0;
      t_reset = 1e30;
    end

    @(cross(V(ref_clk, gnd) - vth, +1)) begin
      up_q = 1.0;
      if (dn_q > 0.5)
        t_reset = $abstime + t_rst;
    end

    @(cross(V(fb_clk, gnd) - vth, +1)) begin
      dn_q = 1.0;
      if (up_q > 0.5)
        t_reset = $abstime + t_rst;
    end

    @(timer(t_reset, 0)) begin
      up_q    = 0.0;
      dn_q    = 0.0;
      t_reset = 1e30;
    end

    I(up, gnd) <+ -icp * transition(up_q, td, tr, tr);
    I(dn, gnd) <+ -icp * transition(dn_q, td, tr, tr);
    I(up, gnd) <+ V(up, gnd) * 1e-9;
    I(dn, gnd) <+ V(dn, gnd) * 1e-9;

    // bound_step(tr / 5);  // IC25+ only; not supported in SPECTRE231
  end
endmodule
```

**Standalone testbench**：

```
simulator lang=spectre
ahdl_include "/path/to/beh_pfd.va"
parameters fref=100e6 dt=1/fref phase_err=500p

Vref  (ref_clk 0) vsource type=pulse val0=0 val1=1.2 \
      period=dt rise=100p fall=100p width='dt/2'
Vfb   (fb_clk  0) vsource type=pulse val0=0 val1=1.2 \
      period=dt delay=phase_err rise=100p fall=100p width='dt/2'

xpfd (ref_clk fb_clk up dn 0) beh_pfd icp=100u t_rst=500p
Ccp (vcp 0) capacitor c=10p
Rcp (vcp 0) resistor  r=10k

tran1 tran stop=200n
```

## 11. LC VCO 行为模型（相位噪声 + Kvco）

相位噪声积分节点模式：白色 FM 噪声电流注入内部 C=1F 节点，积分后得到 φ_noise(t)。  
**推导**：S_φ(fm) = S_f/fm²；在 fm=pn_off 处 L=pn_lin → `pn_si = 8π²·pn_lin·pn_off²`

```verilog
`include "constants.vams"
`include "disciplines.vams"

module beh_vco(vtune, out, gnd);
  inout vtune, out, gnd;
  electrical vtune, out, gnd;
  electrical phi_n;

  parameter real f0     = 2.4e9   from (0:inf);  // 中心频率 (Hz)
  parameter real kvco   = 100e6   from (0:inf);  // 调谐增益 (Hz/V)
  parameter real vamp   = 0.6;                    // 输出半摆幅 (V)
  parameter real pn_dbc = -120.0;                 // 相位噪声 (dBc/Hz) @ pn_off
  parameter real pn_off = 1e6    from (0:inf);   // 参考偏移 (Hz)
  parameter real pn_fc  = 0.0;                    // 1/f 拐角频率 (Hz)，0=不建模

  real phi, f_inst, pn_si;

  analog begin
    f_inst = f0 + kvco * V(vtune, gnd);
    phi    = 2.0 * `M_PI * idt(f_inst, 0);

    pn_si = 8.0 * `M_PI * `M_PI *
            pow(10.0, pn_dbc / 10.0) * pn_off * pn_off;

    I(phi_n, gnd) <+ ddt(V(phi_n, gnd));
    I(phi_n, gnd) <+ V(phi_n, gnd) * 1e-12;
    I(phi_n, gnd) <+ -white_noise(pn_si, "pn_white");

    if (pn_fc > 0.0)
      I(phi_n, gnd) <+ -flicker_noise(pn_si * pn_fc / pn_off, 1, "pn_flicker");

    V(out, gnd) <+ vamp * tanh(50.0 * sin(phi + V(phi_n, gnd)));

    bound_step(0.05 / f_inst);
  end
endmodule
```

**设计要点**：
- `phi_n` 节点电压 = φ_noise(t)（Brownian motion）
- `1e-12` Ω⁻¹（≈1TΩ）防 DC 悬空，不影响 AC/tran
- 验证：Spectre `pnoise` 分析或 tran + FFT

## 12. Sample-and-Hold（孔径延迟 + Droop + kT/C 噪声）

`absdelay()` 孔径延迟；tanh 开关 + 内部电容节点；开关热噪声自动给出 kT/C 总功率。

```verilog
`include "constants.vams"
`include "disciplines.vams"

module beh_sh(inp, out, clk, gnd);
  inout inp, out, clk, gnd;
  electrical inp, out, clk, gnd;
  electrical vc;

  parameter real c      = 1e-12   from (0:inf);
  parameter real ron    = 200.0   from (0:inf);
  parameter real r_leak = 1e12    from (0:inf);
  parameter real td_ap  = 100e-12 from (0:inf);
  parameter real vth    = 0.6;
  parameter real sw_gn  = 1e4;
  parameter real rout   = 10.0    from (0:inf);

  real sw, gsw, clk_d;

  analog begin
    clk_d = absdelay(V(clk, gnd), td_ap);
    sw    = (1.0 + tanh(sw_gn * (clk_d - vth))) / 2.0;
    gsw   = sw / ron + (1.0 - sw) / r_leak;

    I(vc, gnd) <+ c * ddt(V(vc, gnd));
    I(vc, inp) <+ (V(vc, gnd) - V(inp, gnd)) * gsw;

    I(vc, gnd) <+ sw * white_noise(4.0 * `P_K * $temperature / ron, "ktc");

    V(out, gnd) <+ V(vc, gnd) - I(out, gnd) * rout;

    bound_step(ron * c / 5);
  end
endmodule
```

| 功能 | 实现方式 |
|------|---------|
| 孔径延迟 | `absdelay(V(clk), td_ap)` |
| 模式切换 | `tanh(sw_gn*(clk_d-vth))` — 连续，不需要 `cross()` |
| Droop | `(1-sw)/r_leak` 漏电导 |
| kT/C 噪声 | `sw * white_noise(4kT/Ron)` + C → 总功率 ∫ = kT/C |

> `absdelay(x, td)` 要求 td > 0 且在仿真期间保持常数。
