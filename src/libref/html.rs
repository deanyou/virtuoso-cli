//! HTML and jstree helpers shared by the per-library parsers.
//!
//! Every Cadence library reference is FrameMaker-exported HTML with a jstree
//! sidebar, so tag stripping, entity decoding and anchor location are the same
//! job in each. The *shapes* differ — `analoglibref` spreads sections over 17
//! chapter files with CDF tables, `basicLib` is one file of prose — and that is
//! what the per-library parsers are for.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

pub static RE_TABLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<table[^>]*>.*?</table>").unwrap());
pub static RE_ROW: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<tr[^>]*>(.*?)</tr>").unwrap());
pub static RE_CELL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<t[dh][^>]*>(.*?)</t[dh]>").unwrap());
static RE_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<[^>]+>").unwrap());
static RE_WS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

/// Strip HTML tags, unescape the entities Cadence emits, and collapse
/// whitespace runs. Cell text spans multiple source lines.
pub fn plain(s: &str) -> String {
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
        // Em/en dashes: `basicLib` writes the menu path *Create — Pin* with
        // `&#8212;`, and a literal `&#8212;` in the answer is the tool showing
        // its own encoding rather than the manual's words.
        .replace("&#8211;", "-")
        .replace("&#8212;", "-")
        .replace("&#169;", "(c)")
        // `&amp;` last: doing it first would re-expand `&amp;lt;` into `<`.
        .replace("&amp;", "&");
    RE_WS.replace_all(&unescaped, " ").trim().to_string()
}

/// Read a file as text, tolerating the Latin-1 bytes in Cadence's HTML.
pub fn read_lossy(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}

/// Extract the cells of one `<tr>`.
pub fn cells(row: &str) -> Vec<String> {
    RE_CELL
        .captures_iter(row)
        .map(|c| plain(&c[1]))
        .collect::<Vec<_>>()
}

/// Byte offset of an anchor's definition, trying both spellings Cadence uses.
pub fn find_anchor(html: &str, anchor: &str) -> Option<usize> {
    html.find(&format!("name=\"{anchor}\""))
        .or_else(|| html.find(&format!("id=\"{anchor}\"")))
}

/// One node of a jstree sidebar index, with its position in the tree resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexNode {
    /// Node label — `"Symbol: vsin"` in analogLib, `"ipin"` in basicLib.
    pub text: String,
    /// Label of the parent node. This is the category in both layouts.
    pub parent: String,
    /// Distance from the root: the root itself is 0.
    ///
    /// Depth is what separates a cell from a category in `basicLib`, where both
    /// are bare names and two categories (`Obsolete`, `VHDLPins`) have no
    /// children at all — so "has no children" would file them as cells.
    pub depth: usize,
    /// File named by `href`, before the `#`.
    pub file: String,
    /// Anchor named by `href`, after the `#`. Empty when there is none.
    pub anchor: String,
}

/// Parse a jstree dump — `{"core": {"data": [{id, parent, text, href}, …]}}` —
/// into nodes with parent labels and depths resolved.
///
/// Returns every node; filtering to the ones that name a cell is the calling
/// parser's job, because the two layouts mark them differently.
pub fn parse_jstree(json: &str) -> Vec<IndexNode> {
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
    let mut parent_by_id: HashMap<&str, &str> = HashMap::new();
    for node in data {
        let Some(id) = node.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(text) = node.get("text").and_then(|v| v.as_str()) {
            text_by_id.insert(id, text);
        }
        if let Some(parent) = node.get("parent").and_then(|v| v.as_str()) {
            parent_by_id.insert(id, parent);
        }
    }

    let mut out = Vec::new();
    for node in data {
        let (Some(id), Some(text)) = (
            node.get("id").and_then(|v| v.as_str()),
            node.get("text").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let href = node.get("href").and_then(|v| v.as_str()).unwrap_or("");
        let (file, anchor) = match href.split_once('#') {
            Some((f, a)) => (f, a),
            None => (href, ""),
        };
        out.push(IndexNode {
            text: plain(text),
            parent: parent_by_id
                .get(id)
                .and_then(|p| text_by_id.get(*p))
                .map(|t| plain(t))
                .unwrap_or_default(),
            depth: depth_of(&parent_by_id, id, data.len()),
            file: file.to_string(),
            anchor: anchor.to_string(),
        });
    }
    out
}

/// Distance from `id` up to the root of the parent chain.
///
/// A free function rather than a closure over `parent_by_id`: a closure would
/// unify its argument's lifetime with the map's, and inference then demands
/// `'static` for borrows taken out of the parsed `Value`.
///
/// `cap` bounds the walk — a malformed index with a parent cycle is corrupt
/// data, not a reason to hang the loader.
fn depth_of(parent_by_id: &HashMap<&str, &str>, id: &str, cap: usize) -> usize {
    let mut cur = id;
    let mut depth = 0usize;
    while let Some(parent) = parent_by_id.get(cur) {
        if *parent == "#" || depth > cap {
            break;
        }
        cur = parent;
        depth += 1;
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_strips_tags_and_collapses_whitespace() {
        assert_eq!(plain("<h3>ipi<a id=\"ipin\"></a>n</h3>"), "ipin");
        assert_eq!(plain("a\n  b\t c"), "a b c");
    }

    #[test]
    fn entities_are_decoded_including_the_dashes_basiclib_uses() {
        assert_eq!(plain("a&nbsp;&amp;&nbsp;b"), "a & b");
        assert_eq!(plain("Create &#8212; Pin"), "Create - Pin");
        assert_eq!(plain("1 &#8211; 2"), "1 - 2");
        // `&amp;` is expanded last so an escaped entity survives intact.
        assert_eq!(plain("&amp;lt;"), "&lt;");
    }

    #[test]
    fn find_anchor_accepts_both_name_and_id_spellings() {
        assert!(find_anchor(r#"<a name="x"></a>"#, "x").is_some());
        assert!(find_anchor(r#"<a id="x"></a>"#, "x").is_some());
        assert!(find_anchor(r#"<a id="y"></a>"#, "x").is_none());
    }

    const TREE: &str = r##"{"core":{"data":[
        {"id":"n0","text":"Root","parent":"#","href":"TOC.html"},
        {"id":"n1","text":"Categories","parent":"n0","href":"b.html#c0"},
        {"id":"n2","text":"Pins","parent":"n1","href":"b.html#c1"},
        {"id":"n3","text":"ipin","parent":"n2","href":"b.html#pgfId-1"},
        {"id":"n4","text":"Obsolete","parent":"n1","href":"b.html#c2"}
    ]}}"##;

    #[test]
    fn depth_separates_a_cell_from_a_childless_category() {
        let nodes = parse_jstree(TREE);
        let by = |t: &str| nodes.iter().find(|n| n.text == t).unwrap().clone();
        assert_eq!(by("Root").depth, 0);
        assert_eq!(by("Categories").depth, 1);
        assert_eq!(by("Pins").depth, 2);
        assert_eq!(by("ipin").depth, 3);
        // The trap: `Obsolete` has no children, so "leaf" would call it a cell.
        assert_eq!(by("Obsolete").depth, 2);
    }

    #[test]
    fn href_splits_into_file_and_anchor() {
        let n = parse_jstree(TREE)
            .into_iter()
            .find(|n| n.text == "ipin")
            .unwrap();
        assert_eq!(n.file, "b.html");
        assert_eq!(n.anchor, "pgfId-1");
        assert_eq!(n.parent, "Pins");
    }

    #[test]
    fn a_node_without_an_href_still_parses() {
        let nodes = parse_jstree(r##"{"core":{"data":[{"id":"a","text":"X","parent":"#"}]}}"##);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].file, "");
        assert_eq!(nodes[0].anchor, "");
    }

    #[test]
    fn malformed_json_yields_no_nodes_rather_than_panicking() {
        assert!(parse_jstree("not json").is_empty());
        assert!(parse_jstree(r#"{"core":{}}"#).is_empty());
    }

    /// A parent cycle is corrupt data, not a reason to hang the loader.
    #[test]
    fn a_cyclic_index_terminates() {
        let nodes = parse_jstree(
            r#"{"core":{"data":[
                {"id":"a","text":"A","parent":"b"},
                {"id":"b","text":"B","parent":"a"}
            ]}}"#,
        );
        assert_eq!(nodes.len(), 2);
    }
}
