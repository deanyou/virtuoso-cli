use crate::client::bridge::VirtuosoClient;
use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::models::SessionInfo;
use crate::skill_finder::{SearchMode, SKILLFinder};
use serde_json::{json, Value};

pub fn exec(code: &str, timeout: u64, readonly: bool) -> Result<Value> {
    let mut client = VirtuosoClient::from_env()?;
    if readonly {
        client = client.with_sandbox_mode();
    }
    let result = client.execute_skill(code, Some(timeout))?;

    Ok(json!({
        "status": if result.ok() { "success" } else { "error" },
        "output": result.output,
        "errors": result.errors,
        "warnings": result.warnings,
        "execution_time": result.execution_time,
    }))
}

/// Run `code` concurrently against every live local session.
/// Each session gets its own TCP connection in a scoped thread.
/// Returns per-session results; exit is non-zero only when every session fails.
pub fn broadcast(code: &str, timeout: u64) -> Result<Value> {
    let sessions = SessionInfo::list_alive();
    if sessions.is_empty() {
        return Err(VirtuosoError::NotFound("no live sessions found".into()));
    }

    // Collect results preserving original session order (by index)
    let results: Vec<(usize, Value)> = std::thread::scope(|s| {
        let handles: Vec<_> = sessions
            .iter()
            .enumerate()
            .map(|(idx, session)| {
                let id = session.id.clone();
                let port = session.port;
                s.spawn(move || {
                    let client = VirtuosoClient::new("127.0.0.1", port, timeout);
                    match client.execute_skill(code, Some(timeout)) {
                        Ok(r) => (
                            idx,
                            json!({
                                "session": id,
                                "ok": r.skill_ok(),
                                "output": r.output,
                            }),
                        ),
                        Err(e) => (
                            idx,
                            json!({
                                "session": id,
                                "ok": false,
                                "error": e.to_string(),
                            }),
                        ),
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| (0, json!({"ok": false, "error": "thread panicked"})))
            })
            .collect()
    });

    // Sort by original index to preserve session list order
    let mut results: Vec<Value> = results.into_iter().map(|(_, v)| v).collect();
    results.sort_by(|a, b| {
        let idx_a = sessions
            .iter()
            .position(|s| s.id == a["session"].as_str().unwrap_or(""))
            .unwrap_or(0);
        let idx_b = sessions
            .iter()
            .position(|s| s.id == b["session"].as_str().unwrap_or(""))
            .unwrap_or(0);
        idx_a.cmp(&idx_b)
    });

    let n_ok = results
        .iter()
        .filter(|r| r["ok"].as_bool().unwrap_or(false))
        .count();
    let status = if n_ok == results.len() {
        "success"
    } else if n_ok == 0 {
        "error"
    } else {
        "partial"
    };

    if n_ok == 0 {
        return Err(VirtuosoError::Execution(format!(
            "broadcast failed on all {} sessions",
            results.len()
        )));
    }

    Ok(json!({
        "status": status,
        "sessions": results.len(),
        "ok": n_ok,
        "results": results,
    }))
}

pub fn load(file: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;

    if !std::path::Path::new(file).exists() {
        return Err(VirtuosoError::NotFound(format!("file not found: {file}")));
    }

    let result = client.load_il(file)?;

    Ok(json!({
        "status": if result.ok() { "success" } else { "error" },
        "file": file,
        "output": result.output,
        "errors": result.errors,
    }))
}

/// Execute inline SKILL expressions — companion to `load` for one-liners.
///
/// Wraps input in `progn(\n<user>\n)` to:
/// - Enable multi-statement execution without wrapping in progn yourself
/// - Prevent trailing `; comment` from swallowing the closing paren
///
/// Supports two input modes:
/// - `code` provided directly: single expression or multi-line block
/// - `stdin == true`: read from stdin (avoids shell quoting pain)
pub fn eval(code: Option<String>, stdin: bool) -> Result<Value> {
    use std::io::Read;

    // Input validation: mutually exclusive modes
    if stdin && code.is_some() {
        return Err(VirtuosoError::Config(
            "pass SKILL via argument OR --stdin, not both".into(),
        ));
    }

    let skill = if stdin {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(|e| VirtuosoError::Io(std::io::Error::other(e)))?;
        if input.trim().is_empty() {
            return Err(VirtuosoError::Config(
                "empty SKILL expression from stdin".into(),
            ));
        }
        input
    } else {
        let c = code.ok_or_else(|| VirtuosoError::Config("no SKILL expression provided".into()))?;
        if c.trim().is_empty() {
            return Err(VirtuosoError::Config("empty SKILL expression".into()));
        }
        c
    };

    // Wrap in progn on its own lines so that:
    // - Multi-statement inputs work without user adding progn
    // - Trailing `; comment` doesn't swallow the closing paren
    // - Embedded newlines flow through unchanged
    let wrapped = format!("progn(\n{}\n)", skill);

    let client = VirtuosoClient::from_env()?;
    let result = client.execute_skill(&wrapped, None)?;

    Ok(json!({
        "status": if result.ok() { "success" } else { "error" },
        "output": result.output,
        "errors": result.errors,
        "warnings": result.warnings,
        "execution_time": result.execution_time,
    }))
}

/// Search SKILL function names using the Cadence SKILL Finder database.
///
/// Requires VB_SPECTRE_DIR or VB_CADENCE_CSHRC to locate the Cadence installation,
/// or VB_SKILL_FINDER_DIR to specify the path directly.
///
/// # Arguments
///
/// * `query` - Search string
/// * `mode` - Search mode: fuzzy (default), prefix, suffix, exact, regex
/// * `limit` - Maximum results (default: 50)
pub fn find(query: &str, mode: &str, limit: usize) -> Result<Value> {
    let search_mode: SearchMode = mode.parse().unwrap_or(SearchMode::Fuzzy);
    let cfg = Config::from_env()?;

    // Try to find the SKILL Finder directory
    let finder_dir = find_skill_finder_dir(&cfg)?;

    let mut finder = SKILLFinder::new();

    if let Some(dir) = finder_dir {
        finder
            .load(&dir)
            .map_err(|e| VirtuosoError::Config(format!("failed to load SKILL Finder: {}", e)))?;
    }

    let results: Vec<_> = finder
        .search(query, search_mode, limit)
        .into_iter()
        .map(|e| {
            json!({
                "name": e.name,
                "syntax": e.syntax,
                "description": e.description,
                "source": e.source_file
            })
        })
        .collect();

    Ok(json!({
        "query": query,
        "mode": search_mode.to_string(),
        "count": results.len(),
        "entries": results,
    }))
}

/// Get detailed More Info documentation for a specific SKILL function.
///
/// This queries the Cadence More Info system via the Virtuoso bridge.
pub fn info(func_name: &str) -> Result<Value> {
    if func_name.is_empty() {
        return Err(VirtuosoError::Config(
            "function name is required".into(),
        ));
    }

    let client = VirtuosoClient::from_env()?;

    // Use Virtuoso's More Info system via SKILL
    let skill_code = format!(
        r#"let((result)
  when(boundp('mfGetMoreInfo
    result = mfGetMoreInfo("{}" "{}")
    if(result then result else nil)
  )
)"#,
        "$象牙/doc/api_more_info/api_more_info.html",
        func_name
    );

    let result = client.execute_skill(&skill_code, None)?;

    if !result.skill_ok() {
        return Ok(json!({
            "func_name": func_name,
            "found": false,
            "error": "function not found or More Info not available"
        }));
    }

    // Parse the result - typically returns HTML or nil
    let output = result.output.trim();
    if output.is_empty() || output == "nil" {
        return Ok(json!({
            "func_name": func_name,
            "found": false
        }));
    }

    Ok(json!({
        "func_name": func_name,
        "found": true,
        "raw": output,
    }))
}

/// Find the SKILL Finder directory from config.
///
/// Priority:
/// 1. VB_SKILL_FINDER_DIR env var
/// 2. Discover from Cadence installation (via VB_CADENCE_CSHRC or spectre path)
fn find_skill_finder_dir(cfg: &Config) -> Result<Option<std::path::PathBuf>> {
    // 1. Check VB_SKILL_FINDER_DIR
    if let Ok(dir) = std::env::var("VB_SKILL_FINDER_DIR") {
        if !dir.is_empty() && std::path::Path::new(&dir).exists() {
            tracing::debug!("Using VB_SKILL_FINDER_DIR: {}", dir);
            return Ok(Some(std::path::PathBuf::from(dir)));
        }
    }

    // 2. For local, try to discover from spectre path
    if !cfg.is_remote() {
        if let Ok(spectre_path) = std::process::Command::new("which")
            .arg("spectre")
            .output()
        {
            let path = String::from_utf8_lossy(&spectre_path.stdout).trim().to_string();
            if !path.is_empty() && path != "spectre" {
                let ic_dir = std::path::Path::new(&path)
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent());
                if let Some(ic) = ic_dir {
                    let finder_dir = ic.join("doc/finder/SKILL");
                    if finder_dir.exists() {
                        tracing::debug!("Found SKILL Finder at: {}", finder_dir.display());
                        return Ok(Some(finder_dir));
                    }
                }
            }
        }
    }

    // 3. For remote, try to discover using SSH if available
    if cfg.is_remote() {
        // Try to find via SSH using the cadence cshrc
        if let Some(ref cshrc) = cfg.cadence_cshrc {
            if let Ok(discovered) = discover_skill_finder_remote(&cfg.ssh_target(), cshrc) {
                if let Some(path) = discovered {
                    tracing::debug!("Discovered SKILL Finder on remote: {}", path.display());
                    return Ok(Some(path));
                }
            }
        }
    }

    Ok(None)
}

/// Discover SKILL Finder directory on a remote server via SSH.
fn discover_skill_finder_remote(
    target: &str,
    cadence_cshrc: &str,
) -> std::result::Result<Option<std::path::PathBuf>, String> {
    use std::process::Command;

    let sh_cshrc = cadence_cshrc.replace('\'', "'\"'\"'\"'\"'\"");

    let find_script = format!(
        r#"eval "$(csh -c 'source {}; env' 2>/dev/null | grep -E '^(PATH|LM_LICENSE_FILE|CDS)=' | sed 's/^/export /')" 2>/dev/null
which spectre 2>/dev/null || echo NOTFOUND"#,
        sh_cshrc
    );

    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=10"])
        .arg(target)
        .arg(&find_script)
        .output()
        .map_err(|e| format!("SSH failed: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if stdout.contains("NOTFOUND") || stdout.is_empty() {
        return Ok(None);
    }

    let spectre_path = stdout.trim();

    // Walk up from spectre to find doc/finder/SKILL
    let walk_script = format!(
        r#"p="{}"
while [ -n "$p" ] && [ "$p" != "/" ]; do
  if [ -d "$p/doc/finder/SKILL" ]; then echo "$p/doc/finder/SKILL"; exit 0; fi
  p=$(dirname "$p")
done
exit 1"#,
        spectre_path.replace('\'', "'\"'\"'\"'\"'")
    );

    let output2 = Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=10"])
        .arg(target)
        .arg(&walk_script)
        .output()
        .map_err(|e| format!("SSH failed: {}", e))?;

    let stdout2 = String::from_utf8_lossy(&output2.stdout).trim().to_string();

    if stdout2.is_empty() {
        Ok(None)
    } else {
        Ok(Some(std::path::PathBuf::from(stdout2)))
    }
}
