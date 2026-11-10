//! Integration tests for `vcli cell verify-native`.
//!
//! These tests exercise the REAL production pieces on both ends of the
//! format:
//!
//! * the SKILL builder in `virtuoso_cli::client::cell_ops`, and
//! * the parser in `virtuoso_cli::commands::cell`.
//!
//! The fixtures below are not invented: `SKILL_OUTPUT_QVP20` is the verbatim
//! value the audit SKILL returned from a live Virtuoso bridge
//! (`st02/qvp20/layout`, 497 shapes, grid 0.005). The previous version of
//! this file tested the parser against hand-written tuples such as
//! `((M1 100 3))` that the SKILL never emitted, which is exactly why a
//! SKILL/parser format mismatch could not be caught.
//!
//! `SKILL_OUTPUT_MP_WITH_TAILS` is the *live* output of the same SKILL
//! against `st02/mp/layout` with its `tails` section replaced by a hand-built
//! one. It is a synthetic fixture, and it is labelled as such: no layout
//! cellview reachable from the test host carries a `VDD*`/`VSS*`/`VPWR*`/
//! `VGND*` net, so the tails branch could not be exercised end-to-end
//! against the real tool. The two halves it depends on were each verified
//! against the live bridge instead — the supply-net prefix test
//! (`substring` + `strcmp`, because `rexMatchp` has no alternation) and the
//! free-end test (`dbGetOverlaps(cv list(pt pt))`, excluding the shape
//! itself).

use virtuoso_cli::client::cell_ops as ops;
use virtuoso_cli::commands::cell::grid_audit_output_is_well_formed;
use virtuoso_cli::{parse_grid_audit_output, GridLayer, GridTail};

/// Verbatim output of `shape_grid_audit_skill("st02", "qvp20", "layout")`
/// against a live bridge. 497 shapes over six layers, none off-grid, and no
/// supply rail in this cellview.
const SKILL_OUTPUT_QVP20: &str =
    r#"(("TO" 3 0) ("SP" 2 0) ("A1" 3 0) ("BJT" 1 0) ("W1" 487 0) ("TB" 1 0) ("tails" nil))"#;

/// Live output of the same SKILL against `st02/mp/layout` — except that its
/// `tails` section has been replaced by a hand-built one, because no layout
/// cellview reachable from the test host carries a supply net. The layer
/// entries are the real ones; the tails entry is synthetic.
const SKILL_OUTPUT_MP_WITH_TAILS: &str =
    r#"(("TO" 3 0) ("SP" 1 0) ("TB" 1 0) ("W1" 2 0) ("A1" 2 0) ("GT" 5 0) ("tails" (("B" 1))))"#;

/// The same SKILL's live output against `st02/mp/layout`, with the tails
/// section exactly as the tool returned it: no supply net, so no tails.
const SKILL_OUTPUT_MP_NO_TAILS: &str =
    r#"(("TO" 3 0) ("SP" 1 0) ("TB" 1 0) ("W1" 2 0) ("A1" 2 0) ("GT" 5 0) ("tails" nil))"#;

/// Count the parentheses of a SKILL string, ignoring the ones inside
/// double-quoted SKILL strings.
fn paren_balance(skill: &str) -> i32 {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut chars = skill.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_string => {
                chars.next();
            }
            '"' => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string => depth -= 1,
            _ => {}
        }
    }
    depth
}

// --- the SKILL builder -----------------------------------------------------

#[test]
fn tech_grid_precision_skill_reads_the_mfg_grid() {
    let s = ops::tech_grid_precision_skill("myLib");
    assert!(s.contains("techGetMfgGridResolution"), "{s}");
    assert!(s.contains("techGetTechFile(ddGetObj("), "{s}");
    // There is no "user grid precision" in the OpenAccess API.
    assert!(!s.contains("userGridPrecision"), "{s}");
    assert!(!s.contains("axlGetTech"), "{s}");
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
fn shape_grid_audit_opens_the_cellview_with_a_view_type() {
    // The four-argument form of dbOpenCellViewByType returns a WINDOW id,
    // not a cellview, and every accessor on a window id reads nil — so the
    // audit would report "no drift" for every cellview.
    let s = ops::shape_grid_audit_skill("st02", "qvp20", "layout");
    assert!(
        s.contains(r#"dbOpenCellViewByType("st02" "qvp20" "layout" "maskLayout" "r")"#),
        "{s}"
    );
    assert!(!s.contains("axlOpenCellViewByMode"), "{s}");
}

#[test]
fn shape_grid_audit_reads_the_real_grid() {
    let s = ops::shape_grid_audit_skill("lib", "cell", "view");
    assert!(s.contains("techGetMfgGridResolution"), "{s}");
    assert!(s.contains("techGetTechFile(ddGetObj("), "{s}");
    assert!(!s.contains("userGridPrecision"), "{s}");
}

#[test]
fn shape_grid_audit_uses_real_shape_accessors() {
    let s = ops::shape_grid_audit_skill("lib", "cell", "view");
    // layer name, bounding box, vertices
    assert!(s.contains("car(shp~>lpp)"), "{s}");
    assert!(s.contains("caar(bbox)"), "{s}");
    assert!(s.contains("cadadr(bbox)"), "{s}");
    assert!(s.contains("shp~>points"), "{s}");
    // accessors that do not exist / cannot evaluate
    assert!(!s.contains("~>layer~>name"), "{s}");
    assert!(!s.contains("~>x1"), "{s}");
    assert!(!s.contains("~>kind"), "{s}");
    assert!(!s.contains("~>~>points"), "{s}");
}

#[test]
fn shape_grid_audit_closes_the_cellview_on_every_path() {
    // The close is the cleanup form of unwindProtect, so it also runs when
    // the audit raises — a bare trailing dbClose would leak the cellview.
    let s = ops::shape_grid_audit_skill("lib", "cell", "view");
    assert!(s.contains("dbClose(cv)"), "must close cellview: {s}");
    assert!(s.contains("unwindProtect("), "{s}");
}

#[test]
fn shape_grid_audit_parentheses_are_balanced() {
    let s = ops::shape_grid_audit_skill("st02", "qvp20", "layout");
    assert_eq!(paren_balance(&s), 0, "unbalanced SKILL parens: {s}");
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
    assert!(s.contains("dbClose(cv)"), "must close cellview: {s}");
}

// --- the SKILL's output format and the parser, pinned together ------------

#[test]
fn skill_output_format_and_parser_agree() {
    let skill = ops::shape_grid_audit_skill("st02", "qvp20", "layout");

    // The SKILL builds one (layer on off) entry per LAYER and appends the
    // tails section. These are the fragments that decide the format the
    // parser has to read.
    assert!(
        skill.contains("cons(list(lyr on off) newEntries)"),
        "per-layer entry: {skill}"
    );
    assert!(
        skill.contains("(cadr(ent) + on)") && skill.contains("(caddr(ent) + off)"),
        "per-layer counts: {skill}"
    );
    assert!(
        skill.contains(&format!(r#"list("{}" tailEntries)"#, ops::TAILS_KEY)),
        "tails section: {skill}"
    );

    // ... and the parser reads exactly that: a three-element layer tuple
    // and the tails marker.
    let (layers, tails) = parse_grid_audit_output(SKILL_OUTPUT_QVP20);
    assert_eq!(
        layers,
        vec![
            GridLayer("TO".into(), 3, 0),
            GridLayer("SP".into(), 2, 0),
            GridLayer("A1".into(), 3, 0),
            GridLayer("BJT".into(), 1, 0),
            GridLayer("W1".into(), 487, 0),
            GridLayer("TB".into(), 1, 0),
        ]
    );
    assert!(tails.is_empty());
}

#[test]
fn parser_reads_the_live_skill_output() {
    let (layers, tails) = parse_grid_audit_output(SKILL_OUTPUT_QVP20);
    assert_eq!(layers.len(), 6);
    // 497 shapes in the cellview, every one of them on-grid.
    let total: u32 = layers.iter().map(|l| l.1 + l.2).sum();
    assert_eq!(total, 497);
    assert_eq!(layers.iter().map(|l| l.2).sum::<u32>(), 0);
    assert!(tails.is_empty());
    assert!(grid_audit_output_is_well_formed(SKILL_OUTPUT_QVP20));
}

#[test]
fn parser_reads_a_result_carrying_a_tails_section() {
    let (layers, tails) = parse_grid_audit_output(SKILL_OUTPUT_MP_WITH_TAILS);
    assert_eq!(
        layers,
        vec![
            GridLayer("TO".into(), 3, 0),
            GridLayer("SP".into(), 1, 0),
            GridLayer("TB".into(), 1, 0),
            GridLayer("W1".into(), 2, 0),
            GridLayer("A1".into(), 2, 0),
            GridLayer("GT".into(), 5, 0),
        ]
    );
    assert_eq!(tails, vec![GridTail("B".into(), 1)]);
    assert!(grid_audit_output_is_well_formed(SKILL_OUTPUT_MP_WITH_TAILS));
}

#[test]
fn parser_reads_the_live_result_with_no_tails() {
    // The live output of the same cellview: it has no supply net, so the
    // tails section is present and empty. An empty section must not be
    // mistaken for a layer, and must not make the audit look like it found
    // something.
    let (layers, tails) = parse_grid_audit_output(SKILL_OUTPUT_MP_NO_TAILS);
    assert_eq!(layers.len(), 6);
    assert_eq!(layers.iter().map(|l| l.1 + l.2).sum::<u32>(), 14);
    assert!(tails.is_empty());
    assert!(grid_audit_output_is_well_formed(SKILL_OUTPUT_MP_NO_TAILS));
}

#[test]
fn tails_section_is_not_mistaken_for_a_layer() {
    // The tails marker is emitted as a quoted SKILL string, so the parser
    // has to match on the string value. When it matched on the atom type
    // instead, the tails entry was read as a layer called "tails".
    let (layers, tails) = parse_grid_audit_output(SKILL_OUTPUT_MP_WITH_TAILS);
    assert!(
        !layers.iter().any(|l| l.0 == ops::TAILS_KEY),
        "tails must not be reported as a layer: {layers:?}"
    );
    assert_eq!(tails.len(), 1);
}

#[test]
fn off_grid_counts_are_per_layer_and_reach_the_totals() {
    // The drift verdict is the sum of the off column, so the counts have to
    // be per layer and not per shape.
    let raw = r#"(("M1" 100 3) ("M2" 100 200) ("M3" 100 20))"#;
    let (layers, _) = parse_grid_audit_output(raw);
    assert_eq!(layers.len(), 3);
    // sorted by off_grid descending
    assert_eq!(layers[0], GridLayer("M2".into(), 100, 200));
    assert_eq!(layers[1], GridLayer("M3".into(), 100, 20));
    assert_eq!(layers[2], GridLayer("M1".into(), 100, 3));
    assert_eq!(layers.iter().map(|l| l.2).sum::<u32>(), 223);
}

#[test]
fn empty_cellview_is_well_formed_and_reports_nothing() {
    // A cellview with no shapes still produces a well-formed result: the
    // SKILL always emits the tails section.
    let raw = r#"(("tails" nil))"#;
    assert!(grid_audit_output_is_well_formed(raw));
    let (layers, tails) = parse_grid_audit_output(raw);
    assert!(layers.is_empty());
    assert!(tails.is_empty());
}

#[test]
fn parser_rejects_an_unreadable_result() {
    // Not a list at all.
    assert!(!grid_audit_output_is_well_formed("not a sexp"));
    // A layer entry that is not the three-element (layer on off) tuple.
    assert!(!grid_audit_output_is_well_formed(r#"(("M1" 100))"#));
    // A count that is not a number.
    assert!(!grid_audit_output_is_well_formed(r#"(("M1" "foo" "bar"))"#));
    // Garbage.
    let (layers, tails) = parse_grid_audit_output("(this is not s-expr at all");
    assert!(layers.is_empty());
    assert!(tails.is_empty());
}

#[test]
fn parser_reads_a_float_count() {
    // SKILL can print a count that went through arithmetic as "3.0".
    let (layers, _) = parse_grid_audit_output(r#"(("M1" 100 3.0))"#);
    assert_eq!(layers, vec![GridLayer("M1".into(), 100, 3)]);
}
