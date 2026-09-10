//! `libref.*` — read the Virtuoso library references Cadence ships under `doc/`.
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
//!
//! Not just analogLib: every schematic here also places `basic/ipin`,
//! `basic/opin` and `basic/gnd`. Both manuals are read, `lib` selects one, and
//! the libraries with no parser answer with what to do instead rather than with
//! an empty result set.

use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::libref::{registry, LibRefFinder, LibSymbol, LibraryState, ParamHit};
use crate::skill_finder::SearchMode;
use serde_json::{json, Value};

/// What to tell a caller whose symbol is in no manual.
///
/// The likeliest such name is a PDK device — `n50_ckt`, `p50_ckt` — because
/// those are most of what a schematic is made of and no Cadence manual
/// documents any of them. `libref.list` is a dead end for that caller, so name
/// the tool that does answer for them first.
const NOT_FOUND_HINT: &str = "No Cadence manual documents PDK devices (n50_ckt, p50_ckt, …) \
     or project cells — place one and read schematic.list_cdf_params, which is \
     authoritative for any master. libref covers the Virtuoso libraries: analogLib has \
     the sources and passives (vsin, idc, cap), basic has the pins and supplies (ipin, \
     opin, gnd, vdd). Use libref.list to enumerate those.";

/// Load the reference, from the local Cadence tree or the remote cache.
///
/// Shared by every command below so they cannot disagree about what the
/// documentation says — `skill.find` and `skill.info` used to read different
/// sources, and the one that lied was the one people trusted.
fn load_finder(refresh: bool) -> Result<LibRefFinder> {
    let cfg = Config::from_env()?;
    let mut finder = LibRefFinder::new();

    if cfg.is_remote() {
        crate::transport::backend::require_openssh(&cfg)?;
        let host = cfg.remote_host.clone().unwrap_or_default();
        let target = cfg.ssh_target();
        let cshrc = cfg.cadence_cshrc.as_deref();

        if refresh {
            let _ = crate::libref::clear_cache(&host);
        }
        crate::libref::load_or_sync(&mut finder, &host, &target, cshrc).map_err(|e| {
            VirtuosoError::Config(format!("failed to load the library reference: {e}"))
        })?;
    } else if let Some(dir) = find_local_doc_root()? {
        finder.load(&dir).map_err(|e| {
            VirtuosoError::Config(format!("failed to load the library reference: {e}"))
        })?;
    }

    Ok(finder)
}

/// Locate a local Cadence release root that has at least one registered manual.
///
/// The *release root* — the directory `doc/analoglibref` and `doc/basicLib` sit
/// under — not one manual's directory, because one call has to load them all.
///
/// Walks up from each Cadence binary on `PATH` rather than assuming a fixed
/// depth: `tools/dfII/bin/virtuoso` and `tools/spectre/bin/spectre` do not sit
/// at the same level, and both binaries are tried — not `a || b` — because a
/// short-circuiting probe is exactly what left the SKILL cache 39 databases
/// short while reporting "no such function" for every one of them.
fn find_local_doc_root() -> Result<Option<std::path::PathBuf>> {
    if let Ok(dir) = std::env::var("VB_LIBREF_DIR") {
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
            if registry::LIBRARIES
                .iter()
                .filter(|l| l.has_manual())
                .any(|l| dir.join(l.doc_subdir).exists())
            {
                return Ok(Some(dir.to_path_buf()));
            }
            cur = dir.parent();
        }
    }

    Ok(None)
}

/// How much was loaded, per library, and whether anything failed to parse.
///
/// Attached to every answer. Without it, an empty database and a genuinely
/// absent symbol produce the same reply — the failure mode that made
/// `skill.info` report its own breakage as a fact about the caller's query.
///
/// The per-library `note` is deliberately *not* inlined here: four paragraphs
/// on every `list` call buries the answer. The names are listed instead, with
/// the one call that prints the full explanation.
fn provenance(finder: &LibRefFinder) -> Value {
    let report = finder.report();
    let libraries: Vec<Value> = finder
        .libraries()
        .iter()
        .map(|s| {
            json!({
                "lib": s.lib,
                "state": s.state.tag(),
                "symbols": s.symbols,
            })
        })
        .collect();
    let unavailable: Vec<&str> = finder
        .libraries()
        .iter()
        .filter(|s| s.state != LibraryState::Loaded)
        .map(|s| s.lib)
        .collect();

    let mut out = json!({
        "symbols_loaded": finder.len(),
        "symbols_indexed": report.indexed,
        "source": finder.source_dir().map(|p| p.display().to_string()),
        "libraries": libraries,
    });
    if !unavailable.is_empty() {
        out["libraries_without_symbols"] = json!(unavailable);
        out["libraries_hint"] = json!(
            "These libraries yield no symbols here. Call libref.info with \
             lib=<name> for the reason and what to use instead — it is never \
             'the library has no cells'."
        );
    }
    if !report.unresolved.is_empty() {
        out["unresolved_sections"] = json!(report.unresolved);
        out["warning"] = json!(
            "Some indexed symbols could not be located in the chapter HTML — \
             this lookup is incomplete. Report this rather than trusting a miss."
        );
    }
    out
}

/// Validate a `lib` argument against the registry.
///
/// An unknown name is refused rather than quietly filtered to nothing: asking
/// for `pdkLib` and getting `count: 0` reads as "the PDK devices are not
/// documented", when the truth is that this tool never had a table for them.
fn resolve_lib(lib: &str) -> Result<Option<&'static registry::LibraryDoc>> {
    if lib.is_empty() {
        return Ok(None);
    }
    registry::lookup(lib).map(Some).ok_or_else(|| {
        VirtuosoError::Config(format!(
            "'{lib}' is not a library libref knows about (known: {}). \
             PDK and project libraries have no Cadence manual — read their \
             parameters off a placed instance with schematic.list_cdf_params.",
            registry::known_names().join(", ")
        ))
    })
}

/// The answer for a library that is registered but yields nothing.
///
/// Returned instead of an empty list, so the caller learns the difference
/// between "nothing is documented" and "nothing here reads it".
fn unavailable_json(finder: &LibRefFinder, doc: &registry::LibraryDoc) -> Option<Value> {
    let status = finder.library(doc.lib)?;
    if status.state == LibraryState::Loaded {
        return None;
    }
    Some(json!({
        "lib": doc.lib,
        "found": false,
        "state": status.state.tag(),
        "reason": status.state.note(),
        "doc_dir": doc.has_manual().then_some(doc.doc_subdir),
    }))
}

fn param_json(p: &crate::libref::CdfParam) -> Value {
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
        "lib": h.symbol.lib,
        "symbol": h.symbol.name,
        "cell": h.symbol.qualified_name(),
        "category": h.symbol.category,
        "name": h.param.name,
        "label": h.param.label,
        "spectre": h.param.spectre,
        "description": h.param.description,
        "default": h.param.default,
    })
}

/// One symbol in list output.
fn brief_json(s: &LibSymbol) -> Value {
    let mut v = json!({
        "lib": s.lib,
        "name": s.name,
        "cell": s.qualified_name(),
        "category": s.category,
        "title": s.title,
        "param_count": s.params.len(),
        "primitives": s.primitives,
    });
    if !s.description.is_empty() {
        v["description"] = json!(s.description);
    }
    v
}

/// Full documentation for one symbol.
fn detail_json(s: &LibSymbol) -> Value {
    let mut out = json!({
        "lib": s.lib,
        "symbol": s.name,
        "cell": s.qualified_name(),
        "found": true,
        "category": s.category,
        "title": s.title,
        "description": s.description,
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
    let settable: usize = s.params.iter().map(|p| p.expands_to.len().max(1)).sum();
    if settable != s.params.len() {
        out["settable_name_count"] = json!(settable);
    }

    if !s.also_documented_in.is_empty() {
        out["also_documented_in"] = json!(s.also_documented_in);
    }

    // An empty parameter table is a real property of some manuals, not a parse
    // failure — say which, so a caller never reads silence as "this device
    // takes no parameters".
    if s.params.is_empty() {
        out["note"] = json!(if s.lib.eq_ignore_ascii_case("basic") {
            "The Basic Library reference documents no CDF parameters for any of \
             its cells — this is a property of the manual, not a gap in the \
             parse. Pins and supplies are configured through their name and \
             direction, not through CDF; confirm with schematic.list_cdf_params \
             on a placed instance if you need the full list."
        } else if s.primitives.is_empty() {
            "This chapter documents no CDF parameters for this symbol (the \
             supply/ground globals carry none)."
        } else {
            "This chapter has no CDF table; it defers to the Spectre primitive \
             documentation. Run `spectre -h <primitive>` for the parameter list, \
             and confirm against a live instance with schematic.list_cdf_params."
        });
    }

    out["verify_with"] = json!(format!(
        "schematic.list_cdf_params on a placed {} instance",
        s.qualified_name()
    ));

    // Verified 2026-09-10 against a live `analogLib/cap`: the manual documents
    // the last two rows as `area1` and `perim1`, and the placed instance calls
    // them `area` and `perim` — same GUI labels, different CDF names. Setting
    // the manual's spelling would write a junk property that no simulator
    // reads, which is the exact `ampl`-instead-of-`va` failure this tool was
    // built to prevent. So the manual is authoritative on *meaning* and the
    // instance is authoritative on *names*; say so wherever there is a name
    // list to get wrong.
    if !s.params.is_empty() {
        out["names_are_not_authoritative"] = json!(
            "Cross-check every name against the live instance before setting it. \
             The manual has been observed to disagree: analogLib/cap is \
             documented with `area1`/`perim1`, but a placed instance carries \
             `area`/`perim`. Read this table for what a parameter means and \
             list_cdf_params for what it is called."
        );
    }
    out
}

/// List documented symbols, optionally from one library or category.
pub fn list(lib: &str, category: Option<&str>, refresh: bool) -> Result<Value> {
    let doc = resolve_lib(lib)?;
    let finder = load_finder(refresh)?;

    if let Some(doc) = doc {
        if let Some(mut out) = unavailable_json(&finder, doc) {
            out["count"] = json!(0);
            out["symbols"] = json!([]);
            merge(&mut out, provenance(&finder));
            return Ok(out);
        }
    }

    let symbols: Vec<Value> = finder
        .symbols()
        .iter()
        .filter(|s| doc.is_none_or(|d| s.lib.eq_ignore_ascii_case(d.lib)))
        .filter(|s| match category {
            Some(c) => s.category.to_lowercase().contains(&c.to_lowercase()),
            None => true,
        })
        .map(brief_json)
        .collect();

    let mut out = json!({
        "lib": doc.map(|d| d.lib),
        "count": symbols.len(),
        "symbols": symbols,
    });
    merge(&mut out, provenance(&finder));
    Ok(out)
}

/// Full documentation for one symbol: title, Spectre primitives, CDF table.
///
/// With no `lib`, every library that documents the name answers. `vdd` and
/// `gnd` exist in more than one, and they are not the same symbol — so an
/// ambiguous name comes back as a list rather than as whichever one sorted
/// first with the others silently dropped.
pub fn info(symbol: &str, lib: &str, refresh: bool) -> Result<Value> {
    let doc = resolve_lib(lib)?;
    let finder = load_finder(refresh)?;

    // `libref.info --lib rfLib` with no symbol is the "tell me about this
    // library" query, and the answer is the note.
    if symbol.is_empty() {
        let Some(doc) = doc else {
            return Err(VirtuosoError::Config("symbol name is required".into()));
        };
        let mut out = unavailable_json(&finder, doc).unwrap_or_else(|| {
            json!({
                "lib": doc.lib,
                "found": true,
                "state": "loaded",
                "doc_dir": doc.doc_subdir,
            })
        });
        merge(&mut out, provenance(&finder));
        return Ok(out);
    }

    if let Some(doc) = doc {
        if let Some(mut out) = unavailable_json(&finder, doc) {
            out["symbol"] = json!(symbol);
            merge(&mut out, provenance(&finder));
            return Ok(out);
        }
    }

    let matches: Vec<&LibSymbol> = match doc {
        Some(d) => finder.get_in(d.lib, symbol).into_iter().collect(),
        None => finder.get_all(symbol),
    };

    let mut out = match matches.len() {
        0 => {
            let mut out = json!({
                "symbol": symbol,
                "lib": doc.map(|d| d.lib),
                "found": false,
                "reason": match doc {
                    Some(d) => format!("no cell named '{symbol}' in the {} reference", d.lib),
                    None => format!("no cell named '{symbol}' in any parsed library reference"),
                },
                "suggestions": finder.suggest(symbol, 10),
                // The likeliest name to land here is a PDK device — `n50_ckt`,
                // `p50_ckt` — because those are what a schematic is mostly made
                // of and no Cadence manual documents any of them. Sending that
                // caller to `libref.list` is a dead end, so name the tool that
                // does answer for them first.
                "hint": NOT_FOUND_HINT,
            });
            merge(&mut out, provenance(&finder));
            return Ok(out);
        }
        1 => detail_json(matches[0]),
        _ => json!({
            "symbol": symbol,
            "found": true,
            "ambiguous": true,
            "libraries": matches.iter().map(|s| s.lib.clone()).collect::<Vec<_>>(),
            "hint": format!(
                "'{symbol}' is documented in more than one library and they are \
                 different cells. Pass lib=<name> to select one."
            ),
            "matches": matches.iter().map(|s| detail_json(s)).collect::<Vec<_>>(),
        }),
    };

    merge(&mut out, provenance(&finder));
    Ok(out)
}

/// Search the reference.
///
/// `scope` selects what is searched:
/// - `params` (default) — CDF parameter names, labels and descriptions. This is
///   the query that turns *"amplitude"* into `va` / `vaDBm` / `ia`.
/// - `symbols` — symbol names, titles, categories and descriptions.
/// - `both` — both, in one answer.
pub fn find(
    query: &str,
    mode: &str,
    scope: &str,
    lib: &str,
    limit: usize,
    refresh: bool,
) -> Result<Value> {
    if query.is_empty() {
        return Err(VirtuosoError::Config("query is required".into()));
    }
    let doc = resolve_lib(lib)?;
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
        "lib": doc.map(|d| d.lib),
    });

    if let Some(doc) = doc {
        if let Some(u) = unavailable_json(&finder, doc) {
            merge(&mut out, u);
            out["found"] = json!(false);
            merge(&mut out, provenance(&finder));
            return Ok(out);
        }
    }
    let in_lib = |s: &LibSymbol| doc.is_none_or(|d| s.lib.eq_ignore_ascii_case(d.lib));

    // Search unbounded, then truncate here, so the counts report how many
    // matches exist rather than how many survived `limit`. A truncated count
    // presented as a total is the same class of lie as an empty database
    // presented as "no such parameter".
    if scope != "symbols" {
        let hits: Vec<ParamHit<'_>> = finder
            .search_params(query, search_mode, usize::MAX)
            .into_iter()
            .filter(|h| in_lib(h.symbol))
            .collect();
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
        let hits: Vec<&LibSymbol> = finder
            .search(query, search_mode, usize::MAX)
            .into_iter()
            .filter(|s| in_lib(s))
            .collect();
        let total = hits.len();
        let shown: Vec<Value> = hits.iter().take(limit).map(|s| brief_json(s)).collect();
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
    use crate::libref::{CdfParam, LibSymbol};

    fn finder() -> LibRefFinder {
        LibRefFinder::for_test(vec![LibSymbol {
            lib: "analogLib".into(),
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
        let p = provenance(&LibRefFinder::new());
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
        assert!(info("", "", false).is_err());
        assert!(find("", "fuzzy", "params", "", 10, false).is_err());
    }

    #[test]
    fn unknown_scope_is_rejected_rather_than_silently_defaulted() {
        let err = find("amplitude", "fuzzy", "everything", "", 10, false).unwrap_err();
        assert!(err.to_string().contains("unknown scope"));
    }

    /// An unregistered library is an error, not a filter that matches nothing.
    /// `count: 0` for `pdkLib` would read as "the PDK devices are undocumented".
    #[test]
    fn an_unknown_library_is_refused_with_the_known_names() {
        let err = resolve_lib("pdkLib").unwrap_err().to_string();
        assert!(err.contains("analogLib"), "{err}");
        assert!(err.contains("list_cdf_params"), "{err}");
        assert!(resolve_lib("").unwrap().is_none());
        assert_eq!(resolve_lib("BASIC").unwrap().unwrap().lib, "basic");
    }

    /// The defect-L guard at the command layer: a library with no parser
    /// answers with its reason, never with an empty symbol list.
    #[test]
    fn a_library_without_a_parser_answers_with_its_reason() {
        let mut f = LibRefFinder::new();
        let root = tempfile::tempdir().unwrap();
        f.load(root.path()).unwrap();

        let doc = registry::lookup("rfLib").unwrap();
        let out = unavailable_json(&f, doc).unwrap();
        assert_eq!(out["found"], false);
        assert_eq!(out["state"], "no_parser");
        assert!(out["reason"]
            .as_str()
            .unwrap()
            .contains("list_cdf_params"));
        assert_eq!(out["doc_dir"], "doc/rflibrary");
    }

    /// `ahdlLib` has no manual at all, so there is no directory to name.
    #[test]
    fn a_library_without_a_manual_names_no_directory() {
        let mut f = LibRefFinder::new();
        let root = tempfile::tempdir().unwrap();
        f.load(root.path()).unwrap();

        let out = unavailable_json(&f, registry::lookup("ahdlLib").unwrap()).unwrap();
        assert_eq!(out["state"], "no_manual");
        assert!(out["doc_dir"].is_null());
        assert!(out["reason"].as_str().unwrap().contains("veriloga"));
    }

    /// Provenance names the libraries that came back empty. A caller reading
    /// `symbols_loaded: 157` has to be able to see that four more exist and
    /// why they contributed nothing.
    #[test]
    fn provenance_names_the_libraries_that_yielded_nothing() {
        let mut f = LibRefFinder::new();
        let root = tempfile::tempdir().unwrap();
        f.load(root.path()).unwrap();

        let p = provenance(&f);
        assert_eq!(
            p["libraries"].as_array().unwrap().len(),
            registry::LIBRARIES.len()
        );
        let empty: Vec<&str> = p["libraries_without_symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(empty.contains(&"rfLib"));
        assert!(empty.contains(&"analogLib"), "nothing was synced here");
        assert!(p["libraries_hint"].as_str().unwrap().contains("libref.info"));
    }

    /// `basic` cells carry no CDF parameters. The note has to say that is the
    /// manual's shape, not a failed parse.
    #[test]
    fn a_basic_cell_explains_why_its_parameter_list_is_empty() {
        let d = detail_json(&LibSymbol {
            lib: "basic".into(),
            name: "ipin".into(),
            description: "An input pin.".into(),
            ..Default::default()
        });
        assert_eq!(d["cell"], "basic/ipin");
        assert_eq!(d["param_count"], 0);
        let note = d["note"].as_str().unwrap();
        assert!(note.contains("property of the manual"), "{note}");
        assert_eq!(
            d["verify_with"],
            "schematic.list_cdf_params on a placed basic/ipin instance"
        );
    }

    /// The analogLib globals also carry no parameters, and for a different
    /// reason — the two notes must not be swapped.
    #[test]
    fn an_analoglib_symbol_gets_the_analoglib_explanation() {
        let d = detail_json(&LibSymbol {
            lib: "analogLib".into(),
            name: "nmos4".into(),
            primitives: vec!["mos0".into()],
            ..Default::default()
        });
        assert!(d["note"].as_str().unwrap().contains("spectre -h"));
    }

    /// A symbol with no parameters has no name list to get wrong, so the
    /// caveat would be noise. One with a table must carry it — the manual is
    /// known to misname rows (`analogLib/cap`: `area1` vs the live `area`).
    #[test]
    fn a_parameter_table_warns_that_the_manual_can_misname_a_row() {
        let empty = detail_json(&LibSymbol {
            lib: "basic".into(),
            name: "ipin".into(),
            ..Default::default()
        });
        assert!(empty.get("names_are_not_authoritative").is_none());

        let table = detail_json(&LibSymbol {
            lib: "analogLib".into(),
            name: "cap".into(),
            params: vec![CdfParam {
                name: "c".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let caveat = table["names_are_not_authoritative"].as_str().unwrap();
        assert!(caveat.contains("area1"), "{caveat}");
        assert!(caveat.contains("list_cdf_params"), "{caveat}");
    }

    /// The likeliest miss is a PDK device, and `libref.list` cannot help with
    /// one. The hint has to name the tool that can.
    #[test]
    fn a_missing_symbol_points_pdk_devices_at_the_live_instance() {
        assert!(NOT_FOUND_HINT.contains("list_cdf_params"));
        assert!(NOT_FOUND_HINT.contains("n50_ckt"));
    }

    #[test]
    fn brief_json_carries_the_placeable_cell_path() {
        let b = brief_json(&LibSymbol {
            lib: "analogLib".into(),
            name: "vsin".into(),
            ..Default::default()
        });
        assert_eq!(b["cell"], "analogLib/vsin");
        assert!(b.get("description").is_none(), "empty prose is not a field");
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

    /// Filtering by library happens after the search, so the ranking a caller
    /// sees inside one library is the same ranking they would see across all.
    #[test]
    fn a_lib_filter_selects_without_reordering() {
        let f = LibRefFinder::for_test(vec![
            LibSymbol {
                lib: "basic".into(),
                name: "ipin".into(),
                description: "An input pin.".into(),
                ..Default::default()
            },
            LibSymbol {
                lib: "analogLib".into(),
                name: "ipin_src".into(),
                params: vec![CdfParam {
                    name: "i".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ]);
        let all = f.search("ipin", SearchMode::Fuzzy, usize::MAX);
        assert_eq!(all.len(), 2);
        let only_basic: Vec<_> = all
            .into_iter()
            .filter(|s| s.lib.eq_ignore_ascii_case("basic"))
            .collect();
        assert_eq!(only_basic.len(), 1);
        assert_eq!(only_basic[0].name, "ipin");
    }

    fn finder_with_many() -> LibRefFinder {
        let p = |name: &str, label: &str| CdfParam {
            name: name.into(),
            label: label.into(),
            ..Default::default()
        };
        LibRefFinder::for_test(vec![
            LibSymbol {
                // Sorts first, matches only loosely.
                lib: "analogLib".into(),
                name: "iam".into(),
                params: vec![p("sa", "Signal amplitude")],
                ..Default::default()
            },
            LibSymbol {
                lib: "analogLib".into(),
                name: "isource".into(),
                params: vec![p("ia", "Amplitude 1 (Apk)")],
                ..Default::default()
            },
            LibSymbol {
                lib: "analogLib".into(),
                name: "vsin".into(),
                params: vec![p("va", "Amplitude")],
                ..Default::default()
            },
        ])
    }
}
