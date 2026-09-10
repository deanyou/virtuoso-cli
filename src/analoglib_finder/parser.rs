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

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Basename of the jstree index, with or without a `<RELEASE>__` cache prefix.
pub const INDEX_BASENAME: &str = "analoglibref.json";
/// Basename of the "List of All CDF Parameters" appendix.
pub const APPENDIX_BASENAME: &str = "appA.html";

static RE_TABLE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<table[^>]*>.*?</table>").unwrap());
static RE_ROW: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<tr[^>]*>(.*?)</tr>").unwrap());
static RE_CELL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<t[dh][^>]*>(.*?)</t[dh]>").unwrap());
static RE_STRONG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<strong>(.*?)</strong>").unwrap());
static RE_PRIMITIVE: Lazy<Regex> = Lazy::new(|| Regex::new(r"spectre -h (\w+)").unwrap());
static RE_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<[^>]+>").unwrap());
static RE_WS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static RE_RANGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^([A-Za-z_]+)(\d+)\s*-\s*([A-Za-z_]+)(\d+)$").unwrap());

/// One CDF parameter of one analogLib symbol.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdfParam {
    /// The label shown in the ADE / schematic property form, e.g. `Amplitude 1 (Vpk)`.
    pub label: String,
    /// The CDF parameter name — what `schematic.set_param` takes, e.g. `va`.
    pub name: String,
    /// The corresponding Spectre netlist parameter, when the table gives one.
    pub spectre: String,
    /// Description, from `appA.html`. Empty when the appendix has no prose for it.
    pub description: String,
    /// Default value, from `appA.html`. `-` means "no default" in Cadence's tables.
    pub default: String,
    /// Concrete names, when `name` is a range like `F1 - F50`. Empty otherwise.
    ///
    /// The manual compresses fifty rows into one; the live instance does not.
    /// Cross-checking `vsin` against `schematic.list_cdf_params` showed exactly
    /// this: 37 documented rows vs 135 live names, the whole gap being two
    /// ranges. Without the expansion, `name` is a string no caller can pass to
    /// `set_param` — the tool's own notation presented as the parameter's name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expands_to: Vec<String>,
}

/// One analogLib symbol, as documented.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalogLibSymbol {
    /// Symbol name as it appears in the library, e.g. `vsin`.
    pub name: String,
    /// Chapter/category the symbol is filed under, e.g. `Sources - Independent`.
    pub category: String,
    /// Human title, e.g. `Independent Sinusoidal Voltage Source`.
    pub title: String,
    /// Spectre primitives named by the section's `spectre -h <x>` hints.
    /// `nmos4` lists three (`mos0`, `mos1`, `ekv`) and has no CDF table at all.
    pub primitives: Vec<String>,
    /// The symbol's CDF parameters.
    pub params: Vec<CdfParam>,
    /// Cadence release the entry came from, e.g. `IC231`. Empty if unprefixed.
    pub release: String,
    /// Chapter file the entry was parsed from, e.g. `independent.html`.
    pub source_file: String,
    /// Other chapters that document this same symbol. `ports.html` carries
    /// cross-reference stubs — *"This component is the same as psin described
    /// in Chapter 8"* — for the seven port sources, with no parameter table of
    /// their own. Those stubs are folded into the real entry rather than left
    /// to shadow it, and recorded here so the merge is visible.
    pub also_documented_in: Vec<String>,
}

impl AnalogLibSymbol {
    /// One-line summary for list output.
    pub fn summary(&self) -> String {
        format!(
            "{:<14} {:<26} {} param(s)",
            self.name,
            self.category,
            self.params.len()
        )
    }

    /// Look up a single CDF parameter by name (case-insensitive).
    pub fn param(&self, name: &str) -> Option<&CdfParam> {
        self.params
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// An entry of the jstree index.
#[derive(Debug, Clone)]
struct IndexEntry {
    name: String,
    file: String,
    anchor: String,
    category: String,
}

/// Strip HTML tags, unescape the handful of entities Cadence emits, and
/// collapse whitespace runs. Cell text spans multiple source lines.
fn plain(s: &str) -> String {
    let no_tags = RE_TAG.replace_all(s, "");
    let unescaped = no_tags
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#8217;", "'")
        .replace("&#8220;", "\"")
        .replace("&#8221;", "\"")
        // `&amp;` last: doing it first would re-expand `&amp;lt;` into `<`.
        .replace("&amp;", "&");
    RE_WS.replace_all(&unescaped, " ").trim().to_string()
}

/// Read a file as text, tolerating the Latin-1 bytes in Cadence's HTML.
fn read_lossy(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}

/// Extract the cells of one `<tr>`.
fn cells(row: &str) -> Vec<String> {
    RE_CELL
        .captures_iter(row)
        .map(|c| plain(&c[1]))
        .collect::<Vec<_>>()
}

/// Parse `analoglibref.json` (a jstree dump) into index entries.
///
/// Shape: `{"core": {"data": [{"id":…, "parent":…, "text":…, "href":…}, …]}}`.
/// Symbol nodes have `text = "Symbol: <name>"`; the parent node's text is the
/// category.
fn parse_index(json: &str) -> Vec<IndexEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(data) = value
        .get("core")
        .and_then(|c| c.get("data"))
        .and_then(|d| d.as_array())
    else {
        return Vec::new();
    };

    let mut text_by_id: HashMap<&str, &str> = HashMap::new();
    for node in data {
        if let (Some(id), Some(text)) = (
            node.get("id").and_then(|v| v.as_str()),
            node.get("text").and_then(|v| v.as_str()),
        ) {
            text_by_id.insert(id, text);
        }
    }

    let mut out = Vec::new();
    for node in data {
        let Some(text) = node.get("text").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(name) = text.strip_prefix("Symbol: ") else {
            continue;
        };
        let Some(href) = node.get("href").and_then(|v| v.as_str()) else {
            continue;
        };
        let (file, anchor) = match href.split_once('#') {
            Some((f, a)) => (f, a),
            None => (href, ""),
        };
        if file.is_empty() || anchor.is_empty() {
            continue;
        }
        let category = node
            .get("parent")
            .and_then(|v| v.as_str())
            .and_then(|p| text_by_id.get(p))
            .map(|t| plain(t))
            .unwrap_or_default();
        out.push(IndexEntry {
            name: name.trim().to_string(),
            file: file.to_string(),
            anchor: anchor.to_string(),
            category,
        });
    }
    out
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

/// Byte offset of an anchor's definition, trying both spellings Cadence uses.
fn find_anchor(html: &str, anchor: &str) -> Option<usize> {
    html.find(&format!("name=\"{anchor}\""))
        .or_else(|| html.find(&format!("id=\"{anchor}\"")))
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
    if &c[1] != &c[3] {
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

/// Outcome of parsing one directory, including what could not be parsed.
#[derive(Debug, Clone, Default)]
pub struct ParseReport {
    /// Symbols named by the index.
    pub indexed: usize,
    /// Symbols whose section anchor was found and parsed.
    pub parsed: usize,
    /// Index entries whose anchor was missing — `"<name> (<file>#<anchor>)"`.
    /// Must stay empty; a non-empty list means the docs changed shape and the
    /// lookup is quietly incomplete.
    pub unresolved: Vec<String>,
    /// Rows dropped because they belonged to a non-parameter table.
    pub skipped_rows: usize,
}

/// Parse every release found in `dir` into symbols, plus a parse report.
pub fn parse_analoglib_directory_reported(dir: &Path) -> (Vec<AnalogLibSymbol>, ParseReport) {
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
pub fn parse_analoglib_directory(dir: &Path) -> Vec<AnalogLibSymbol> {
    parse_analoglib_directory_reported(dir).0
}

fn parse_release(dir: &Path, prefix: &str) -> (Vec<AnalogLibSymbol>, ParseReport) {
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

            out.push(AnalogLibSymbol {
                name: entry.name.clone(),
                category: entry.category.clone(),
                title,
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
fn merge_cross_reference_stubs(symbols: Vec<AnalogLibSymbol>) -> Vec<AnalogLibSymbol> {
    let mut best: BTreeMap<(String, String), AnalogLibSymbol> = BTreeMap::new();
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

    fn sym(name: &str, file: &str, nparams: usize) -> AnalogLibSymbol {
        AnalogLibSymbol {
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
        let a = AnalogLibSymbol {
            release: "IC231".into(),
            ..sym("cap", "passives.html", 15)
        };
        let b = AnalogLibSymbol {
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
