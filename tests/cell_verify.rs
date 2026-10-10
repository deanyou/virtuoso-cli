//! Integration tests for `vcli cell verify-native` grid audit parser.
//!
//! Round 2 (M4): the previous test file shipped a clone of
//! `parse_grid_audit_output` ("Mirrors the logic in commands/cell.rs") and
//! only tested that clone. The real parser could change without breaking
//! tests. These tests now go through the lib re-export
//! `virtuoso_cli::parse_grid_audit_output`, which IS the production code.

use virtuoso_cli::client::cell_ops as ops;
use virtuoso_cli::{parse_grid_audit_output, GridLayer, GridTail};

#[test]
fn tech_grid_precision_skill_reads_user_grid_precision() {
    let s = ops::tech_grid_precision_skill("myLib");
    assert!(s.contains("userGridPrecision"), "{s}");
}

#[test]
fn tech_grid_precision_skill_escapes_quotes() {
    let s = ops::tech_grid_precision_skill(r#"PDK"42"#);
    assert!(s.contains(r#"PDK\"42"#), "{s}");
}

#[test]
fn shape_grid_audit_is_read_only() {
    let s = ops::shape_grid_audit_skill("lib", "cell", "layout");
    assert!(s.contains(r#""r""#), "must open read-only: {s}");
}

#[test]
fn shape_grid_audit_checks_b_box() {
    let s = ops::shape_grid_audit_skill("lib", "cell", "view");
    assert!(s.contains("bBox"), "{s}");
}

#[test]
fn shape_grid_audit_checks_grid_precision() {
    let s = ops::shape_grid_audit_skill("lib", "cell", "view");
    assert!(s.contains("userGridPrecision"), "{s}");
}

#[test]
fn shape_grid_audit_checks_vdd_vss_nets() {
    let s = ops::shape_grid_audit_skill("lib", "cell", "view");
    assert!(
        s.contains("VDD") && s.contains("VSS"),
        "must check VDD/VSS nets: {s}"
    );
}

#[test]
fn cell_info_escapes() {
    let s = ops::cell_info_skill("lib", r#"cell"name"#, "schematic");
    assert!(s.contains(r#"cell\"name"#), "{s}");
}

#[test]
fn cell_info_opens_read_only() {
    let s = ops::cell_info_skill("lib", "cell", "view");
    assert!(s.contains(r#""r""#), "{s}");
}

#[test]
fn shape_grid_audit_skill_closes_cellview_in_dusk() {
    // m6: SKILL must call dbClose(cv) at the end so successive verify-native
    // calls don't accumulate open views in the CIW.
    let s = ops::shape_grid_audit_skill("lib", "cell", "view");
    assert!(s.contains("dbClose(cv)"), "must close cellview: {s}");
}

// --- parse_grid_audit_output: REAL production parser --------------------

#[test]
fn parser_empty_input_returns_empty() {
    let (layers, tails) = parse_grid_audit_output("");
    assert!(layers.is_empty(), "empty input yields no layers");
    assert!(tails.is_empty(), "empty input yields no tails");
}

#[test]
fn parser_single_layer_three_tuple() {
    // (layerName on off)
    let raw = "((\"M1\" 100 3))";
    let (layers, tails) = parse_grid_audit_output(raw);
    assert_eq!(layers, vec![GridLayer("M1".into(), 100, 3)]);
    assert!(tails.is_empty());
}

#[test]
fn parser_single_layer_two_tuple_flat_form() {
    // (layerName count) — only 2 elements, off=0
    let raw = "((\"M2\" 50))";
    let (layers, tails) = parse_grid_audit_output(raw);
    assert_eq!(layers, vec![GridLayer("M2".into(), 50, 0)]);
    assert!(tails.is_empty());
}

#[test]
fn parser_multiple_layers_sorted_by_off_grid_desc() {
    // M1=10, M2=200, M3=20 — M2 has the most off-grid so it sorts first.
    let raw = "((\"M1\" 100 10) (\"M2\" 100 200) (\"M3\" 100 20))";
    let (layers, _) = parse_grid_audit_output(raw);
    assert_eq!(layers.len(), 3);
    assert_eq!(layers[0], GridLayer("M2".into(), 100, 200));
    assert_eq!(layers[1], GridLayer("M3".into(), 100, 20));
    assert_eq!(layers[2], GridLayer("M1".into(), 100, 10));
}

#[test]
fn parser_tails_marker_extracted() {
    // ((layer ...) (tails (("VDD" 5))))
    let raw = r#"(("M1" 100 3) (tails (("VDD" 5) ("VSS" 2))))"#;
    let (layers, tails) = parse_grid_audit_output(raw);
    assert_eq!(layers, vec![GridLayer("M1".into(), 100, 3)]);
    assert_eq!(
        tails,
        vec![GridTail("VDD".into(), 5), GridTail("VSS".into(), 2)]
    );
}

#[test]
fn parser_tails_count_defaults_to_one_on_parse_failure() {
    // ("VDD" "not-a-number") — count falls back to 1
    let raw = r#"(("M1" 100 0) (tails (("VDD" "not-a-number"))))"#;
    let (_, tails) = parse_grid_audit_output(raw);
    assert_eq!(tails, vec![GridTail("VDD".into(), 1)]);
}

#[test]
fn parser_garbage_returns_empty_gracefully() {
    let (layers, tails) = parse_grid_audit_output("(this is not s-expr at all");
    assert!(layers.is_empty());
    assert!(tails.is_empty());
}

#[test]
fn parser_layer_with_garbage_numbers_falls_back_to_zero() {
    // ("M1" "foo" "bar") — both counts fall back to 0
    let raw = "((\"M1\" \"foo\" \"bar\"))";
    let (layers, _) = parse_grid_audit_output(raw);
    assert_eq!(layers, vec![GridLayer("M1".into(), 0, 0)]);
}
