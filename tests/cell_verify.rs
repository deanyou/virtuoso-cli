//! Integration tests for cell verify-native grid audit parser.
//!
//! Exercises the shape-grid-audit parser against fixture SKILL outputs.

use virtuoso_cli::client::cell_ops as ops;

// Re-export the parser logic from cell.rs for testing.
// We test the SKILL builders directly (unit), and verify the parsing
// logic with fixture inputs (integration-style).

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
    // Must open with "r" mode
    assert!(s.contains("\"r\""), "must open read-only: {s}");
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
    // Must detect VDD*/VSS*/VPWR*/VGND* nets for decorative tail detection
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
    assert!(s.contains("\"r\""), "{s}");
}

// Tests for the parsing logic — we re-implement parse_grid_audit_output
// as a local function so we can test it without depending on internal modules.

/// Parse the shape-grid-audit SKILL output (reimplementation for testing).
///
/// Mirrors the logic in commands/cell.rs::parse_grid_audit_output.
type TestLayer = (String, u32, u32);
type TestTail = (String, u32);

fn parse_grid_audit_output(raw: &str) -> (Vec<TestLayer>, Vec<TestTail>) {
    let parsed = match virtuoso_cli::client::skill_sexp::parse_sexp(raw) {
        Ok(v) => v,
        Err(_) => return (Vec::new(), Vec::new()),
    };

    use virtuoso_cli::client::skill_sexp::SexpVal;
    let mut layers = Vec::new();
    let mut tails = Vec::new();

    if let SexpVal::List(items) = parsed {
        for item in items {
            if let SexpVal::List(ref sub) = item {
                if sub.len() >= 2 {
                    // Check for "tails" marker FIRST.
                    if let SexpVal::Atom(ref a) = sub[0] {
                        if a == "tails" {
                            if let SexpVal::List(tail_items) = &sub[1] {
                                for tail_item in tail_items {
                                    if let SexpVal::List(tp) = tail_item {
                                        if tp.len() >= 2 {
                                            let net = tp[0].as_str().unwrap_or("").to_string();
                                            let count = tp[1]
                                                .as_str()
                                                .and_then(|s| s.parse::<u32>().ok())
                                                .unwrap_or(1);
                                            tails.push((net, count));
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                    }

                    if sub.len() >= 3 {
                        // Layer with off-grid count: (layerName onCount offCount)
                        let layer = sub[0].as_str().unwrap_or("").to_string();
                        let on = sub[1]
                            .as_str()
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        let off = sub[2]
                            .as_str()
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        layers.push((layer, on, off));
                    } else {
                        // Flat form: (layerName count) — only 2 elements, off=0
                        let layer = sub[0].as_str().unwrap_or("").to_string();
                        let on = sub[1]
                            .as_str()
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0);
                        layers.push((layer, on, 0));
                    }
                }
            }
        }
    }

    // Sort by off_grid count descending
    layers.sort_by_key(|l| std::cmp::Reverse(l.2));
    (layers, tails)
}

#[test]
fn parse_empty_cell_zero_shapes() {
    let raw = "(0 0)";
    let (layers, tails) = parse_grid_audit_output(raw);
    assert!(layers.is_empty(), "empty cell should have no layers");
    assert!(tails.is_empty(), "empty cell should have no tails");
}

#[test]
fn parse_mixed_layout() {
    // SKILL output with multiple layers
    let raw = "((metal1 12 0) (metal2 8 1) (poly 20 0))";
    let (layers, tails) = parse_grid_audit_output(raw);
    assert_eq!(layers.len(), 3);
    // Sorted by off_grid desc: metal2 (1 off) before metal1 (0) before poly (0)
    assert_eq!(layers[0].0, "metal2");
    assert_eq!(layers[0].1, 8); // on
    assert_eq!(layers[0].2, 1); // off
    assert_eq!(layers[1].0, "metal1");
    assert_eq!(layers[1].1, 12);
    assert_eq!(layers[1].2, 0);
    assert!(tails.is_empty());
}

#[test]
fn parse_with_tails() {
    // SKILL foreach increments by 1 per decorative tail shape
    let raw = "((metal1 5 0) (tails ((VDD 1) (VSS 1))))";
    let (layers, tails) = parse_grid_audit_output(raw);
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].0, "metal1");
    assert_eq!(tails.len(), 2);
    assert_eq!(tails[0].0, "VDD");
    assert_eq!(tails[0].1, 1); // foreach increments by 1 per shape
    assert_eq!(tails[1].0, "VSS");
    assert_eq!(tails[1].1, 1);
}

#[test]
fn parse_missing_tails_key() {
    // Real cells may not have any tails section at all
    let raw = "((poly 10 0) (metal1 20 1))";
    let (layers, tails) = parse_grid_audit_output(raw);
    assert_eq!(layers.len(), 2);
    assert!(tails.is_empty(), "missing tails should give empty vec");
}

#[test]
fn parse_tails_nil() {
    // tails marker present but content is nil
    let raw = "((metal1 5 0) (tails nil))";
    let (layers, tails) = parse_grid_audit_output(raw);
    assert_eq!(layers.len(), 1);
    assert!(tails.is_empty());
}

#[test]
fn parse_sorts_by_off_grid_desc() {
    // metal3 has 5 off-grid shapes, metal1 has 1
    let raw = "((metal1 10 1) (metal3 8 5) (metal2 15 2))";
    let (layers, _tails) = parse_grid_audit_output(raw);
    assert_eq!(layers.len(), 3);
    assert_eq!(layers[0].0, "metal3"); // 5 off
    assert_eq!(layers[0].2, 5);
    assert_eq!(layers[1].0, "metal2"); // 2 off
    assert_eq!(layers[2].0, "metal1"); // 1 off
}

#[test]
fn parse_decorative_tail_detection() {
    // SKILL foreach increments by 1 per decorative tail shape
    let raw = "((metal1 5 0) (tails ((VDD 2) (VPWR 1) (VSS 3) (net_regular 0))))";
    let (_layers, tails) = parse_grid_audit_output(raw);
    // Should include VDD, VPWR, VSS
    let vdd_count = tails.iter().find(|(n, _)| n == "VDD").map(|(_, c)| *c);
    let vss_count = tails.iter().find(|(n, _)| n == "VSS").map(|(_, c)| *c);
    let vpwr_count = tails.iter().find(|(n, _)| n == "VPWR").map(|(_, c)| *c);
    assert_eq!(vdd_count, Some(2), "VDD tails should be counted");
    assert_eq!(vss_count, Some(3), "VSS tails should be counted");
    assert_eq!(vpwr_count, Some(1), "VPWR tails should be counted");
}

#[test]
fn parse_flat_list_form() {
    // Some tools emit flat lists: (layer1 12 0 layer2 8 1 ...)
    // Our parser handles this via the 2-element pair case
    let raw = "((metal1 12) (metal2 8))";
    let (layers, tails) = parse_grid_audit_output(raw);
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].0, "metal1");
    assert_eq!(layers[0].1, 12);
    assert_eq!(layers[0].2, 0); // flat form → off=0
    assert!(tails.is_empty());
}
