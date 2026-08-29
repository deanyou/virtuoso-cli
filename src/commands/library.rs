use crate::client::bridge::VirtuosoClient;
use crate::client::library_ops::LibraryOps;
use crate::client::skill_sexp::{parse_sexp, sexp_to_str_list};
use crate::error::{Result, VirtuosoError};
use serde_json::{json, Value};

pub fn list() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    // Use unchecked — capability check already passed at RPC dispatch level
    let r = client.execute_skill_unchecked(&LibraryOps.list(), Some(client.read_timeout()))?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "library list failed: {}",
            r.output
        )));
    }
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
