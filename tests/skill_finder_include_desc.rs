//! Integration tests for the `vcli skill find --include-desc` flag (added in
//! v0.3.18 to match bridge-lite's skill-finder parity).
//!
//! Uses the public SKILLFinder API (in-process) to validate that:
//!   - default search only matches function names
//!   - include_desc=true additionally matches the description field
//!   - all search modes respect the flag in the documented way
//!
//! No network or live Virtuoso is required.

use virtuoso_cli::skill_finder::{SKILLFinder, SearchMode, SkillEntry};

fn corpus() -> SKILLFinder {
    SKILLFinder::for_test(vec![
        SkillEntry {
            name: "dbOpenCellView".into(),
            syntax: "dbOpenCellView(lib cell view)".into(),
            description: "Opens a cellView in the database".into(),
            source_file: Some("db.fnd".into()),
        },
        SkillEntry {
            name: "rodCreateRect".into(),
            syntax: "rodCreateRect(...)".into(),
            description: "Create a rectangle in the layout editor".into(),
            source_file: Some("rod.fnd".into()),
        },
        SkillEntry {
            name: "schOpen".into(),
            syntax: "schOpen()".into(),
            description: "Opens a cellView in the schematic editor".into(),
            source_file: Some("sch.fnd".into()),
        },
        SkillEntry {
            name: "dbSave".into(),
            syntax: "dbSave(cv)".into(),
            description: "Save a cellView to disk".into(),
            source_file: Some("db.fnd".into()),
        },
        SkillEntry {
            name: "dbCloseCellViewByType".into(),
            syntax: "dbCloseCellViewByType(cv)".into(),
            description: "Close the cellView window".into(),
            source_file: Some("db.fnd".into()),
        },
    ])
}

fn names(rs: &[&SkillEntry]) -> Vec<String> {
    let mut v: Vec<String> = rs.iter().map(|e| e.name.clone()).collect();
    v.sort();
    v
}

#[test]
fn default_search_does_not_match_description_only() {
    let f = corpus();
    let r = f.search("layout", SearchMode::Fuzzy, 50, false);
    assert!(
        r.is_empty(),
        "default fuzzy search should NOT match description-only; got {:?}",
        names(&r)
    );
}

#[test]
fn include_desc_fuzzy_matches_description() {
    let f = corpus();
    let r = f.search("layout", SearchMode::Fuzzy, 50, true);
    assert_eq!(names(&r), vec!["rodCreateRect".to_string()]);
}

#[test]
fn include_desc_fuzzy_still_matches_names() {
    let f = corpus();
    let r = f.search("Open", SearchMode::Fuzzy, 50, true);
    let got = names(&r);
    assert!(got.contains(&"dbOpenCellView".to_string()));
    assert!(got.contains(&"schOpen".to_string()));
}

#[test]
fn include_desc_fuzzy_is_case_insensitive() {
    let f = corpus();
    let r = f.search("LAYOUT", SearchMode::Fuzzy, 50, true);
    let got = names(&r);
    assert_eq!(got, vec!["rodCreateRect".to_string()]);
}

#[test]
fn include_desc_prefix_still_filters_by_name_shape() {
    let f = corpus();
    let r = f.search("db", SearchMode::Prefix, 50, true);
    let got = names(&r);
    assert!(got.contains(&"dbOpenCellView".to_string()));
    assert!(got.contains(&"dbSave".to_string()));
    assert!(!got.contains(&"rodCreateRect".to_string()));

    let r = f.search("rectangle", SearchMode::Prefix, 50, true);
    assert!(r.is_empty());
}

#[test]
fn include_desc_suffix_still_filters_by_name_shape() {
    let f = corpus();
    let r = f.search("Open", SearchMode::Suffix, 50, true);
    let got = names(&r);
    assert_eq!(got, vec!["schOpen".to_string()]);
}

#[test]
fn include_desc_exact_still_uses_name_only() {
    let f = corpus();
    let r = f.search("dbOpenCellView", SearchMode::Exact, 50, true);
    assert_eq!(names(&r), vec!["dbOpenCellView".to_string()]);

    let r = f.search("Opens", SearchMode::Exact, 50, true);
    assert!(r.is_empty());
}

#[test]
fn include_desc_regex_matches_description() {
    let f = corpus();
    let r = f.search(r"layout\s+editor", SearchMode::Regex, 50, true);
    let got = names(&r);
    assert!(got.contains(&"rodCreateRect".to_string()));

    let r = f.search(r"schematic\s+editor", SearchMode::Regex, 50, true);
    let got = names(&r);
    assert!(got.contains(&"schOpen".to_string()));
    assert!(!got.contains(&"rodCreateRect".to_string()));
}

#[test]
fn include_desc_limit_is_respected() {
    let f = corpus();
    let r = f.search("cellView", SearchMode::Fuzzy, 2, true);
    assert_eq!(
        r.len(),
        2,
        "limit=2 must be respected even when 4 entries match"
    );

    let r = f.search("cellView", SearchMode::Fuzzy, 0, true);
    assert_eq!(r.len(), 0, "limit=0 should return zero entries");
}

#[test]
fn unloaded_finder_returns_empty_with_desc() {
    let f = SKILLFinder::new();
    let r = f.search("y", SearchMode::Fuzzy, 50, true);
    assert!(r.is_empty(), "unloaded finder must not return results");
}

#[test]
fn empty_query_with_include_desc_matches_everything_with_substring() {
    let f = corpus();
    let r = f.search("", SearchMode::Fuzzy, 50, true);
    assert_eq!(r.len(), 5);
}
