# Borrow from virtuoso-bridge-lite — Review & Plan

> Reference: https://github.com/Arcadia-1/virtuoso-bridge-lite
> Surveyed: 2026-06-13 (HEAD: `2110dde docs(spectre): rescue measured Spectre power-user notes from stash`)

## 0. Upstream cross-pollination baseline

The two repos are **siblings, not forks** (no shared commits; only parallel history). Our `d824ded feat: adopt virtuoso-bridge-lite patterns from recent commits` (2026-05-25) was the explicit cross-pollination commit. After that, the bridge-side convergence is mostly complete:

| Upstream feature | vbl-review commit | Adopted in our repo? | Our commit |
|---|---|---|---|
| Auto-load streamout/schematic/si packages at startup | (vbl-review analogue) | ✅ | `ca69a1a` + fixup `1f8137e` |
| Cross-user daemon guard | `3cc7fb0` | ✅ | `54603d3` (`guard_cross_user`, `VB_ALLOW_CROSS_USER_DAEMON`) |
| Window-kind classifier | (d7c3732 era) | ✅ | `d7c3732 feat(windows,spectre): window-kind classifier + parallel batch simulation` |
| Profile-isolated scratch dirs | `ca6b133` | ✅ | `e13431e feat(tunnel,profile): profile-isolated setup dir + multi-scope binding` |
| Venv profile binding | `f2211d2` | ✅ | `src/profile.rs` `.vcli-profile` resolver |
| aarch64 cross-compiled release | `b6fc4e0` (named) | ✅ | `b6fc4e0`, `ddf369f`, `961037d` |
| SKILL Finder + `--include-desc` | `a9da9ba`, `93ec970` | ✅ | `041b4af`, `71aca94` (remote cache), `80ec6e5` (real .fnd) |
| `VB_SPECTRE_BIN` | `c2a0653` | ✅ | `fbdfe73` |
| `maestro` set_simulator_mode portable | `f9389e0` | ✅ | `f9389e0` |
| Parallel SpectreSimulator / `wait_all` | `aee7a93`, `91a811d`, `2592ae4` | ✅ (partial) | `src/spectre/runner.rs::run_parallel`, `d7c3732` |
| `maestro::snapshot` (4-section probe + on-disk dump) | `93001bc` | ✅ | `src/commands/maestro.rs::snapshot` |
| `add_power_labels` step in digital import | (recipe in examples) | ✅ (inline snippet) | `.claude/skills/digital-import/SKILL.md` Step 3 |

**Bottom line:** the bridge/tunnel/maestro/spectre/skill-finder surface is converged. The remaining gaps are in **X11 / OS-level integration**, **diagnostics**, **content (docs)**, and **digital-import edge cases**.

## 1. Gaps to consider borrowing

| # | Feature | vbl-review source | Priority | Why we should consider it |
|---|---|---|---|---|
| G1 | **X11 dialog dismissal (SSH bypass)** | `d139c02` + `22e737c` + `1230003` + `14fdb4a` (helper at `src/virtuoso_bridge/resources/x11_dismiss_dialog.py`) | **HIGH** | Our `vcli window dismiss-dialog` is **SKILL-only** — when a modal blocks the CIW event loop, the SKILL channel itself is dead, so our command can hang the full `VB_TIMEOUT` (30–120 s). The X11 bypass SSHs into the same host, finds the modal via `python3 + Xlib`, sends `Enter`/`Esc`/`Alt+Y`/`Alt+N`, then the SKILL channel recovers. Same host, no daemon, no lockstep. |
| G2 | **`.cdslck` sniffer** | `examples/01_virtuoso/diagnostics/sniff_cdslck.py` (reads `cat <lock>` → `owner@host:pid:start_time`, ages each) | **MEDIUM** | We mention `.cdslck` in `maestro` + `troubleshooting` references and propose `rm -f`. A first-class `vcli diag cdslck [--lib X]` would be more diagnostic and far less destructive: it reports **who holds the lock + how old**, never deletes. Fits the "skill-broken" pattern (locks not under Virtuoso's control, can't be cleared by SKILL). |
| G3 | **Power-user Spectre docs (measured data)** | `2110dde` (`skills/spectre/SKILL.md` +152 lines: ax/mx/lx/vx ENOB Δ, Verilog-A replacement rule, `save=selected`, `noiseruns` trap) | **HIGH** | Pure knowledge content; we have `sim-run`/`sim-sweep` skills but no measured-precision table. This data is in vbl-review's `skills/spectre/SKILL.md` and ready to copy/adapt. Reuse rather than re-derive. |
| G4 | **Spectre 21.1 / lab-cluster gotchas (5 issues)** | `7786645` (`skills/spectre/SKILL.md` +32 lines: `-param X=Y` broken, `parameters` re-declare, default 600s timeout, PSF `\<\>` escape, `strobeoutput=all` is **larger** not smaller) | **HIGH** | Same: pure docs. Several are silent (PSF escape, `strobeoutput` semantics) and would be valuable in our `sim-run` skill. |
| G5 | **X11 screenshot via `xwd` (SSH bypass)** | `af7e2d7`, `fe411c6` | **MEDIUM** | Our `vcli window screenshot` uses SKILL `hiReshapeImage` + `dbSaveImage` on the CIW. Works for the focused window only; some headless Xvfb setups (no DISPLAY visible to Virtuoso) fail. vbl-review's `xwd -screen` is a one-shot screenshot, never blocks SKILL. Useful for crash debugging. |
| G6 | **Visio export** | `src/virtuoso_bridge/virtuoso/visio.py` (545 lines) + `examples/test_visio_export.py` | **LOW** | Windows + Visio + pywin32 — three platform constraints we don't currently have. Would need a separate optional crate (or `pyo3` wrapper). Out of scope unless there's a concrete user need. |
| G7 | **`add_power_labels.py` standalone (full xform-based)** | `examples/01_virtuoso/digital_import/add_power_labels.py` | **LOW** | Our `digital-import` skill has a "Step 3" SKILL snippet, but it's the "simplified" form. vbl-review has the full xform-based walk that handles nested instances. Could be a `.claude/skills/digital-import/scripts/add_power_labels.il` companion. |
| G8 | **SRAM auto-detect from Verilog** | `4726417 feat(digital_import): auto-detect SRAM cell from Verilog` | **LOW** | Probes `${top}_import.v` and `${top}.ipg_import_elc.v` in that order — a small but real ergonomic win. Could fold into our `digital-import` skill as a fallback when the user forgets `--verilog`. |
| G9 | **Windows persistent shell for SSH** | `bd1c01e fix(ssh,cli): stabilize startup + probe flow; Windows persistent-shell` | **LOW (skip)** | Win-only; we have a Linux-first user base. Defer until reported. |
| G10 | **Traffic / stats badges** | multiple `chore(stats)` | **N/A** | vbl-review uses them to show a public project's momentum. Not relevant to a CLI tool. |

## 2. Plan — what to actually pick up

Three categories, ordered by ROI:

### A. X11-bypass path for the dialog deadlock (G1) — concrete spec

**Why first:** this is the only one that **fixes a real failure mode** we already have (the user just had to time out an `evalstring` once after a modal popped up — see the `d139c02` commit message: "save you when SKILL channel deadlocks on a modal"). Our current SKILL-only dismiss_dialog is **strictly worse** than the vbl-review one in that scenario.

**API surface (proposed):**
```
vcli window dismiss-dialog [--x11] [--action enter|escape|alt-y|alt-n] [--dry-run]
```

When `--x11` is passed (default in a follow-up if `VB_USE_X11_DISMISS=1`), the flow is:
1. SSH into `$VB_REMOTE_HOST` (using existing `transport/ssh.rs::SSHRunner`).
2. Auto-detect `DISPLAY` + `XAUTHORITY` by reading `/proc/<virtuoso-pid>/environ` of the Virtuoso process (filter out `-nograph`).
3. Upload a small `x11_dismiss_dialog.py` helper to the existing scratch dir (`/tmp/virtuoso_bridge/<client_id>/`).
4. Run `python3 <helper>` over SSH; the helper uses `Xlib` (`XQueryTree` + `XGetWindowProperty(_NET_WM_NAME)`) to find dialogs whose frame title matches `virtuoso`/`libManager` and sends the chosen keypress to the frame's child window.
5. Stream the JSON output, parse `{window_id, title, x, y, w, h}`, exit code 0 = dismissed, 1 = no dialogs, 2 = error.

When `--x11` is **not** passed, fall back to the existing SKILL path (kept for backward compatibility).

**Files / structure:**
- `src/transport/x11.rs` — new module: detect display, upload helper, run helper, parse output.
- `resources/x11_dismiss_dialog.py` — vendored helper (modify in place, don't fetch at runtime).
- `src/commands/window.rs::dismiss_dialog` — add `--x11` flag, dispatch to either path.
- `src/client/window_ops.rs` — leave unchanged; the X11 path doesn't go through SKILL.

**Why a vendored helper, not a runtime dependency on vbl-review:**
- vbl-review uses `python2` in the script header but `python3` everywhere; vbl-review's `bd1c01e` fixed this. We can use `python3` from the start.
- vbl-review depends on `Xlib` (Python); we should pin `python3-Xlib` install instructions in the SKILL doc.

**Testing strategy (no live Virtuoso required):**
- `transport/x11.rs` helpers take a trait-abstracted `SshRunner`, so unit tests use a mock that returns canned `pgrep` / `cat` / `python3` output.
- The vendored Python helper is run through `cargo test` via a separate `--ignored` integration test, gated on `python3` being on `PATH` and `python3-Xlib` importable.

### B. `.cdslck` sniffer (G2) — small, high-leverage

**Why:** when "this view won't open" is reported, the fastest path to a fix is "who holds the lock + how old". Currently our skills say `rm -f`, which is destructive. The vbl-review sniffer is read-only and reports owner/host/pid/age.

**API surface:**
```
vcli diag cdslck <LIB> [--view maestro] [--remote]
```

1. `ddGetObj("<LIB>")~>readPath` → library root.
2. SSH `find <readPath> -name '.cdslck' -print` (parallel-safe, doesn't go through SKILL).
3. For each: `cat <path>` to get `owner@host:pid:start_time`; `stat -c %Y` for mtime.
4. JSON: `{cellview, owner, host, pid, age_seconds}` rows.

**Files:**
- `src/commands/diag.rs` (new) — `pub fn cdslck(lib: &str, view_filter: Option<&str>) -> Result<Value>`.
- `src/main.rs` — `Diag` clap subcommand, `cdslck` sub-sub.
- `src/rpc/schema.rs` — register `diag.cdslck` method.
- `.claude/skills/diag/SKILL.md` (new, thin) — recipe for the use case.

**Why not the SKILL path:** SKILL `ddGetObj` can hang if the library root is being held by a remote Virtuoso; SSH `find` cannot.

### C. Knowledge transfer (G3, G4) — pure docs

**Action:** copy the 5 Spectre 21.1 gotchas (G4) and the 4 measured-precision sections (G3) into our `sim-run` and `sim-sweep` skills, with attribution. Two short docs PRs, no code change.

Proposed locations:
- G3 (`ax`/`mx`/`lx`/`vx` table, Verilog-A replacement rule, `save=selected`, `noiseruns` trap) → `.claude/skills/sim-run/SKILL.md` new section "**Spectre mode selection (measured)**"
- G4 (5 gotchas) → `.claude/skills/sim-run/SKILL.md` new section "**Spectre 21.1 + lab-cluster traps**"

Both are factual content; vbl-review's measured data is on an 11-bit SAR ADC, which is a typical workload, so the table is broadly useful.

## 3. What we should NOT adopt (and why)

| Item | Why skip |
|---|---|
| **G6 (Visio export)** | Windows + Visio + pywin32; niche; we have no other Windows-only code path. |
| **G9 (Windows persistent shell)** | Win-only. Reopen if a user reports it. |
| **G10 (traffic stats)** | vbl-review is a public project that uses traffic stats as social proof. We are not. |
| **MS1: `stat <event>` and `screenshot` window totals** | Same as G10 — not a tool, a metrics dashboard. |
| **MS1: restyle-labels/label-restyle code in vbl-review** | We already cover it in `digital-import` Step 4. |

## 4. Open questions to confirm before implementation

1. **G1 helper packaging:** are we OK shipping a vendored `x11_dismiss_dialog.py` in `resources/`, or do we want to add it as a build-time dep (e.g. via `git submodule`)? Recommendation: vendored (keeps offline behavior consistent with our existing `upload_file` semantics).
2. **G2 command location:** new top-level `vcli diag` subcommand, or fold into `vcli maestro diag cdslck`? `vcli diag` is cleaner if we expect to add more diagnostics later (`diag cdslck`, `diag license`, `diag oa-refs`, …).
3. **G3/G4 attribution style:** copy verbatim (with a "Source: virtuoso-bridge-lite, 2026-05-31" header) or paraphrase? Recommendation: keep the data verbatim, write our own prose around it.

## 5. Estimated effort

| Item | Estimate | Notes |
|---|---|---|
| G1 (X11 dismiss) | 1.5 days | New module + 1 vendored helper + tests + docs. |
| G2 (cdslck sniffer) | 0.5 day | SSH `find` + `cat`/`stat` + JSON; ~80 LOC. |
| G3 + G4 (docs) | 0.25 day each | Pure markdown edits. |
| **Total** | **~2.5 days** | |

G1 is the largest single piece and the only one that meaningfully changes runtime behavior. The other two are clear wins for low effort.

## 6. Status

| Section | Status |
|---|---|
| 0 — Cross-pollination baseline | ✅ verified via `git log --oneline -15` + repo comparison |
| 1 — Gap inventory | ✅ identified G1–G10 |
| 2 — Plan A/B/C | ⏳ awaiting user OK to implement |
| 3 — Out-of-scope | ✅ listed |
| 4 — Open questions | ⏳ waiting on user input |
| 5 — Effort | ✅ rough estimate |
