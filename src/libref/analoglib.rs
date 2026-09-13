//! Parser for Cadence's `doc/analoglibref/` — the analogLib device reference.
//!
//! # Why this exists
//!
//! `schematic.list_cdf_params` tells you a symbol has a parameter called `va`.
//! It does not tell you that `va` means *"Amplitude 1 (Vpk)"*, which is the
//! only thing that distinguishes it from `vaDBm`, `ia`, `acm` and `pacmag`.
//! Guessing produced `ampl=5m` on a `vsin` once: `ampl` is not a CDF parameter
//! of anything in analogLib, the netlister silently ignored the stray property,
//! and the transient came out a flat line. This module is the lookup that turns
//! that guess into a manual reference.
//!
//! # Structure of the source data
//!
//! | File | Content |
//! |---|---|
//! | `analoglibref.json` | jstree index: one `"Symbol: <name>"` node per symbol, `href = "<file>#<anchor>"`, parent node names the category |
//! | `appA.html` | "List of All CDF Parameters": 4-column rows *label \| CDF name \| description \| default* |
//! | `passives.html`, `independent.html`, `actives.html`, … | one section per symbol: title, `spectre -h <primitive>`, and the per-symbol CDF table |
//!
//! # Why the index drives the parse
//!
//! The obvious approach — scan each chapter for `Symbol: <name>` headings —
//! silently loses six symbols, because their headings have anchor tags embedded
//! *mid-word*:
//!
//! ```text
//! Sym<a id="bsource"></a>bol: bsource
//! Symbol: s<a name="sprobe"></a>probe
//! Symbol: diffs<a name="diffsprobe"></a>probe
//! ```
//!
//! `capq`, `indq`, `bsource`, `vrefgnd`, `sprobe` and `diffsprobe` all vanish,
//! and nothing reports an error — the same failure mode as the incomplete
//! prefix table that made the SKILL function census miss 7 of 107 names.
//! So the symbol list comes from the index (authoritative, 156 entries) and
//! each section is located by its `pgfId-…` anchor, which is always present.
//! A section that cannot be located is dropped from the count, never silently
//! merged into its neighbour.

use super::html::{cells, find_anchor, parse_jstree, plain, read_lossy, RE_ROW, RE_TABLE};
use super::types::{CdfParam, LibSymbol, ParseReport};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Basename of the jstree index, with or without a `<RELEASE>__` cache prefix.
pub const INDEX_BASENAME: &str = "analoglibref.json";
/// Basename of the "List of All CDF Parameters" appendix.
pub const APPENDIX_BASENAME: &str = "appA.html";

/// Virtuoso library name these symbols belong to.
const LIB: &str = "analogLib";

static RE_STRONG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<strong>(.*?)</strong>").unwrap());
static RE_PRIMITIVE: Lazy<Regex> = Lazy::new(|| Regex::new(r"spectre -h (\w+)").unwrap());
static RE_RANGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^([A-Za-z_]+)(\d+)\s*-\s*([A-Za-z_]+)(\d+)$").unwrap());

/// An entry of the jstree index.
#[derive(Debug, Clone)]
struct IndexEntry {
    name: String,
    file: String,
    anchor: String,
    category: String,
}

/// Parse `analoglibref.json` (a jstree dump) into index entries.
///
/// Symbol nodes are the ones labelled `"Symbol: <name>"`; the parent node's
/// text is the category. Depth is not usable as the discriminator the way it is
/// in `basicLib` — this index nests chapters unevenly — but the `Symbol: `
/// prefix is unambiguous here, and it is what keeps navigation nodes like
/// *"Passive Components"* out of the symbol list.
fn parse_index(json: &str) -> Vec<IndexEntry> {
    parse_jstree(json)
        .into_iter()
        .filter_map(|n| {
            let name = n.text.strip_prefix("Symbol: ")?.trim().to_string();
            if name.is_empty() || n.file.is_empty() || n.anchor.is_empty() {
                return None;
            }
            Some(IndexEntry {
                name,
                file: n.file,
                anchor: n.anchor,
                category: n.parent,
            })
        })
        .collect()
}

/// A row of `appA.html`.
#[derive(Debug, Clone)]
struct AppendixRecord {
    label: String,
    description: String,
    default: String,
}

/// Parse `appA.html` into `CDF name -> records`.
///
/// A CDF name is not unique across symbols: `area` is *"Capacitor Area"* on
/// `cap`, *"Device area"* on a transistor and *"Alias of mult"* elsewhere.
/// Keep every record and disambiguate by label at merge time.
fn parse_appendix(html: &str) -> HashMap<String, Vec<AppendixRecord>> {
    let mut out: HashMap<String, Vec<AppendixRecord>> = HashMap::new();
    for row in RE_ROW.captures_iter(html) {
        let c = cells(&row[1]);
        if c.len() != 4 || c[1].is_empty() || c[1] == "CDF Parameter Name" {
            continue;
        }
        out.entry(c[1].clone()).or_default().push(AppendixRecord {
            label: c[0].clone(),
            description: c[2].clone(),
            default: c[3].clone(),
        });
    }
    out
}

/// Expand a compact range name (`F1 - F50`) into the names it stands for.
///
/// Returns empty for anything that is not a range, so an ordinary name is
/// unaffected. Bounded at [`MAX_RANGE_EXPANSION`]: a malformed table must not
/// be able to turn one row into a million entries.
fn expand_range(name: &str) -> Vec<String> {
    let Some(c) = RE_RANGE.captures(name) else {
        return Vec::new();
    };
    if c[1] != c[3] {
        return Vec::new(); // `F1 - N50` is not a range, it is a typo
    }
    let (Ok(from), Ok(to)) = (c[2].parse::<u32>(), c[4].parse::<u32>()) else {
        return Vec::new();
    };
    if from >= to || (to - from) as usize >= MAX_RANGE_EXPANSION {
        return Vec::new();
    }
    (from..=to).map(|i| format!("{}{}", &c[1], i)).collect()
}

/// Ceiling on how many names one table row may stand for.
const MAX_RANGE_EXPANSION: usize = 256;

/// Pull the CDF parameter table out of one symbol's section.
///
/// Sections also contain *value* tables — `capq` has a 2-column `Mode |
/// Description` table whose rows are prose, not parameters. A blanket `<tr>`
/// scan swallows those and produces "parameters" whose name is a whole
/// sentence. The parameter table is identified by its header instead: its
/// second column is always `CDF Parameter`. Column count is not usable — the
/// tables are 3 columns in some chapters and 7 in others.
fn parse_params(
    body: &str,
    appendix: &HashMap<String, Vec<AppendixRecord>>,
) -> (Vec<CdfParam>, usize) {
    let mut params = Vec::new();
    let mut skipped_rows = 0usize;

    for table in RE_TABLE.find_iter(body) {
        let rows: Vec<&str> = RE_ROW
            .captures_iter(table.as_str())
            .map(|c| c.get(1).map(|m| m.as_str()).unwrap_or(""))
            .collect();
        let Some(header) = rows.first() else {
            continue;
        };
        let hdr = cells(header);
        if hdr.len() < 2 || !hdr[1].contains("CDF Parameter") {
            skipped_rows += rows.len().saturating_sub(1);
            continue;
        }
        for row in &rows[1..] {
            let c = cells(row);
            if c.len() < 2 || c[1].is_empty() {
                continue;
            }
            let (label, name) = (c[0].clone(), c[1].clone());
            let spectre = c.get(2).cloned().unwrap_or_default();
            // Prefer the appendix record whose label matches this table's
            // label; fall back to the first record for the name.
            let record = appendix.get(&name).and_then(|recs| {
                recs.iter()
                    .find(|r| r.label == label)
                    .or_else(|| recs.first())
            });
            params.push(CdfParam {
                expands_to: expand_range(&name),
                label,
                name,
                spectre,
                description: record.map(|r| r.description.clone()).unwrap_or_default(),
                default: record.map(|r| r.default.clone()).unwrap_or_default(),
            });
        }
    }
    (params, skipped_rows)
}

/// Parse every release found in `dir` into symbols, plus a parse report.
pub fn parse_analoglib_directory_reported(dir: &Path) -> (Vec<LibSymbol>, ParseReport) {
    let mut symbols = Vec::new();
    let mut report = ParseReport::default();

    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return (symbols, report);
    };

    // Cached files are namespaced `<RELEASE>__<basename>`; a directly-loaded
    // Cadence tree has bare basenames. Both are just a prefix on every file.
    let mut prefixes: Vec<String> = read_dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            name.strip_suffix(INDEX_BASENAME).map(|p| p.to_string())
        })
        .collect();
    prefixes.sort();
    prefixes.dedup();

    for prefix in prefixes {
        let (mut s, r) = parse_release(dir, &prefix);
        symbols.append(&mut s);
        report.indexed += r.indexed;
        report.parsed += r.parsed;
        report.skipped_rows += r.skipped_rows;
        report.unresolved.extend(r.unresolved);
    }

    let merged = merge_cross_reference_stubs(symbols);
    (merged, report)
}

/// Convenience wrapper that drops the report.
pub fn parse_analoglib_directory(dir: &Path) -> Vec<LibSymbol> {
    parse_analoglib_directory_reported(dir).0
}

fn parse_release(dir: &Path, prefix: &str) -> (Vec<LibSymbol>, ParseReport) {
    let mut report = ParseReport::default();
    let release = prefix.trim_end_matches('_').to_string();

    let index = parse_index(&read_lossy(&dir.join(format!("{prefix}{INDEX_BASENAME}"))));
    report.indexed = index.len();
    if index.is_empty() {
        return (Vec::new(), report);
    }

    let appendix = parse_appendix(&read_lossy(&dir.join(format!("{prefix}{APPENDIX_BASENAME}"))));

    let mut by_file: BTreeMap<String, Vec<IndexEntry>> = BTreeMap::new();
    for entry in index {
        by_file.entry(entry.file.clone()).or_default().push(entry);
    }

    let mut out = Vec::new();
    for (file, entries) in by_file {
        let html = read_lossy(&dir.join(format!("{prefix}{file}")));
        if html.is_empty() {
            for e in &entries {
                report
                    .unresolved
                    .push(format!("{} ({}#{})", e.name, e.file, e.anchor));
            }
            continue;
        }

        // Locate every section first, then sort by offset: a section runs from
        // its own anchor to the next one in the same file.
        let mut located: Vec<(usize, IndexEntry)> = Vec::new();
        for entry in entries {
            match find_anchor(&html, &entry.anchor) {
                Some(offset) => located.push((offset, entry)),
                None => report
                    .unresolved
                    .push(format!("{} ({}#{})", entry.name, entry.file, entry.anchor)),
            }
        }
        located.sort_by_key(|(offset, _)| *offset);

        for i in 0..located.len() {
            let start = located[i].0;
            let end = located.get(i + 1).map(|(o, _)| *o).unwrap_or(html.len());
            let body = &html[start..end];
            let entry = &located[i].1;

            let title = RE_STRONG
                .captures(body)
                .map(|c| plain(&c[1]))
                .unwrap_or_default();
            let mut primitives: Vec<String> = RE_PRIMITIVE
                .captures_iter(body)
                .map(|c| c[1].to_string())
                .collect();
            primitives.dedup();

            let (params, skipped) = parse_params(body, &appendix);
            report.skipped_rows += skipped;
            report.parsed += 1;

            out.push(LibSymbol {
                lib: LIB.to_string(),
                name: entry.name.clone(),
                category: entry.category.clone(),
                title,
                description: String::new(),
                primitives,
                params,
                release: release.clone(),
                source_file: file.clone(),
                also_documented_in: Vec::new(),
            });
        }
    }

    (out, report)
}

/// Fold `ports.html`-style cross-reference stubs into the real entry.
///
/// Seven symbols (`port`, `pdc`, `pexp`, `ppulse`, `ppwl`, `ppwlf`, `psin`)
/// are indexed twice: once in `independent.html` with the full table (`port`
/// has 48 parameters) and once in `ports.html` as a stub that says only *"This
/// component is the same as … described in Chapter 8"*. Left alone, a lookup
/// could return whichever came first and report zero parameters — the tool
/// stating its own parse gap as a fact about the device. Keep the richest
/// entry per `(release, name)` and record the other chapters on it.
fn merge_cross_reference_stubs(symbols: Vec<LibSymbol>) -> Vec<LibSymbol> {
    let mut best: BTreeMap<(String, String), LibSymbol> = BTreeMap::new();
    let mut order: Vec<(String, String)> = Vec::new();

    for symbol in symbols {
        let key = (symbol.release.clone(), symbol.name.clone());
        match best.get_mut(&key) {
            None => {
                order.push(key.clone());
                best.insert(key, symbol);
            }
            Some(existing) => {
                if symbol.params.len() > existing.params.len() {
                    let mut winner = symbol;
                    winner
                        .also_documented_in
                        .append(&mut existing.also_documented_in);
                    winner.also_documented_in.push(existing.source_file.clone());
                    *existing = winner;
                } else {
                    existing.also_documented_in.push(symbol.source_file);
                }
            }
        }
    }

    order
        .into_iter()
        .filter_map(|k| best.remove(&k))
        .map(|mut s| {
            s.also_documented_in.sort();
            s.also_documented_in.dedup();
            s
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_strips_tags_entities_and_whitespace() {
        assert_eq!(plain("<b>AC magnitude</b>"), "AC magnitude");
        assert_eq!(plain("a&nbsp;&amp;&nbsp;b"), "a & b");
        assert_eq!(plain("line one\n\n   line two"), "line one line two");
        // `&amp;` is expanded last so an escaped entity survives intact.
        assert_eq!(plain("&amp;lt;"), "&lt;");
    }

    /// The heading text is unusable as a locator: Cadence embeds anchors
    /// mid-word. Six symbols are found only via their `pgfId` anchor.
    #[test]
    fn anchors_are_found_even_when_split_across_the_heading() {
        let html = r#"<h3><a id="pgfId-1"></a>Sym<a id="bsource"></a>bol: bsource</h3>"#;
        assert!(html.find("Symbol: bsource").is_none(), "the naive scan fails");
        // The offset lands inside the opening `<a …>`, i.e. at or before the
        // section it introduces — that is all the section slicer needs.
        let at = find_anchor(html, "pgfId-1").expect("anchor is locatable");
        assert!(at < html.find("bol: bsource").unwrap());
        assert!(html[at..].starts_with(r#"id="pgfId-1""#));
        assert!(find_anchor(html, "bsource").is_some());
    }

    /// `F1 - F50` is fifty parameters printed as one row. The live instance
    /// has all fifty; the manual has the notation.
    #[test]
    fn compact_ranges_expand_to_the_names_a_caller_can_set() {
        let f = expand_range("F1 - F50");
        assert_eq!(f.len(), 50);
        assert_eq!(f[0], "F1");
        assert_eq!(f[49], "F50");
        assert_eq!(expand_range("N1-N50").len(), 50, "spacing is optional");
    }

    #[test]
    fn an_ordinary_name_is_not_treated_as_a_range() {
        for name in ["va", "vaDBm", "acm", "pam4_modulation", "nrd"] {
            assert!(expand_range(name).is_empty(), "{name} is not a range");
        }
    }

    /// Guards against the parser inventing parameters out of malformed markup.
    #[test]
    fn malformed_or_unbounded_ranges_expand_to_nothing() {
        assert!(expand_range("F1 - N50").is_empty(), "prefixes must match");
        assert!(expand_range("F50 - F1").is_empty(), "must ascend");
        assert!(expand_range("F1 - F1").is_empty(), "a range of one is a name");
        assert!(
            expand_range("F0 - F99999").is_empty(),
            "one bad row must not become 100k parameters"
        );
    }

    #[test]
    fn find_anchor_accepts_both_name_and_id_spellings() {
        assert!(find_anchor(r#"<a name="pgfId-9"></a>"#, "pgfId-9").is_some());
        assert!(find_anchor(r#"<a id="pgfId-9"></a>"#, "pgfId-9").is_some());
        assert!(find_anchor(r#"<a id="other"></a>"#, "pgfId-9").is_none());
    }

    // Extra hashes: the index JSON contains `"#"` (jstree's root parent) and
    // `#pgfId-…` fragments, either of which ends an `r#"…"#` literal early.
    fn index_json() -> &'static str {
        r##"{"core":{"data":[
          {"id":"c1","parent":"#","text":"Passive Components"},
          {"id":"s1","parent":"c1","text":"Symbol: cap","href":"passives.html#pgfId-100"},
          {"id":"s2","parent":"c1","text":"Symbol: capq","href":"passives.html#pgfId-200"},
          {"id":"x1","parent":"c1","text":"Not a symbol node","href":"passives.html#pgfId-300"}
        ]}}"##
    }

    #[test]
    fn index_yields_symbol_nodes_with_category_from_the_parent() {
        let idx = parse_index(index_json());
        assert_eq!(idx.len(), 2, "non-symbol nodes are not entries");
        assert_eq!(idx[0].name, "cap");
        assert_eq!(idx[0].file, "passives.html");
        assert_eq!(idx[0].anchor, "pgfId-100");
        assert_eq!(idx[0].category, "Passive Components");
    }

    #[test]
    fn index_survives_malformed_json() {
        assert!(parse_index("not json at all").is_empty());
        assert!(parse_index(r#"{"core":{}}"#).is_empty());
    }

    #[test]
    fn appendix_disambiguates_a_reused_cdf_name_by_label() {
        let html = r#"
          <tr><td>Label</td><td>CDF Parameter Name</td><td>Description</td><td>Default</td></tr>
          <tr><td>Capacitor Area</td><td>area</td><td>Area of capacitor</td><td>1</td></tr>
          <tr><td>Device area</td><td>area</td><td>Transistor area factor.</td><td>-</td></tr>"#;
        let a = parse_appendix(html);
        assert_eq!(a["area"].len(), 2);
        assert_eq!(a["area"][0].label, "Capacitor Area");
        assert!(!a.contains_key("CDF Parameter Name"), "header row is not data");
    }

    /// A section's value tables must not become parameters. `capq` documents
    /// its `mode` values in a 2-column `Mode | Description` table; scooping
    /// those up yields "parameters" whose CDF name is a whole sentence.
    #[test]
    fn value_tables_are_not_mistaken_for_parameter_tables() {
        let body = r#"
          <table><tr><td>Mode</td><td>Description</td></tr>
                 <tr><td>1</td><td>Constant real part of the admittance.</td></tr>
                 <tr><td>2</td><td>Re(Y) increases proportional to sqrt(freq).</td></tr></table>
          <table><tr><td>CDF Parameter Label</td><td>CDF Parameter</td><td>spectre</td></tr>
                 <tr><td>Capacitance</td><td>c</td><td>c</td></tr></table>"#;
        let (params, skipped) = parse_params(body, &HashMap::new());
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "c");
        assert_eq!(skipped, 2, "the two value rows are counted, not silently lost");
    }

    /// The tables are 3 columns in some chapters and 7 in others, so the
    /// header — not the column count — has to be the discriminator.
    #[test]
    fn parameter_tables_are_recognised_at_any_column_count() {
        let seven = r#"<table>
          <tr><td>CDF Parameter Label</td><td>CDF Parameter</td><td>spectre</td>
              <td>auCdl</td><td>auLvs</td><td>hspiceD</td><td>UltraSim</td></tr>
          <tr><td>Amplitude 1 (Vpk)</td><td>va</td><td>-</td>
              <td>-</td><td>-</td><td>-</td><td>-</td></tr></table>"#;
        let (params, _) = parse_params(seven, &HashMap::new());
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "va");
        assert_eq!(params[0].label, "Amplitude 1 (Vpk)");
    }

    #[test]
    fn appendix_prose_is_merged_onto_the_matching_label() {
        let mut appendix = HashMap::new();
        appendix.insert(
            "c".to_string(),
            vec![AppendixRecord {
                label: "Capacitance".into(),
                description: "Capacitance".into(),
                default: "1p F".into(),
            }],
        );
        let body = r#"<table>
          <tr><td>CDF Parameter Label</td><td>CDF Parameter</td><td>spectre</td></tr>
          <tr><td>Capacitance</td><td>c</td><td>c</td></tr></table>"#;
        let (params, _) = parse_params(body, &appendix);
        assert_eq!(params[0].default, "1p F");
        assert_eq!(params[0].description, "Capacitance");
    }

    fn sym(name: &str, file: &str, nparams: usize) -> LibSymbol {
        LibSymbol {
            name: name.into(),
            release: "IC231".into(),
            source_file: file.into(),
            params: vec![CdfParam::default(); nparams],
            ..Default::default()
        }
    }

    /// `ports.html` stubs must never shadow the real table.
    #[test]
    fn cross_reference_stub_never_shadows_the_full_entry() {
        for order in [
            vec![sym("port", "independent.html", 48), sym("port", "ports.html", 0)],
            vec![sym("port", "ports.html", 0), sym("port", "independent.html", 48)],
        ] {
            let merged = merge_cross_reference_stubs(order);
            assert_eq!(merged.len(), 1, "one entry per (release, name)");
            assert_eq!(merged[0].params.len(), 48, "the richest entry wins");
            assert_eq!(merged[0].source_file, "independent.html");
            assert_eq!(merged[0].also_documented_in, vec!["ports.html"]);
        }
    }

    #[test]
    fn same_name_in_two_releases_stays_separate() {
        let a = LibSymbol {
            release: "IC231".into(),
            ..sym("cap", "passives.html", 15)
        };
        let b = LibSymbol {
            release: "IC618".into(),
            ..sym("cap", "passives.html", 12)
        };
        assert_eq!(merge_cross_reference_stubs(vec![a, b]).len(), 2);
    }

    #[test]
    fn param_lookup_is_case_insensitive() {
        let mut s = sym("vsin", "independent.html", 0);
        s.params.push(CdfParam {
            name: "va".into(),
            label: "Amplitude 1 (Vpk)".into(),
            ..Default::default()
        });
        assert_eq!(s.param("VA").unwrap().label, "Amplitude 1 (Vpk)");
        assert!(s.param("ampl").is_none(), "the guess that started this");
    }

    #[test]
    fn missing_directory_yields_no_symbols_and_no_panic() {
        let (symbols, report) =
            parse_analoglib_directory_reported(Path::new("/nonexistent/analoglibref"));
        assert!(symbols.is_empty());
        assert_eq!(report.indexed, 0);
    }
}
