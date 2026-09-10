//! Parser for Cadence's `doc/basicLib/` — the Virtuoso Basic Library reference.
//!
//! # Why this exists separately from [`super::analoglib`]
//!
//! Every schematic this project builds places `basic` cells — `ipin`, `opin`,
//! `iopin` for the ports, `gnd`/`vdd` for the supplies — and none of them are
//! in the analogLib manual. Pointing the analogLib parser at this directory
//! returns nothing, because the two manuals share almost no structure:
//!
//! | | `analoglibref` | `basicLib` |
//! |---|---|---|
//! | Files | ~17 chapters | one `basicLib.html` |
//! | Index label | `"Symbol: vsin"` | `"ipin"` — a bare name |
//! | Section marker | `Symbol: <name>` heading | `<h3>` heading |
//! | Payload | CDF parameter table | a *Description* paragraph |
//! | Parameters | 2000 across 149 symbols | **none, anywhere** |
//!
//! That last row is the one worth stating out loud: `basic` cells genuinely
//! carry no documented CDF parameters, so an empty `params` list here is a fact
//! about the manual, not a parse failure — and the answer says which.
//!
//! # Two traps in this file's shape
//!
//! **Sections must be bounded by `<h2>` as well as `<h3>`.** The `Obsolete` and
//! `VHDLPins` categories have headings but no cells under them: they are just a
//! multi-column table of names. They sit immediately after `patch` (last of
//! *Misc*) and `vss_inherit` (last of *Supplies*), so a section that runs "to
//! the next `<h3>`" swallows the whole listing and hands it back as those two
//! cells' description.
//!
//! **Anchors are embedded mid-word,** exactly as in analogLib:
//!
//! ```text
//! <h3><a id="pgfId-1065702"></a><a id="76761"></a>ipi<a id="ipin"></a>n</h3>
//! ```
//!
//! Searching for the heading text `ipin` finds nothing. The index's `pgfId-…`
//! anchor is the first thing inside the `<h3>`, so the index drives the parse
//! here for the same reason it does in analogLib.

use super::html::{find_anchor, parse_jstree, plain, read_lossy};
use super::types::{LibSymbol, ParseReport};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;

/// Basename of the jstree index, with or without a `<RELEASE>__` cache prefix.
pub const INDEX_BASENAME: &str = "basiclib.json";

/// Virtuoso library name these cells belong to.
const LIB: &str = "basic";

/// Depth of a cell node in the index tree.
///
/// `Root(0) → "Basic Library Categories"(1) → "Pins"(2) → "ipin"(3)`. Depth is
/// the discriminator rather than "has no children" because `Obsolete` and
/// `VHDLPins` are childless categories at depth 2.
const CELL_DEPTH: usize = 3;

static RE_HEADING: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)<h([23])[\s>]").unwrap());
static RE_SUBHEADING: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)<h4[\s>]").unwrap());

/// Parse every release found in `dir` into symbols, plus a parse report.
pub fn parse_basiclib_directory_reported(dir: &Path) -> (Vec<LibSymbol>, ParseReport) {
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
        report.absorb(r);
    }
    (symbols, report)
}

/// Convenience wrapper that drops the report.
pub fn parse_basiclib_directory(dir: &Path) -> Vec<LibSymbol> {
    parse_basiclib_directory_reported(dir).0
}

fn parse_release(dir: &Path, prefix: &str) -> (Vec<LibSymbol>, ParseReport) {
    let mut report = ParseReport::default();
    let release = prefix.trim_end_matches('_').to_string();

    let index: Vec<_> = parse_jstree(&read_lossy(&dir.join(format!("{prefix}{INDEX_BASENAME}"))))
        .into_iter()
        .filter(|n| n.depth == CELL_DEPTH && !n.anchor.is_empty() && !n.file.is_empty())
        .collect();
    report.indexed = index.len();
    if index.is_empty() {
        return (Vec::new(), report);
    }

    let mut by_file: BTreeMap<String, Vec<super::html::IndexNode>> = BTreeMap::new();
    for node in index {
        by_file.entry(node.file.clone()).or_default().push(node);
    }

    let mut out = Vec::new();
    for (file, nodes) in by_file {
        let html = read_lossy(&dir.join(format!("{prefix}{file}")));
        if html.is_empty() {
            for n in &nodes {
                report
                    .unresolved
                    .push(format!("{} ({}#{})", n.text, n.file, n.anchor));
            }
            continue;
        }
        let headings = heading_offsets(&html);

        for node in nodes {
            let Some(at) = find_anchor(&html, &node.anchor) else {
                report
                    .unresolved
                    .push(format!("{} ({}#{})", node.text, node.file, node.anchor));
                continue;
            };
            let Some((start, end)) = section_bounds(&headings, at, html.len()) else {
                report.unresolved.push(format!(
                    "{} ({}#{}): anchor is not inside an <h3> section",
                    node.text, node.file, node.anchor
                ));
                continue;
            };
            let body = &html[start..end];

            // The heading must be the cell the index named. A mismatch means
            // the anchor resolved into a neighbouring section, and attaching
            // another cell's prose to this name is worse than admitting the
            // miss — it is a wrong answer that reads like a right one.
            let heading = heading_text(body);
            if !heading.eq_ignore_ascii_case(&node.text) {
                report.unresolved.push(format!(
                    "{} ({}#{}): section heading says '{}'",
                    node.text, node.file, node.anchor, heading
                ));
                continue;
            }

            report.parsed += 1;
            out.push(LibSymbol {
                lib: LIB.to_string(),
                name: node.text.clone(),
                category: node.parent.clone(),
                title: String::new(),
                description: description(body),
                primitives: Vec::new(),
                params: Vec::new(),
                release: release.clone(),
                source_file: file.clone(),
                also_documented_in: Vec::new(),
            });
        }
    }

    (out, report)
}

/// Byte offsets of every `<h2>`/`<h3>` open tag, with its level, in order.
fn heading_offsets(html: &str) -> Vec<(usize, u8)> {
    RE_HEADING
        .captures_iter(html)
        .filter_map(|c| {
            let m = c.get(0)?;
            let level = c.get(1)?.as_str().parse::<u8>().ok()?;
            Some((m.start(), level))
        })
        .collect()
}

/// The `<h3>` section containing `at`, bounded by the next heading of *any*
/// level — see the `Obsolete` / `VHDLPins` trap in the module docs.
fn section_bounds(headings: &[(usize, u8)], at: usize, len: usize) -> Option<(usize, usize)> {
    let idx = headings.iter().rposition(|(off, _)| *off <= at)?;
    let (start, level) = headings[idx];
    if level != 3 {
        return None;
    }
    let end = headings
        .get(idx + 1)
        .map(|(off, _)| *off)
        .unwrap_or(len)
        .min(len);
    Some((start, end))
}

/// Text of the section's own heading, with the mid-word anchors stripped out.
fn heading_text(body: &str) -> String {
    match find_ci(body, "</h3>") {
        Some(end) => plain(&body[..end]),
        None => String::new(),
    }
}

/// The prose under the section's *Description* subheading.
///
/// Walked as offsets rather than matched as one regex: the pattern this needs —
/// "an `<h4>` whose heading text says Description, up to the next `<h4>`" —
/// wants a negative lookahead to keep `.*?` from running through `</h4>`, and
/// the `regex` crate has none. A `<h4[^>]*>.*?Description` without it happily
/// starts at *Symbol View* and matches across into the next subsection.
fn description(body: &str) -> String {
    let heads: Vec<usize> = RE_SUBHEADING.find_iter(body).map(|m| m.start()).collect();
    for (i, &start) in heads.iter().enumerate() {
        let end = heads.get(i + 1).copied().unwrap_or(body.len());
        let section = &body[start..end];
        let Some(close) = find_ci(section, "</h4>") else {
            continue;
        };
        if !plain(&section[..close])
            .to_ascii_lowercase()
            .contains("description")
        {
            continue;
        }
        return plain(&section[close + "</h4>".len()..]);
    }
    String::new()
}

/// Byte offset of `needle` (given lowercase) in `hay`, ignoring ASCII case.
///
/// `to_ascii_lowercase` is byte-for-byte on the tag characters, so offsets into
/// the lowered copy are offsets into the original — which `to_lowercase` would
/// not guarantee on the Latin-1 bytes Cadence's HTML carries.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    hay.to_ascii_lowercase().find(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed copy of the real `basicLib.html` shape, including the
    /// `Obsolete` listing that follows the last cell of a category.
    const HTML: &str = r#"<h1>Basic Library Categories</h1>
<h2><a id="cat-misc"></a>Misc</h2>
<h3><a id="pgfId-1"></a>cds_ali<a id="cds_alias"></a>as</h3>
<h4><a id="p"></a>Symbol View</h4><p><img src="x.gif" /></p>
<h4><a id="q"></a>Description</h4>
<p><a id="r"></a>An alias for a net, used in <code>schematic</code> views.</p>
<h4><em><a id="s"></a>Related Topics</em></h4><p>See the guide.</p>
<h3><a id="pgfId-2"></a>pat<a id="patch"></a>ch</h3>
<h4><a id="t"></a>Description</h4>
<p>A patch cord &#8212; joins two nets.</p>
<h2><a id="cat-obs"></a>Obsolete</h2>
<table><tr><td>FCON</td><td>MCON</td></tr><tr><td>idc</td><td>vsin</td></tr></table>
"#;

    const INDEX: &str = r##"{"core":{"data":[
        {"id":"n0","text":"Virtuoso Basic Library Reference","parent":"#","href":"TOC.html"},
        {"id":"n1","text":"Basic Library Categories","parent":"n0","href":"basicLib.html#top"},
        {"id":"n2","text":"Misc","parent":"n1","href":"basicLib.html#cat-misc"},
        {"id":"n3","text":"cds_alias","parent":"n2","href":"basicLib.html#pgfId-1"},
        {"id":"n4","text":"patch","parent":"n2","href":"basicLib.html#pgfId-2"},
        {"id":"n5","text":"Obsolete","parent":"n1","href":"basicLib.html#cat-obs"}
    ]}}"##;

    fn fixture() -> (tempfile::TempDir, Vec<LibSymbol>, ParseReport) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("basiclib.json"), INDEX).unwrap();
        std::fs::write(dir.path().join("basicLib.html"), HTML).unwrap();
        let (s, r) = parse_basiclib_directory_reported(dir.path());
        (dir, s, r)
    }

    #[test]
    fn only_cells_are_indexed_not_the_categories_above_them() {
        let (_d, symbols, report) = fixture();
        assert_eq!(report.indexed, 2, "Misc and Obsolete are not cells");
        assert_eq!(report.parsed, 2);
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["cds_alias", "patch"]);
    }

    /// The mid-word anchor case: the heading reads `cds_ali<a…>as`, so the
    /// name only survives if tags are stripped before comparing.
    #[test]
    fn a_mid_word_anchor_does_not_lose_the_cell() {
        let (_d, symbols, _) = fixture();
        let s = symbols.iter().find(|s| s.name == "cds_alias").unwrap();
        assert_eq!(s.category, "Misc");
        assert_eq!(s.lib, "basic");
        assert!(s.description.starts_with("An alias for a net"));
    }

    /// *Description* is neither the first nor the last `<h4>` in a section:
    /// *Symbol View* precedes it and *Related Topics* follows. Both are
    /// neighbours, not part of the answer.
    #[test]
    fn the_description_starts_after_symbol_view_and_stops_before_related_topics() {
        let (_d, symbols, _) = fixture();
        let s = symbols.iter().find(|s| s.name == "cds_alias").unwrap();
        assert_eq!(
            s.description,
            "An alias for a net, used in schematic views."
        );
        assert!(!s.description.contains("x.gif"));
        assert!(!s.description.contains("See the guide"));
    }

    /// The `Obsolete` trap: `patch` is the last cell of its category, and the
    /// obsolete-cell table sits between it and the next `<h3>`. Bounding the
    /// section at the next `<h3>` only would hand that table back as `patch`'s
    /// description — a listing of unrelated cell names presented as prose.
    #[test]
    fn a_section_stops_at_the_next_category_not_the_next_cell() {
        let (_d, symbols, _) = fixture();
        let patch = symbols.iter().find(|s| s.name == "patch").unwrap();
        assert_eq!(patch.description, "A patch cord - joins two nets.");
        assert!(!patch.description.contains("FCON"));
        assert!(!patch.description.contains("vsin"));
    }

    /// No `basic` cell has a documented CDF parameter. That has to read as a
    /// property of the manual, so the parse must report full success — a
    /// `parsed` count short of `indexed` would say the opposite.
    #[test]
    fn no_parameters_is_a_complete_parse_not_a_failed_one() {
        let (_d, symbols, report) = fixture();
        assert!(symbols.iter().all(|s| s.params.is_empty()));
        assert_eq!(report.parsed, report.indexed);
        assert_eq!(report.skipped_rows, 0);
    }

    #[test]
    fn entities_in_the_prose_are_decoded() {
        let (_d, symbols, _) = fixture();
        let patch = symbols.iter().find(|s| s.name == "patch").unwrap();
        assert!(!patch.description.contains("&#"));
    }

    /// An anchor that lands on a category heading is not a cell section; the
    /// entry is reported unresolved rather than given its neighbour's prose.
    #[test]
    fn an_anchor_outside_any_h3_is_reported_not_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let index = INDEX.replace(r#""href":"basicLib.html#pgfId-1""#, r#""href":"basicLib.html#cat-misc""#);
        std::fs::write(dir.path().join("basiclib.json"), &index).unwrap();
        std::fs::write(dir.path().join("basicLib.html"), HTML).unwrap();
        let (symbols, report) = parse_basiclib_directory_reported(dir.path());
        assert_eq!(report.parsed, 1);
        assert_eq!(report.unresolved.len(), 1);
        assert!(report.unresolved[0].contains("not inside an <h3>"));
        assert!(!symbols.iter().any(|s| s.name == "cds_alias"));
    }

    /// If the heading does not name the cell the index asked for, the anchor
    /// resolved into the wrong section. Admit the miss.
    #[test]
    fn a_heading_that_names_another_cell_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let index = INDEX.replace(r#""text":"cds_alias""#, r#""text":"cds_thru""#);
        std::fs::write(dir.path().join("basiclib.json"), &index).unwrap();
        std::fs::write(dir.path().join("basicLib.html"), HTML).unwrap();
        let (symbols, report) = parse_basiclib_directory_reported(dir.path());
        assert!(!symbols.iter().any(|s| s.name == "cds_thru"));
        assert_eq!(report.unresolved.len(), 1);
        assert!(report.unresolved[0].contains("heading says 'cds_alias'"));
    }

    #[test]
    fn a_missing_html_file_is_reported_rather_than_returning_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("basiclib.json"), INDEX).unwrap();
        let (symbols, report) = parse_basiclib_directory_reported(dir.path());
        assert!(symbols.is_empty());
        assert_eq!(report.indexed, 2);
        assert_eq!(report.parsed, 0);
        assert_eq!(report.unresolved.len(), 2);
    }

    #[test]
    fn a_release_prefixed_cache_parses_the_same_way() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("IC231__basiclib.json"), INDEX).unwrap();
        std::fs::write(dir.path().join("IC231__basicLib.html"), HTML).unwrap();
        let (symbols, _) = parse_basiclib_directory_reported(dir.path());
        assert_eq!(symbols.len(), 2);
        assert!(symbols.iter().all(|s| s.release == "IC231"));
    }

    #[test]
    fn an_empty_directory_yields_an_empty_report_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let (symbols, report) = parse_basiclib_directory_reported(dir.path());
        assert!(symbols.is_empty());
        assert_eq!(report.indexed, 0);
    }

    #[test]
    fn section_bounds_rejects_an_offset_before_any_heading() {
        assert!(section_bounds(&[(100, 3)], 5, 200).is_none());
    }
}
