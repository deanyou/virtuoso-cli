# Verilog-A `bound_step()` Unsupported in SPECTRE231 / IC23.1

## Source
Session 2026-04-21: live test of veriloga skill templates against SPECTRE231.
VACOMP-2259 fatal error when compiling beh_comp.va containing `bound_step(tr/5)`.

## Summary
`bound_step(dt)` is a Verilog-A analog operator that is not implemented in
SPECTRE231 (IC23.1); using it causes a fatal VACOMP-2259 compilation error.

## Content

### Symptom

```
Error found by spectre during AHDL read-in.
    ERROR (VACOMP-2259): "bound_step(tr / 5)<<--? ;"
        "/tmp/test_beh_comp.va", line 10: Encountered an undefined function
        bound_step. Check the spelling of the function or define the function
```

Spectre terminates with `1 error` and no PSF output.

### Root Cause

`bound_step(dt)` is a Verilog-A system task that forces the simulator time-step
to stay ≤ dt around fast transitions. SPECTRE231 simply does not implement it.

### Fix

Remove or comment out all `bound_step()` calls. The simulation still runs; it
may take smaller automatic steps internally based on `transition()` slopes, but
without the explicit bound.

```verilog
// SPECTRE231: comment this out
// bound_step(tr / 5);

// transition() already limits output slew — simulation converges without bound_step
V(out, vss) <+ transition(vout_ideal, td, tr, tr);
```

### Verification

After removing the call: `spectre completes with 0 errors`.

### Version note

`bound_step` may be supported in IC25 / SPECTRE241+. Confirm before re-enabling.

## When to Use
- Writing any Verilog-A model for the IC23.1 environment
- Porting models from IC25 that include `bound_step()` calls
- Diagnosing VACOMP-2259 errors

## Context Links
- Related: [[spectre-vsource-ac-parameter]] — other SPECTRE231 language quirks
- Related: [[spectre-bandgap-dc-convergence]] — IC23.1 simulation troubleshooting
