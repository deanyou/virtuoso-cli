use crate::client::bridge::VirtuosoClient;
use crate::client::cell_ops;
use crate::error::Result;
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
// Exits non-zero only when off_grid > 0 (real coordinate drift).
// ============================================================================

use crate::client::skill_sexp::{parse_sexp, SexpVal};

/// Parse the shape-grid-audit SKILL output.
///
/// SKILL returns:
/// ```text
/// ((layer1 on off) (layer2 on off) ... (tails ((net count) ...)))
/// ```
///
/// Handles:
/// - `(0 0)` empty cell (no shapes)
/// - `(layer1 12 0 layer2 8 1 ...)` flat lists (some tools emit flat)
/// - Missing `tails` key
///
/// Returns `Vec<(layer, on_grid, off_grid)>` and `Vec<(net, count)>` for tails.
/// One layer's grid audit result: (layer_name, on_grid_count, off_grid_count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridLayer(pub String, pub u32, pub u32);

/// One detected decorative supply rail tail: (net_name, shape_count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridTail(pub String, pub u32);

/// Parse the SKILL output of `shape_grid_audit_skill`.
///
/// Public for integration tests to exercise the real parser (round-2 M4:
/// the test file used to ship a clone and never touched this code).
/// Returns `(layers, tails)` — `tails` is empty for pure grid-audit
/// outputs and populated when `shape_grid_audit_skill` also reports
/// decorative rail tails.
pub fn parse_grid_audit_output(raw: &str) -> (Vec<GridLayer>, Vec<GridTail>) {
    let parsed = match parse_sexp(raw) {
        Ok(v) => v,
        Err(_) => return (Vec::new(), Vec::new()),
    };

    let mut layers = Vec::new();
    let mut tails = Vec::new();

    if let SexpVal::List(items) = parsed {
        for item in items {
            if let SexpVal::List(ref sub) = item {
                if sub.len() >= 2 {
                    // Check for "tails" marker FIRST.
                    if let SexpVal::Atom(ref a) = sub[0] {
                        if a == "tails" {
                            if let SexpVal::List(tail_items) = &sub[1] {
                                for tail_item in tail_items {
                                    if let SexpVal::List(tp) = tail_item {
                                        if tp.len() >= 2 {
                                            let net = tp[0].as_str().unwrap_or("").to_string();
                                            let count = tp[1]
                                                .as_str()
                                                .and_then(|s| s.parse::<u32>().ok())
                                                .unwrap_or(1);
                                            tails.push(GridTail(net, count));
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                    }

                    // Layer entry: (layerName onCount offCount) — requires at least 3 elements
                    if sub.len() >= 3 {
                        let layer = sub[0].as_str().unwrap_or("").to_string();
                        let on = sub[1]
                            .as_str()
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        let off = sub[2]
                            .as_str()
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        layers.push(GridLayer(layer, on, off));
                    } else {
                        // Flat form: (layerName count) — only 2 elements, off=0
                        let layer = sub[0].as_str().unwrap_or("").to_string();
                        let on = sub[1]
                            .as_str()
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        layers.push(GridLayer(layer, on, 0));
                    }
                }
            }
        }
    }

    // Sort by off_grid count descending
    layers.sort_by_key(|l| std::cmp::Reverse(l.2));
    (layers, tails)
}

/// Verify a cellview's native grid fidelity (read-only).
///
/// Opens the cellview read-only, reads `tech~>userGridPrecision`,
/// checks every shape's bBox for grid drift, and detects decorative
/// supply rail tails.
///
/// Returns structured JSON with per-layer results and a top-level
/// verdict. Exits non-zero only when `off_grid > 0`.
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

    let (layers, tails) = parse_grid_audit_output(r.output_unquoted());

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
