use crate::client::bridge::VirtuosoClient;
use crate::client::cell_ops;
use crate::error::{Result, VirtuosoError};
use serde_json::{json, Value};

pub fn open(
    ctx: &crate::context::CommandContext,
    lib: &str,
    cell: &str,
    view: &str,
    mode: &str,
    dry_run: bool,
) -> Result<Value> {
    if dry_run {
        return Ok(json!({
            "action": "open",
            "resource": "cell",
            "target": {
                "lib": lib,
                "cell": cell,
                "view": view,
                "mode": mode,
            },
            "dry_run": true,
        }));
    }

    let client = VirtuosoClient::from_context(ctx)?;
    let result = client.open_cell_view(lib, cell, view, mode)?;

    Ok(json!({
        "status": if result.ok() { "success" } else { "error" },
        "lib": lib,
        "cell": cell,
        "view": view,
        "output": result.output,
        "errors": result.errors,
    }))
}

pub fn save(ctx: &crate::context::CommandContext) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    let result = client.save_current_cellview()?;

    Ok(json!({
        "status": if result.ok() { "success" } else { "error" },
        "output": result.output,
    }))
}

pub fn close(ctx: &crate::context::CommandContext, save: bool) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    let result = client.close_current_cellview(save)?;

    Ok(json!({
        "status": if result.ok() { "success" } else { "error" },
        "output": result.output,
    }))
}

pub fn info(ctx: &crate::context::CommandContext) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    let (lib, cell, view) = client.get_current_design()?;

    Ok(json!({
        "lib": lib,
        "cell": cell,
        "view": view,
    }))
}

// ============================================================================
// Cell Verify Native  (analogous to PR #175)
//
// Read-only verification: catches grid quantization drift and decorative
// supply rail tails after a canvas import pipeline.
//
// This function never fails the process — it returns the JSON breakdown
// either way. The CLI dispatch in `main.rs` turns a `"drift"` status into a
// non-zero exit, so CI sees the failure while the JSON still carries the
// per-layer breakdown.
// ============================================================================

use crate::client::skill_sexp::{parse_sexp, SexpVal};

/// Parse a non-negative integer out of a SKILL number.
///
/// SKILL prints integers as `497`, but a value that went through arithmetic
/// can come back as `497.0`, so both are accepted. Anything else is not a
/// count and is rejected by the caller rather than silently read as zero.
fn audit_count(value: &SexpVal) -> Option<u32> {
    let text = value.as_str()?;
    if let Ok(n) = text.parse::<u32>() {
        Some(n)
    } else {
        text.parse::<f64>().ok().map(|f| f as u32)
    }
}

/// Parse the shape-grid-audit SKILL output into per-layer counts and tails.
///
/// Returns `None` when `raw` is not the format the audit SKILL emits: a
/// value that is not a list, a list entry that is not a tuple, a layer
/// tuple that does not have exactly the three elements `(layer on off)`,
/// or a count that is not a number. Callers that must fail closed on an
/// unreadable audit use [`grid_audit_output_is_well_formed`].
fn parse_grid_audit_sexp(raw: &str) -> Option<(Vec<GridLayer>, Vec<GridTail>)> {
    let items = match parse_sexp(raw).ok()? {
        SexpVal::List(items) => items,
        _ => return None,
    };

    let mut layers = Vec::new();
    let mut tails = Vec::new();

    for item in items {
        let sub = match item {
            SexpVal::List(sub) => sub,
            _ => return None,
        };

        // The tails section is marked by a literal key. The SKILL emits it
        // as a quoted string, so it is matched on the string value and not
        // on the s-expression atom type.
        if sub.first().and_then(|v| v.as_str()) == Some(cell_ops::TAILS_KEY) {
            if let Some(SexpVal::List(tail_items)) = sub.get(1) {
                for tail_item in tail_items {
                    let tp = match tail_item {
                        SexpVal::List(tp) => tp,
                        _ => return None,
                    };
                    if tp.len() != 2 {
                        return None;
                    }
                    let net = tp[0].as_str()?.to_string();
                    let count = audit_count(&tp[1])?;
                    tails.push(GridTail(net, count));
                }
            }
            continue;
        }

        // A layer entry is exactly (layer on off) — `on` and `off` are
        // per-LAYER shape counts, which is what makes the off-grid total
        // (and therefore the drift verdict) reachable at all.
        if sub.len() != 3 {
            return None;
        }
        let layer = sub[0].as_str()?.to_string();
        let on = audit_count(&sub[1])?;
        let off = audit_count(&sub[2])?;
        layers.push(GridLayer(layer, on, off));
    }

    // Sort by off_grid count descending
    layers.sort_by_key(|l| std::cmp::Reverse(l.2));
    Some((layers, tails))
}

/// One layer's grid audit result: (layer_name, on_grid_count, off_grid_count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridLayer(pub String, pub u32, pub u32);

/// One detected decorative supply rail tail: (net_name, shape_count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridTail(pub String, pub u32);

/// Parse the SKILL output of `shape_grid_audit_skill`.
///
/// The SKILL emits (see [`cell_ops::shape_grid_audit_skill`]):
///
/// ```text
/// ((layer1 on off) (layer2 on off) ... (tails ((net count) ...)))
/// ```
///
/// Returns `(layers, tails)` — `tails` is empty when the audit found no
/// decorative supply tail.
///
/// Public for integration tests to exercise the real parser (round-2 M4:
/// the test file used to ship a clone and never touched this code). The
/// output that is not the shape the SKILL emits yields no layers and no
/// tails; use [`grid_audit_output_is_well_formed`] to tell "the audit ran
/// and found nothing" apart from "the audit result is unreadable".
pub fn parse_grid_audit_output(raw: &str) -> (Vec<GridLayer>, Vec<GridTail>) {
    parse_grid_audit_sexp(raw).unwrap_or((Vec::new(), Vec::new()))
}

/// True when `raw` has the shape the audit SKILL emits.
///
/// A cellview with no shapes still yields a well-formed result — the SKILL
/// always emits the tails section — so this distinguishes "nothing to
/// report" from "the audit did not return what it should".
pub fn grid_audit_output_is_well_formed(raw: &str) -> bool {
    parse_grid_audit_sexp(raw).is_some()
}

/// Verify a cellview's native grid fidelity (read-only).
///
/// Opens the cellview read-only, reads the technology's manufacturing grid
/// resolution, checks every shape's bounding-box corners *and* vertices for
/// grid drift, and reports decorative supply rail tails — supply-net shapes
/// with a free end, i.e. a piece of rail that stops in mid-air.
///
/// Returns structured JSON with per-layer results and a top-level verdict:
///
/// * `status` — `"drift"` when at least one shape is off-grid, `"dangling"`
///   when only decorative supply tails were found, `"ok"` otherwise;
/// * `on_grid` / `off_grid` — the per-layer shape counts summed up;
/// * `decorative_tails` — the number of supply-net shapes with a free end.
///
/// This function does not exit: the CLI dispatch in `main.rs` turns a
/// `"drift"` status into a non-zero exit. An audit result that is not the
/// format the SKILL emits is an error, not a clean bill of health.
pub fn verify_native(
    ctx: &crate::context::CommandContext,
    lib: &str,
    cell: &str,
    view: &str,
) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;

    // Run the grid audit SKILL
    let skill = cell_ops::shape_grid_audit_skill(lib, cell, view);
    let r = client
        .execute_skill(&skill, None)?
        .ok_or_exec("grid audit")?;

    let raw = r.output_unquoted();
    if !grid_audit_output_is_well_formed(raw) {
        return Err(VirtuosoError::Execution(format!(
            "cell verify-native: the grid audit returned an unreadable result: {raw}"
        )));
    }
    let (layers, tails) = parse_grid_audit_output(raw);

    let total_on: u32 = layers.iter().map(|l| l.1).sum();
    let total_off: u32 = layers.iter().map(|l| l.2).sum();
    let total_tails: u32 = tails.iter().map(|t| t.1).sum();

    // Build details per layer (already sorted by off_grid desc inside the parser).
    let details: Vec<Value> = layers
        .iter()
        .map(|layer| {
            let GridLayer(name, on, off) = layer;
            let total = on + off;
            let off_rate = if total > 0 {
                (*off as f64) / (total as f64)
            } else {
                0.0
            };
            json!({
                "layer": name,
                "on_grid": on,
                "off_grid": off,
                "off_grid_rate": format!("{:.2}", off_rate),
            })
        })
        .collect();

    // Decorative tail details.
    let tail_details: Vec<Value> = tails
        .iter()
        .map(|tail| {
            let GridTail(net, count) = tail;
            json!({
                "net": net,
                "tail_count": count,
            })
        })
        .collect();

    // Determine overall status
    let status = if total_off > 0 {
        "drift"
    } else if total_tails > 0 {
        "dangling"
    } else {
        "ok"
    };

    Ok(json!({
        "status": status,
        "on_grid": total_on,
        "off_grid": total_off,
        "decorative_tails": total_tails,
        "lib": lib,
        "cell": cell,
        "view": view,
        "details": details,
        "tail_details": tail_details,
    }))
}
