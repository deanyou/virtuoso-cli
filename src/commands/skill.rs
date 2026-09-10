use crate::client::bridge::VirtuosoClient;
use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::models::SessionInfo;
use crate::skill_finder::{SKILLFinder, SearchMode};
use serde_json::{json, Value};

pub fn exec(
    ctx: &crate::context::CommandContext,
    code: &str,
    timeout: u64,
    readonly: bool,
) -> Result<Value> {
    let mut client = VirtuosoClient::from_context(ctx)?;
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

pub fn load(ctx: &crate::context::CommandContext, file: &str, skillpp: bool) -> Result<Value> {
    let client = VirtuosoClient::from_context(ctx)?;

    let result = client.load_il(file, skillpp)?;
    let skillpp_mode = result.metadata.contains_key("skillpp_mode");

    Ok(json!({
        "status": "success",
        "file": file,
        "loaded_path": result.metadata.get("loaded_path"),
        "skillpp_mode": skillpp_mode,
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
pub fn eval(
    ctx: &crate::context::CommandContext,
    code: Option<String>,
    stdin: bool,
) -> Result<Value> {
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

    let client = VirtuosoClient::from_context(ctx)?;
    let result = client.execute_skill(&wrapped, None)?;

    Ok(json!({
        "status": if result.ok() { "success" } else { "error" },
        "output": result.output,
        "errors": result.errors,
        "warnings": result.warnings,
        "execution_time": result.execution_time,
    }))
}

/// Load the SKILL Finder databases for the configured host.
///
/// Shared by [`find`] and [`info`] so both answer from exactly the same
/// source — when they disagreed, `info` was the one that lied.
fn load_finder(cfg: &Config, refresh: bool) -> Result<SKILLFinder> {
    let mut finder = SKILLFinder::new();

    if cfg.is_remote() {
        // Remote mode: use cache or sync. Reject an explicit `native` backend
        // before any ssh/scp sync runs, so the mismatch is never swallowed.
        crate::transport::backend::require_openssh(cfg)?;
        let host = cfg.remote_host.clone().unwrap_or_default();
        let target = cfg.ssh_target();
        let cshrc = cfg.cadence_cshrc.as_deref();

        if refresh {
            // Force refresh: clear cache first, then sync
            let _ = crate::skill_finder::clear_cache(&host);
        }

        let _ = crate::skill_finder::load_or_sync(&mut finder, &host, &target, cshrc)?;
    } else {
        // Local mode: find from local Cadence installation
        if let Some(dir) = find_skill_finder_dir(cfg)? {
            finder
                .load(&dir)
                .map_err(|e| VirtuosoError::Config(format!("failed to load SKILL Finder: {}", e)))?;
        }
    }

    Ok(finder)
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
/// * `refresh` - Force a fresh cache sync (remote mode only)
/// * `include_desc` - When `true`, also match against the description field,
///   not just the function name. Useful for "what function does X" queries.
pub fn find(
    ctx: &crate::context::CommandContext,
    query: &str,
    mode: &str,
    limit: usize,
    refresh: bool,
    include_desc: bool,
) -> Result<Value> {
    let search_mode: SearchMode = mode.parse().unwrap_or(SearchMode::Fuzzy);
    let finder = load_finder(ctx.config(), refresh)?;

    let results: Vec<_> = finder
        .search(query, search_mode, limit, include_desc)
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
        "include_desc": include_desc,
        "count": results.len(),
        "entries": results,
    }))
}

/// Get the documented signature and description for a specific SKILL function.
///
/// Answers from the local SKILL Finder `.fnd` databases — no Virtuoso, no
/// Admin capability, works offline.
///
/// It used to call `mfGetMoreInfo` on the live bridge instead, which was wrong
/// in three compounding ways:
///
/// 1. `mfGetMoreInfo` **does not exist on IC23.1** (`getd` → nil), and the call
///    sat inside `when(boundp('mfGetMoreInfo) …)` — so it quietly evaluated to
///    nil for *every* input.
/// 2. That nil was reported as `{"found": false}` — i.e. *"no such function"*.
///    The tool was reporting its own breakage as a fact about the caller's
///    query. That is the one lie that sends you back to guessing, which is
///    precisely what looking things up exists to prevent.
/// 3. Reaching the bridge at all required raw-SKILL (Admin) access, so the
///    manual was unreadable to a plain design token.
///
/// If a future IC release ships `mfGetMoreInfo`, enrich from it — but keep the
/// `.fnd` answer as the floor, and never again report a missing documentation
/// *system* as a missing *function*.
pub fn info(ctx: &crate::context::CommandContext, func_name: &str) -> Result<Value> {
    if func_name.is_empty() {
        return Err(VirtuosoError::Config("function name is required".into()));
    }

    let finder = load_finder(ctx.config(), false)?;

    if let Some(e) = finder
        .search(func_name, SearchMode::Exact, 1, false)
        .into_iter()
        .next()
    {
        return Ok(json!({
            "func_name": func_name,
            "found": true,
            "name": e.name,
            "syntax": e.syntax,
            "description": e.description,
            "source": e.source_file,
        }));
    }

    // Not documented. Offer near misses so the caller has somewhere to go, and
    // say plainly what "not found" means here.
    let suggestions: Vec<_> = finder
        .search(func_name, SearchMode::Fuzzy, 10, false)
        .into_iter()
        .map(|e| e.name.clone())
        .collect();

    Ok(json!({
        "func_name": func_name,
        "found": false,
        "reason": "no entry in the SKILL Finder databases",
        "entries_loaded": finder.len(),
        "suggestions": suggestions,
        "hint": "On IC23.1 an undocumented function is almost always an undefined \
                 one. Confirm with getd('<fn>) before calling it.",
    }))
}

/// Find the SKILL Finder directory from config.
///
/// Priority:
/// 1. `VB_SKILL_FINDER_DIR` env var
/// 2. Discover from the local Cadence installation (`virtuoso`, then `spectre`)
/// 3. Discover on the remote host over SSH (via `VB_CADENCE_CSHRC`)
fn find_skill_finder_dir(cfg: &Config) -> Result<Option<std::path::PathBuf>> {
    // 1. Check VB_SKILL_FINDER_DIR
    if let Ok(dir) = std::env::var("VB_SKILL_FINDER_DIR") {
        if !dir.is_empty() && std::path::Path::new(&dir).exists() {
            tracing::debug!("Using VB_SKILL_FINDER_DIR: {}", dir);
            return Ok(Some(std::path::PathBuf::from(dir)));
        }
    }

    // 2. For local, walk up from the Cadence binaries on PATH. `virtuoso`
    //    first: a Spectre-only install has a `doc/finder/SKILL` too, but it
    //    holds 2 databases instead of the DFII tree's 39 — stopping there is
    //    what left every db/dd/ge/sch/hi lookup unanswerable.
    if !cfg.is_remote() {
        for bin in ["virtuoso", "spectre"] {
            let Ok(out) = std::process::Command::new("command")
                .args(["-v", bin])
                .output()
                .or_else(|_| std::process::Command::new("which").arg(bin).output())
            else {
                continue;
            };
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if path.is_empty() || path == bin {
                continue;
            }
            // Walk up until a doc/finder/SKILL turns up, rather than assuming
            // a fixed depth — `tools/dfII/bin/virtuoso` and
            // `tools/spectre/bin/spectre` do not sit at the same level.
            let mut cur = std::path::Path::new(&path).parent();
            while let Some(dir) = cur {
                let finder_dir = dir.join("doc/finder/SKILL");
                if finder_dir.exists() {
                    tracing::debug!("Found SKILL Finder at: {}", finder_dir.display());
                    return Ok(Some(finder_dir));
                }
                cur = dir.parent();
            }
        }
    }

    // 3. For remote, try to discover using SSH if available
    if cfg.is_remote() {
        // Try to find via SSH using the cadence cshrc
        if let Some(ref cshrc) = cfg.cadence_cshrc {
            if let Ok(Some(path)) = discover_skill_finder_remote(&cfg.ssh_target(), cshrc) {
                tracing::debug!("Discovered SKILL Finder on remote: {}", path.display());
                return Ok(Some(path));
            }
        }
    }

    Ok(None)
}

/// Discover SKILL Finder directory on a remote server via SSH.
///
/// Probes `virtuoso` **and** `spectre` and returns the most relevant hit (the
/// DFII tree wins). Probing only `spectre` lands on a Spectre-only install
/// that ships 2 `.fnd` files instead of the 39 in the Virtuoso tree.
fn discover_skill_finder_remote(
    target: &str,
    cadence_cshrc: &str,
) -> std::result::Result<Option<std::path::PathBuf>, String> {
    use std::process::Command;

    let script = crate::skill_finder::remote_finder_probe_script(Some(cadence_cshrc));

    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=10"])
        .arg(target)
        .arg(&script)
        .output()
        .map_err(|e| format!("SSH failed: {}", e))?;

    let dirs = crate::skill_finder::parse_finder_dirs(&String::from_utf8_lossy(&output.stdout));

    Ok(dirs.into_iter().next().map(std::path::PathBuf::from))
}

/// Sync SKILL Finder cache from remote server.
pub fn sync_cache(
    ctx: &crate::context::CommandContext,
    host: Option<&str>,
    cshrc: Option<&str>,
    verbose: bool,
) -> Result<Value> {
    let cfg = ctx.config();
    crate::transport::backend::require_openssh(cfg)?;
    let target_host = host
        .map(String::from)
        .or(cfg.remote_host.clone())
        .ok_or_else(|| VirtuosoError::Config("Remote host required for sync".into()))?;

    let target = cfg.ssh_target();
    let target_cshrc = cshrc.map(String::from).or(cfg.cadence_cshrc.clone());
    let target_cshrc_ref = target_cshrc.as_deref();

    let old_count = crate::skill_finder::cache_file_count(&target_host);

    // Sync with or without progress
    let new_count = if verbose {
        fn print_progress(msg: &str) {
            eprintln!("{}", msg);
        }
        crate::skill_finder::sync_from_remote(
            &target_host,
            &target,
            target_cshrc_ref,
            Some(print_progress),
        )
        .map_err(|e| VirtuosoError::Config(e.to_string()))?
    } else {
        fn noop(_: &str) {}
        crate::skill_finder::sync_from_remote(&target_host, &target, target_cshrc_ref, Some(noop))
            .map_err(|e| VirtuosoError::Config(e.to_string()))?
    };

    let cache_path = crate::skill_finder::cache_dir(&target_host)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    Ok(json!({
        "host": target_host,
        "cache_path": cache_path,
        "old_file_count": old_count,
        "new_file_count": new_count,
        "synced": new_count > 0,
    }))
}

/// Show or clear SKILL Finder cache.
pub fn show_cache(
    ctx: &crate::context::CommandContext,
    host: Option<&str>,
    clear: bool,
) -> Result<Value> {
    let cfg = ctx.config();
    let target_host = host
        .map(String::from)
        .or(cfg.remote_host.clone())
        .unwrap_or_else(|| "default".to_string());

    if clear {
        crate::skill_finder::clear_cache(&target_host)
            .map_err(|e| VirtuosoError::Config(e.to_string()))?;
        return Ok(json!({
            "host": target_host,
            "cleared": true,
            "message": format!("Cache cleared for {}", target_host),
        }));
    }

    // Show cache info
    if let Some(info) = crate::skill_finder::cache_info(&target_host) {
        Ok(json!({
            "host": target_host,
            "exists": true,
            "path": info.path.to_string_lossy(),
            "file_count": info.file_count,
            "modified": info.modified.map(|t| {
                chrono::DateTime::<chrono::Utc>::from(t)
                    .format("%Y-%m-%d %H:%M:%S UTC")
                    .to_string()
            }),
        }))
    } else {
        Ok(json!({
            "host": target_host,
            "exists": false,
            "message": format!("No cache found for {}. Use 'vcli skill sync' to download.", target_host),
        }))
    }
}
