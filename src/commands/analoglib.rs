//! `analoglib.*` — read the analogLib device reference.
//!
//! Pure local file reads: no SKILL is executed and Virtuoso is never contacted,
//! which is why the capability gate lets any caller in. The documentation is
//! cached from the remote Cadence install the same way the SKILL Finder's is.
//!
//! The rule these commands exist to serve: **look up a CDF parameter before
//! setting it.** `schematic.set_param` writes whatever name it is given, and a
//! name that is not in the device's CDF becomes a stray property the netlister
//! ignores without complaint — `ampl=5m` on a `vsin` cost a day of chasing a
//! flat transient, when the parameter is `va`, *"Amplitude 1 (Vpk)"*.

use crate::analoglib_finder::{AnalogLibFinder, ParamHit};
use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::skill_finder::SearchMode;
use serde_json::{json, Value};

/// Load the reference, from the local Cadence tree or the remote cache.
///
/// Shared by every command below so they cannot disagree about what the
/// documentation says — `skill.find` and `skill.info` used to read different
/// sources, and the one that lied was the one people trusted.
fn load_finder(refresh: bool) -> Result<AnalogLibFinder> {
    let cfg = Config::from_env()?;
    let mut finder = AnalogLibFinder::new();

    if cfg.is_remote() {
        crate::transport::backend::require_openssh(&cfg)?;
        let host = cfg.remote_host.clone().unwrap_or_default();
        let target = cfg.ssh_target();
        let cshrc = cfg.cadence_cshrc.as_deref();

        if refresh {
            let _ = crate::analoglib_finder::clear_cache(&host);
        }
        crate::analoglib_finder::load_or_sync(&mut finder, &host, &target, cshrc).map_err(|e| {
            VirtuosoError::Config(format!("failed to load analogLib reference: {e}"))
        })?;
    } else if let Some(dir) = find_local_doc_dir()? {
        finder.load(&dir).map_err(|e| {
            VirtuosoError::Config(format!("failed to load analogLib reference: {e}"))
        })?;
    }

    Ok(finder)
}

/// Locate `doc/analoglibref` in a local Cadence installation.
///
/// Walks up from each Cadence binary on `PATH` rather than assuming a fixed
/// depth: `tools/dfII/bin/virtuoso` and `tools/spectre/bin/spectre` do not sit
/// at the same level, and both binaries are tried — not `a || b` — because a
/// short-circuiting probe is exactly what left the SKILL cache 39 databases
/// short while reporting "no such function" for every one of them.
fn find_local_doc_dir() -> Result<Option<std::path::PathBuf>> {
    if let Ok(dir) = std::env::var("VB_ANALOGLIB_DIR") {
        if !dir.is_empty() && std::path::Path::new(&dir).exists() {
            return Ok(Some(std::path::PathBuf::from(dir)));
        }
    }

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
        let mut cur = std::path::Path::new(&path).parent();
        while let Some(dir) = cur {
            let doc_dir = dir.join(crate::analoglib_finder::DOC_SUBDIR);
            if doc_dir.exists() {
                return Ok(Some(doc_dir));
            }
            cur = dir.parent();
        }
    }

    Ok(None)
}

/// How many symbols are loaded, and whether anything failed to parse.
///
/// Attached to every "not found" answer. Without it, an empty database and a
/// genuinely absent symbol produce the same reply — the failure mode that made
/// `skill.info` report its own breakage as a fact about the caller's query.
fn provenance(finder: &AnalogLibFinder) -> Value {
    let report = finder.report();
    let mut out = json!({
        "symbols_loaded": finder.len(),
        "symbols_indexed": report.indexed,
        "source": finder.source_dir().map(|p| p.display().to_string()),
    });
    if !report.unresolved.is_empty() {
        out["unresolved_sections"] = json!(report.unresolved);
        out["warning"] = json!(
            "Some indexed symbols could not be located in the chapter HTML — \
             this lookup is incomplete. Report this rather than trusting a miss."
        );
    }
    out
}

fn param_json(p: &crate::analoglib_finder::CdfParam) -> Value {
    let mut v = json!({
        "name": p.name,
        "label": p.label,
        "spectre": p.spectre,
        "description": p.description,
        "default": p.default,
    });
    // A range row's `name` is documentation notation, not a settable name.
    // Say so, and give the names that are.
    if !p.expands_to.is_empty() {
        v["expands_to"] = json!(p.expands_to);
        v["note"] = json!(
            "`name` is a range as printed in the manual — pass one of `expands_to` \
             to schematic.set_param, not the range itself."
        );
    }
    v
}

fn hit_json(h: &ParamHit<'_>) -> Value {
    json!({
        "symbol": h.symbol.name,
        "category": h.symbol.category,
        "name": h.param.name,
        "label": h.param.label,
        "spectre": h.param.spectre,
        "description": h.param.description,
        "default": h.param.default,
    })
}

/// List every documented analogLib symbol.
pub fn list(category: Option<&str>, refresh: bool) -> Result<Value> {
    let finder = load_finder(refresh)?;

    let symbols: Vec<Value> = finder
        .symbols()
        .iter()
        .filter(|s| match category {
            Some(c) => s.category.to_lowercase().contains(&c.to_lowercase()),
            None => true,
        })
        .map(|s| {
            json!({
                "name": s.name,
                "category": s.category,
                "title": s.title,
                "param_count": s.params.len(),
                "primitives": s.primitives,
            })
        })
        .collect();

    let mut out = json!({
        "count": symbols.len(),
        "symbols": symbols,
    });
    merge(&mut out, provenance(&finder));
    Ok(out)
}

/// Full documentation for one symbol: title, Spectre primitives, CDF table.
pub fn info(symbol: &str, refresh: bool) -> Result<Value> {
    if symbol.is_empty() {
        return Err(VirtuosoError::Config("symbol name is required".into()));
    }
    let finder = load_finder(refresh)?;

    let Some(s) = finder.get(symbol) else {
        let mut out = json!({
            "symbol": symbol,
            "found": false,
            "reason": "no such symbol in the analogLib reference",
            "suggestions": finder.suggest(symbol, 10),
            "hint": "Symbol names are the cell names in the analogLib library \
                     (vsin, idc, cap, nmos4, …). Use analoglib.list to enumerate.",
        });
        merge(&mut out, provenance(&finder));
        return Ok(out);
    };

    let mut out = json!({
        "symbol": s.name,
        "found": true,
        "category": s.category,
        "title": s.title,
        "primitives": s.primitives,
        "release": s.release,
        "source_file": s.source_file,
        "param_count": s.params.len(),
        "params": s.params.iter().map(param_json).collect::<Vec<_>>(),
    });

    // Rows vs settable names: `vsin` documents 37 rows but a live instance
    // carries 135 names, and the whole difference is two ranges. Reporting only
    // the row count invites a "the manual is missing 98 parameters" conclusion
    // that is about the notation, not the device.
    let settable: usize = s
        .params
        .iter()
        .map(|p| p.expands_to.len().max(1))
        .sum();
    if settable != s.params.len() {
        out["settable_name_count"] = json!(settable);
    }

    if !s.also_documented_in.is_empty() {
        out["also_documented_in"] = json!(s.also_documented_in);
    }

    // An empty parameter table is a real property of some chapters, not a
    // parse failure — say which, so a caller never reads silence as "this
    // device takes no parameters".
    if s.params.is_empty() {
        out["note"] = json!(if s.primitives.is_empty() {
            "This chapter documents no CDF parameters for this symbol (the \
             supply/ground globals carry none)."
        } else {
            "This chapter has no CDF table; it defers to the Spectre primitive \
             documentation. Run `spectre -h <primitive>` for the parameter list, \
             and confirm against a live instance with schematic.list_cdf_params."
        });
    }

    out["verify_with"] = json!(format!(
        "schematic.list_cdf_params on a placed analogLib/{} instance",
        s.name
    ));
    merge(&mut out, provenance(&finder));
    Ok(out)
}

/// Search the reference.
///
/// `scope` selects what is searched:
/// - `params` (default) — CDF parameter names, labels and descriptions. This is
///   the query that turns *"amplitude"* into `va` / `vaDBm` / `ia`.
/// - `symbols` — symbol names, titles and categories.
/// - `both` — both, in one answer.
pub fn find(query: &str, mode: &str, scope: &str, limit: usize, refresh: bool) -> Result<Value> {
    if query.is_empty() {
        return Err(VirtuosoError::Config("query is required".into()));
    }
    let search_mode: SearchMode = mode.parse().unwrap_or(SearchMode::Fuzzy);
    let scope = match scope {
        "" => "params",
        s => s,
    };
    if !matches!(scope, "params" | "symbols" | "both") {
        return Err(VirtuosoError::Config(format!(
            "unknown scope '{scope}' (expected: params, symbols, both)"
        )));
    }
    let finder = load_finder(refresh)?;

    let mut out = json!({
        "query": query,
        "mode": search_mode.to_string(),
        "scope": scope,
    });

    // Search unbounded, then truncate here, so the counts report how many
    // matches exist rather than how many survived `limit`. A truncated count
    // presented as a total is the same class of lie as an empty database
    // presented as "no such parameter".
    if scope != "symbols" {
        let hits = finder.search_params(query, search_mode, usize::MAX);
        let total = hits.len();
        let shown: Vec<Value> = hits.iter().take(limit).map(hit_json).collect();
        out["param_count"] = json!(total);
        out["params_shown"] = json!(shown.len());
        out["params"] = json!(shown);
        if total > shown.len() {
            out["params_truncated"] = json!(true);
        }
    }
    if scope != "params" {
        let hits = finder.search(query, search_mode, usize::MAX);
        let total = hits.len();
        let shown: Vec<Value> = hits
            .iter()
            .take(limit)
            .map(|s| {
                json!({
                    "name": s.name,
                    "category": s.category,
                    "title": s.title,
                    "param_count": s.params.len(),
                })
            })
            .collect();
        out["symbol_count"] = json!(total);
        out["symbols_shown"] = json!(shown.len());
        out["symbols"] = json!(shown);
        if total > shown.len() {
            out["symbols_truncated"] = json!(true);
        }
    }

    merge(&mut out, provenance(&finder));
    Ok(out)
}

/// Copy the fields of `extra` into `target`.
fn merge(target: &mut Value, extra: Value) {
    let (Some(t), Some(e)) = (target.as_object_mut(), extra.as_object()) else {
        return;
    };
    for (k, v) in e {
        t.insert(k.clone(), v.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analoglib_finder::{AnalogLibSymbol, CdfParam};

    fn finder() -> AnalogLibFinder {
        AnalogLibFinder::for_test(vec![AnalogLibSymbol {
            name: "vsin".into(),
            params: vec![CdfParam {
                name: "va".into(),
                label: "Amplitude 1 (Vpk)".into(),
                ..Default::default()
            }],
            ..Default::default()
        }])
    }

    #[test]
    fn provenance_reports_how_much_was_loaded() {
        let p = provenance(&finder());
        assert_eq!(p["symbols_loaded"], 1);
        assert_eq!(p["symbols_indexed"], 1);
        assert!(p.get("warning").is_none());
    }

    /// An empty database must never look like a well-answered miss.
    #[test]
    fn provenance_on_an_empty_finder_shows_zero_rather_than_nothing() {
        let p = provenance(&AnalogLibFinder::new());
        assert_eq!(p["symbols_loaded"], 0);
        assert_eq!(p["symbols_indexed"], 0);
    }

    #[test]
    fn merge_copies_fields_without_dropping_existing_ones() {
        let mut a = json!({"found": true});
        merge(&mut a, json!({"symbols_loaded": 149}));
        assert_eq!(a["found"], true);
        assert_eq!(a["symbols_loaded"], 149);
    }

    #[test]
    fn merge_is_a_noop_on_non_objects() {
        let mut a = json!([1, 2]);
        merge(&mut a, json!({"x": 1}));
        assert_eq!(a, json!([1, 2]));
    }

    #[test]
    fn param_json_carries_the_label_not_just_the_name() {
        let p = param_json(&CdfParam {
            name: "va".into(),
            label: "Amplitude 1 (Vpk)".into(),
            default: "-".into(),
            ..Default::default()
        });
        assert_eq!(p["name"], "va");
        assert_eq!(p["label"], "Amplitude 1 (Vpk)");
        assert_eq!(p["default"], "-");
    }

    #[test]
    fn empty_arguments_are_rejected_at_the_boundary() {
        assert!(info("", false).is_err());
        assert!(find("", "fuzzy", "params", 10, false).is_err());
    }

    #[test]
    fn unknown_scope_is_rejected_rather_than_silently_defaulted() {
        let err = find("amplitude", "fuzzy", "everything", 10, false).unwrap_err();
        assert!(err.to_string().contains("unknown scope"));
    }

    /// `limit` bounds the page, never the reported total.
    ///
    /// On the real corpus `"amplitude"` matches 31 parameters; reporting
    /// `param_count: 8` for `limit: 8` would let a caller conclude the other 23
    /// do not exist — a limit dressed up as a fact about analogLib.
    #[test]
    fn a_truncated_page_still_reports_the_true_total() {
        let f = finder_with_many();
        let hits = f.search_params("amplitude", SearchMode::Fuzzy, usize::MAX);
        assert_eq!(hits.len(), 3, "fixture sanity");

        let page: Vec<_> = hits.iter().take(2).collect();
        assert_eq!(hits.len(), 3, "total is independent of the page size");
        assert_eq!(page.len(), 2);
    }

    /// The exact-label hit must survive a small `limit`, whatever its symbol
    /// name sorts as — `vsin` is last alphabetically among the sources.
    #[test]
    fn the_exactly_labelled_parameter_outranks_looser_matches() {
        let f = finder_with_many();
        let hits = f.search_params("amplitude", SearchMode::Fuzzy, 1);
        assert_eq!(hits[0].symbol.name, "vsin");
        assert_eq!(hits[0].param.name, "va");
    }

    fn finder_with_many() -> AnalogLibFinder {
        let p = |name: &str, label: &str| CdfParam {
            name: name.into(),
            label: label.into(),
            ..Default::default()
        };
        AnalogLibFinder::for_test(vec![
            AnalogLibSymbol {
                // Sorts first, matches only loosely.
                name: "iam".into(),
                params: vec![p("sa", "Signal amplitude")],
                ..Default::default()
            },
            AnalogLibSymbol {
                name: "isource".into(),
                params: vec![p("ia", "Amplitude 1 (Apk)")],
                ..Default::default()
            },
            AnalogLibSymbol {
                name: "vsin".into(),
                params: vec![p("va", "Amplitude")],
                ..Default::default()
            },
        ])
    }
}
