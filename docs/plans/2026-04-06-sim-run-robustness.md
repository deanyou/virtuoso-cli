# sim run Robustness Fixes

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `sim run` and `sim measure` detect and report simulation failures instead of returning false success.

**Architecture:** Add post-run validation to `sim run` (check run() return value + verify spectre.out exists), add diagnostic hints to `sim measure` when all values are nil, and add a `sim netlist` subcommand to attempt programmatic netlist regeneration.

**Tech Stack:** Rust, clap CLI, SKILL expressions via bridge

---

### Task 1: `sim run` — Detect run() returning nil

**Files:**
- Modify: `src/commands/sim.rs:27-69` (the `run` function)

**Step 1: Add nil-return detection after run()**

In `src/commands/sim.rs`, after the `run()` call succeeds at the SKILL level but returns `"nil"` as output, verify that spectre actually ran by checking for `spectre.out`.

```rust
// Replace lines 52-57 with:

    // Execute run
    let result = client.execute_skill("run()", Some(timeout))?;
    if !result.ok() {
        return Err(VirtuosoError::Execution(
            result.errors.join("; "),
        ));
    }

    // Get actual results dir
    let rdir = client.execute_skill("resultsDir()", None)?;
    let results_dir = rdir.output.trim().trim_matches('"').to_string();

    // Validate: run() returning nil usually means simulation didn't execute
    let run_output = result.output.trim().trim_matches('"');
    if run_output == "nil" {
        // Check if spectre.out exists — definitive proof spectre ran
        let check = client.execute_skill(
            &format!(r#"isFile("{results_dir}/psf/spectre.out")"#),
            None,
        )?;
        let has_spectre_out = check.output.trim().trim_matches('"');
        if has_spectre_out == "nil" || has_spectre_out == "0" {
            return Err(VirtuosoError::Execution(
                "Simulation failed: run() returned nil and no spectre.out found. \
                 The netlist may be missing or stale — regenerate via ADE \
                 (Simulation → Netlist and Run) or `virtuoso sim netlist`."
                    .into(),
            ));
        }
    }
```

The final `Ok(json!(...))` block stays as-is but now uses the already-computed `results_dir`.

**Step 2: Build and verify it compiles**

Run: `cargo build 2>&1 | tail -5`
Expected: successful build

**Step 3: Commit**

```bash
git add src/commands/sim.rs
git commit -m "fix(sim): detect run() returning nil and report netlist errors"
```

---

### Task 2: `sim measure` — Diagnose all-nil results

**Files:**
- Modify: `src/commands/sim.rs:72-104` (the `measure` function)

**Step 1: Add all-nil detection with diagnostic hints**

After collecting all measurements, check if every value is `"nil"` and provide hints.

```rust
// After the measures loop (after line 98), before the final Ok(), add:

    // Detect all-nil results and provide diagnostics
    let all_nil = measures.iter().all(|m| {
        m.get("value")
            .and_then(|v| v.as_str())
            .map(|s| s == "nil")
            .unwrap_or(false)
    });

    let mut warnings = Vec::new();
    if all_nil && !measures.is_empty() {
        // Check if spectre.out exists
        let rdir_for_check = rdir_val.to_string();
        let check = client.execute_skill(
            &format!(r#"isFile("{rdir_for_check}/psf/spectre.out")"#),
            None,
        );
        let spectre_exists = check
            .map(|r| r.output.trim().trim_matches('"') != "nil")
            .unwrap_or(false);

        if !spectre_exists {
            warnings.push("All measurements returned nil. No spectre.out found — simulation may not have run. Check netlist with `virtuoso sim netlist`.".to_string());
        } else {
            warnings.push("All measurements returned nil. Spectre ran but produced no matching data — verify signal names match your schematic (e.g., \"/net1\" vs \"/OUT\") and that the correct analysis type is selected.".to_string());
        }
    }

    Ok(json!({
        "status": "success",
        "measures": measures,
        "warnings": warnings,
    }))
```

Note: this replaces the existing `Ok(json!({...}))` at lines 100-103. The `rdir_val` variable is already in scope from line 77.

**Step 2: Build and verify**

Run: `cargo build 2>&1 | tail -5`
Expected: successful build

**Step 3: Commit**

```bash
git add src/commands/sim.rs
git commit -m "fix(sim): add diagnostics when all measure values are nil"
```

---

### Task 3: Add `sim netlist` subcommand

**Files:**
- Modify: `src/commands/sim.rs` (add `netlist` function)
- Modify: `src/main.rs` (add `Netlist` variant to `SimCmd` enum + dispatch)

**Step 1: Add the netlist function to sim.rs**

Append to the end of `src/commands/sim.rs`:

```rust
pub fn netlist(recreate: bool) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    // Method 1: Ocean createNetlist
    let r1 = client.execute_skill(
        if recreate {
            "createNetlist(?recreateAll t ?display nil)"
        } else {
            "createNetlist(?display nil)"
        },
        Some(60),
    )?;
    let r1_out = r1.output.trim().trim_matches('"');
    if r1.ok() && r1_out != "nil" {
        return Ok(json!({
            "status": "success",
            "method": "createNetlist",
            "output": r1_out,
        }));
    }

    // Method 2: ASI session-based netlisting
    let r2 = client.execute_skill(
        "asiCreateNetlist(asiGetSession(hiGetCurrentWindow()))",
        Some(60),
    )?;
    let r2_out = r2.output.trim().trim_matches('"');
    if r2.ok() && r2_out != "nil" {
        return Ok(json!({
            "status": "success",
            "method": "asiCreateNetlist",
            "output": r2_out,
        }));
    }

    Err(VirtuosoError::Execution(
        "Cannot create netlist programmatically. \
         Open ADE L for this cell and run Simulation → Netlist and Run."
            .into(),
    ))
}
```

**Step 2: Add CLI variant in main.rs**

In `src/main.rs`, add `Netlist` to the `SimCmd` enum (after `Results`):

```rust
    /// Force netlist regeneration
    #[command(
        long_about = "Attempt to regenerate the simulation netlist programmatically.\n\n\
            Examples:\n  \
            virtuoso sim netlist\n  \
            virtuoso sim netlist --recreate"
    )]
    Netlist {
        /// Force full netlist recreation
        #[arg(long)]
        recreate: bool,
    },
```

**Step 3: Add dispatch in main.rs**

In the `SimCmd` match block (around line 584), add:

```rust
            SimCmd::Netlist { recreate } => commands::sim::netlist(recreate),
```

**Step 4: Build and verify**

Run: `cargo build 2>&1 | tail -5`
Expected: successful build

**Step 5: Commit**

```bash
git add src/commands/sim.rs src/main.rs
git commit -m "feat(sim): add `sim netlist` command for programmatic netlist regeneration"
```

---

### Task 4: Guard against dangerous SKILL commands

**Files:**
- Modify: `src/client/bridge.rs:75-158` (the `execute_skill` method)

**Step 1: Add pre-flight safety check**

At the beginning of `execute_skill()`, before creating the TCP connection, add a check for known-dangerous patterns:

```rust
    pub fn execute_skill(&self, skill_code: &str, timeout: Option<u64>) -> Result<VirtuosoResult> {
        // Guard: block SKILL expressions that can hang the daemon
        if let Some(warning) = check_dangerous_skill(skill_code) {
            return Err(VirtuosoError::Execution(warning));
        }

        let timeout = timeout.unwrap_or(self.timeout);
        // ... rest unchanged
```

Then add the helper function at the bottom of bridge.rs (before the existing `escape_skill_string` function):

```rust
fn check_dangerous_skill(code: &str) -> Option<String> {
    // system() with recursive find can hang the daemon indefinitely
    if code.contains("system(") || code.contains("sh(") {
        let lower = code.to_lowercase();
        if lower.contains("find /") || lower.contains("find \"/") {
            return Some(
                "Blocked: system()/sh() with recursive 'find /' can hang the SKILL daemon. \
                 Use a specific directory instead (e.g., find /home/...)."
                    .into(),
            );
        }
    }
    None
}
```

**Step 2: Build and verify**

Run: `cargo build 2>&1 | tail -5`
Expected: successful build

**Step 3: Commit**

```bash
git add src/client/bridge.rs
git commit -m "fix(bridge): guard against dangerous SKILL commands that hang the daemon"
```
