use crate::client::bridge::VirtuosoClient;
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

pub fn close(ctx: &crate::context::CommandContext) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    let result = client.close_current_cellview()?;

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
