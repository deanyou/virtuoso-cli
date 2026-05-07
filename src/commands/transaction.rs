//! Transaction CLI commands.

use crate::client::bridge::VirtuosoClient;
use crate::error::Result;
use serde_json::{json, Value};

pub fn begin(id: &str, lib: &str, cell: &str, view: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    client.tx_begin(id, lib, cell, view)?;
    Ok(json!({ "status": "transaction_began", "id": id }))
}

pub fn commit() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    client.tx_commit()?;
    Ok(json!({ "status": "transaction_committed" }))
}

pub fn rollback() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    client.tx_rollback()?;
    Ok(json!({ "status": "transaction_rolled_back" }))
}

pub fn diff() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let diff = client.tx_diff()?;
    let (id, _snap) = client
        .tx_status()
        .ok_or_else(|| crate::error::VirtuosoError::Execution("no active transaction".into()))?;
    Ok(json!({ "status": "ok", "transaction": id, "diff": diff }))
}

pub fn status() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    if let Some((id, snap)) = client.tx_snapshot() {
        Ok(json!({
            "status": "active",
            "id": id,
            "lib": snap.lib,
            "cell": snap.cell,
            "view": snap.view,
            "instances": snap.instances.len(),
            "nets": snap.nets.len(),
            "pins": snap.pins.len(),
        }))
    } else {
        Ok(json!({ "status": "no_active_transaction" }))
    }
}
