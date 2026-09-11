use crate::client::bridge::VirtuosoClient;
use crate::client::library_ops::LibraryOps;
use crate::client::skill_sexp::{parse_sexp, sexp_to_str_list, SexpVal};
use crate::error::{Result, VirtuosoError};
use serde_json::{json, Value};

pub fn list(ctx: &crate::context::CommandContext) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    // Use unchecked — capability check already passed at RPC dispatch level.
    // `ok_or_exec` rather than a hand-rolled `skill_ok()` check: when SKILL
    // raises, the text lands in `errors`, not `output`, and a hand-rolled check
    // that only prints `output` reports the failure with an empty reason.
    let r = client
        .execute_skill_unchecked(&LibraryOps.list(), Some(client.read_timeout()))?
        .ok_or_exec("library list")?;
    let names = sexp_to_str_list(
        &parse_sexp(r.output_unquoted())
            .map_err(|e| VirtuosoError::Execution(format!("library list parse failed: {e}")))?,
    )
    .ok_or_else(|| VirtuosoError::Execution("library list returned non-list".into()))?
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    Ok(json!({"status":"success","libraries":names}))
}

/// Every cell in `lib`, each with its views. The delete manifest is built from
/// this, so it reports what the design database says rather than what the
/// filesystem happens to show.
pub fn list_cells(
    ctx: &crate::context::CommandContext,
    lib: &str,
    pattern: Option<&str>,
) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;
    let skill = LibraryOps.list_cells(lib, pattern);
    // Use unchecked — capability check already passed at RPC dispatch level.
    // `ok_or_exec_nil_ok`, not `ok_or_exec`: `lib~>cells` is nil for a library
    // with no cells, and an empty library is an empty answer, not a failure —
    // `parse_cell_rows` says so too. A library that does not exist raises in
    // SKILL, which arrives as a NAK and still errors here.
    let r = client
        .execute_skill_unchecked(&skill, Some(client.read_timeout()))?
        .ok_or_exec_nil_ok("library list_cells")?;
    let parsed = parse_sexp(r.output_unquoted())
        .map_err(|e| VirtuosoError::Execution(format!("library list_cells parse failed: {e}")))?;
    let cells = parse_cell_rows(&parsed)?;
    Ok(json!({
        "status": "success",
        "lib": lib,
        "count": cells.len(),
        "cells": cells,
    }))
}

/// `(("cell" ("view" …)) …)` → `[{"name": …, "views": [...]}, …]`.
///
/// An empty library comes back as `nil`, which is a legitimate answer (a library
/// with no cells), not a parse failure.
fn parse_cell_rows(val: &SexpVal) -> Result<Vec<Value>> {
    let rows = match val {
        SexpVal::Nil => return Ok(Vec::new()),
        SexpVal::List(rows) => rows,
        other => {
            return Err(VirtuosoError::Execution(format!(
                "library list_cells returned non-list: {other:?}"
            )))
        }
    };
    rows.iter()
        .map(|row| {
            let SexpVal::List(parts) = row else {
                return Err(VirtuosoError::Execution(format!(
                    "library list_cells: expected (name views), got {row:?}"
                )));
            };
            let name = parts
                .first()
                .and_then(SexpVal::as_str)
                .ok_or_else(|| VirtuosoError::Execution("library list_cells: cell with no name".into()))?;
            // A cell whose views list is nil has no views — an empty list, not an error.
            let views: Vec<String> = match parts.get(1) {
                Some(v @ SexpVal::List(_)) => sexp_to_str_list(v)
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .collect(),
                _ => Vec::new(),
            };
            Ok(json!({"name": name, "views": views}))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(src: &str) -> Vec<Value> {
        parse_cell_rows(&parse_sexp(src).expect("parses")).expect("converts")
    }

    #[test]
    fn parses_cells_with_their_views() {
        let out = rows(r#"(("amp" ("schematic" "symbol")) ("amp" ("schematic")))"#);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["name"], "amp");
        assert_eq!(out[0]["views"], json!(["schematic", "symbol"]));
        assert_eq!(out[1]["views"], json!(["schematic"]));
    }

    /// An empty library is an answer, not a failure — the delete manifest has to
    /// be able to say "nothing here" without an error.
    #[test]
    fn an_empty_library_is_an_empty_list() {
        assert!(rows("nil").is_empty());
    }

    #[test]
    fn a_cell_with_no_views_keeps_an_empty_view_list() {
        let out = rows(r#"(("half_made" nil))"#);
        assert_eq!(out[0]["name"], "half_made");
        assert_eq!(out[0]["views"], json!([]));
    }

    #[test]
    fn a_row_that_is_not_a_pair_is_an_error() {
        let bad = parse_sexp(r#"("just_a_name")"#).expect("parses");
        assert!(parse_cell_rows(&bad).is_err());
    }
}
