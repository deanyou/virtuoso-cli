use crate::client::bridge::VirtuosoClient;
use crate::client::skill_sexp::{parse_sexp, SexpVal};
use crate::client::symbol_ops::SymbolOps;
use crate::error::{Result, VirtuosoError};
use serde_json::{json, Value};

pub fn inspect(lib: &str, cell: &str, view: &str, view_type: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let r = client.execute_skill(
        &SymbolOps::new().inspect(lib, cell, view, view_type),
        Some(client.read_timeout()),
    )?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "symbol inspect failed: {}",
            r.output
        )));
    }
    let data = sexp_json(r.output_unquoted())?;
    if let Some(tag) = data
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_str)
    {
        if tag == "notFound" {
            return Err(VirtuosoError::NotFound(format!(
                "symbol {lib}/{cell}/{view} not found"
            )));
        }
        if tag == "failed" {
            return Err(VirtuosoError::Execution(format!(
                "symbol inspect failed: {data}"
            )));
        }
    }
    Ok(json!({"status":"success","lib":lib,"cell":cell,"view":view,"data":data}))
}

pub fn generate(
    lib: &str,
    cell: &str,
    schematic_view: &str,
    symbol_view: &str,
    sort_pins: Option<&str>,
) -> Result<Value> {
    if schematic_view == symbol_view {
        return Err(VirtuosoError::Config(
            "schematic_view and symbol_view must differ".into(),
        ));
    }
    if let Some(sort) = sort_pins {
        if !matches!(sort, "alphanumeric" | "geometric") {
            return Err(VirtuosoError::Config(
                "sort_pins must be alphanumeric or geometric".into(),
            ));
        }
    }
    let client = VirtuosoClient::from_env()?;
    let r = client.execute_skill(
        &SymbolOps::new().generate(lib, cell, schematic_view, symbol_view, sort_pins),
        Some(client.read_timeout()),
    )?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "symbol generate failed: {}",
            r.output
        )));
    }
    let data = sexp_json(r.output_unquoted())?;
    if let Some(tag) = data
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_str)
    {
        if tag == "conflict" {
            return Err(VirtuosoError::Conflict(format!(
                "symbol target {lib}/{cell}/{symbol_view} already exists"
            )));
        }
        if tag == "notFound" {
            return Err(VirtuosoError::NotFound(format!(
                "source schematic {lib}/{cell}/{schematic_view} not found"
            )));
        }
        if tag == "failed" {
            return Err(VirtuosoError::Execution(format!(
                "symbol generate failed: {data}"
            )));
        }
    }
    Ok(
        json!({"status":"success","lib":lib,"cell":cell,"source_view":schematic_view,"symbol_view":symbol_view,"data":data}),
    )
}

fn sexp_json(raw: &str) -> Result<Value> {
    fn convert(v: SexpVal) -> Value {
        match v {
            SexpVal::Nil => Value::Null,
            SexpVal::Bool(b) => json!(b),
            SexpVal::Str(s) | SexpVal::Atom(s) => json!(s),
            SexpVal::List(xs) => Value::Array(xs.into_iter().map(convert).collect()),
        }
    }
    Ok(convert(parse_sexp(raw).map_err(|e| {
        VirtuosoError::Execution(format!("symbol response parse failed: {e}"))
    })?))
}
