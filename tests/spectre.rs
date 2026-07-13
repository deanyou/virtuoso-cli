//! Integration tests for Spectre netlist and SKILL parsing.
//!
//! Tests ocean module functions that don't require a live Virtuoso connection.

use std::collections::HashMap;
use virtuoso_cli::ocean::corner::{AnalysisConfig, Corner, CornerConfig, DesignTarget, Measure};
use virtuoso_cli::ocean::{
    analysis_skill_simple, corner_skill, parse_skill_list, setup_skill, sweep_skill,
};

/// Test setup_skill generates valid SKILL code.
#[test]
fn test_setup_skill_generates_progn() {
    let skill = setup_skill("myLib", "myCell", "schematic", "spectre");

    // Should use progn to wrap multiple expressions
    assert!(skill.starts_with("progn("));
    // Should set simulator
    assert!(skill.contains("spectre"));
    // Should call design with escaped library/cell/view
    assert!(skill.contains(r#"design("myLib" "myCell" "schematic")"#));
    // Should include resultsDir
    assert!(skill.contains("resultsDir()"));
}

/// Test setup_skill escapes special characters.
#[test]
fn test_setup_skill_escaping() {
    let skill = setup_skill(r#"lib"with"quotes"#, "normal", "schematic", "spectre");

    // Quotes should be escaped in the lib name
    assert!(skill.contains(r#"\"#));
}

/// Test analysis_skill_simple with numeric parameters.
#[test]
fn test_analysis_skill_simple_numbers() {
    let mut params = HashMap::new();
    params.insert("start".to_string(), "1e-9".to_string());
    params.insert("stop".to_string(), "1e-3".to_string());
    params.insert("step".to_string(), "1e-6".to_string());

    let skill = analysis_skill_simple("tran", &params);

    assert!(skill.starts_with("analysis('tran"));
    assert!(skill.contains("?start 1e-9"));
    assert!(skill.contains("?stop 1e-3"));
    assert!(skill.contains("?step 1e-6"));
}

/// Test analysis_skill_simple with boolean t/nil.
#[test]
fn test_analysis_skill_simple_booleans() {
    let mut params = HashMap::new();
    params.insert("conservative".to_string(), "t".to_string());
    params.insert("disable".to_string(), "nil".to_string());

    let skill = analysis_skill_simple("tran", &params);

    // Booleans should NOT be quoted
    assert!(skill.contains("?conservative t"));
    assert!(skill.contains("?disable nil"));
}

/// Test analysis_skill_simple with string parameters.
#[test]
fn test_analysis_skill_simple_strings() {
    let mut params = HashMap::new();
    params.insert("errpreset".to_string(), "moderate".to_string());
    params.insert("engine".to_string(), "spectre".to_string());

    let skill = analysis_skill_simple("tran", &params);

    // Strings should be quoted
    assert!(skill.contains(r#"?errpreset "moderate""#));
    assert!(skill.contains(r#"?engine "spectre""#));
}

/// Test sweep_skill generates sweep loop.
#[test]
fn test_sweep_skill_structure() {
    let values = vec![0.8, 0.9, 1.0, 1.1];
    let exprs = vec!["VT(\"M1\")".to_string()];
    let skill = sweep_skill("VDD", &values, "dc", &exprs);

    // Should use let for results
    assert!(skill.contains("let((results)"));
    // Should use foreach with value list
    assert!(skill.contains("foreach(val '("));
    // Should use desVar to set sweep variable
    assert!(skill.contains(r#"desVar("VDD" val)"#));
    // Should call run and selectResult
    assert!(skill.contains("run()"));
    assert!(skill.contains("selectResult('dc)"));
    // Should collect results and reverse
    assert!(skill.contains("reverse(results)"));
}

/// Test sweep_skill with multiple measure expressions.
#[test]
fn test_sweep_skill_multiple_measures() {
    let values = vec![1.0, 2.0];
    let exprs = vec![
        "VT(\"M1\" \"vgs\")".to_string(),
        "VT(\"M2\" \"vds\")".to_string(),
        "ID(\"I1\")".to_string(),
    ];
    let skill = sweep_skill("Vin", &values, "dc", &exprs);

    // All expressions should appear
    assert!(skill.contains("VT(\"M1\" \"vgs\")"));
    assert!(skill.contains("VT(\"M2\" \"vds\")"));
    assert!(skill.contains("ID(\"I1\")"));
}

/// Test parse_skill_list with nested list.
#[test]
fn test_parse_skill_list_nested() {
    let input = "((1.0 2.0 3.0) (4.0 5.0 6.0) (7.0 8.0 9.0))";
    let result = parse_skill_list(input);

    assert_eq!(result.len(), 3);
    assert_eq!(result[0], vec!["1.0", "2.0", "3.0"]);
    assert_eq!(result[1], vec!["4.0", "5.0", "6.0"]);
    assert_eq!(result[2], vec!["7.0", "8.0", "9.0"]);
}

/// Test parse_skill_list with single value.
#[test]
fn test_parse_skill_list_single() {
    let result = parse_skill_list("42");
    assert_eq!(result, vec![vec!["42"]]);
}

/// Test parse_skill_list with nil (empty).
#[test]
fn test_parse_skill_list_nil() {
    let result = parse_skill_list("nil");
    assert!(result.is_empty());

    let result = parse_skill_list("");
    assert!(result.is_empty());
}

/// Test parse_skill_list with quoted strings.
#[test]
fn test_parse_skill_list_quoted() {
    let input = r#"(( "cell1" "view1") ("cell2" "view2"))"#;
    let result = parse_skill_list(input);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0], vec!["cell1", "view1"]);
    assert_eq!(result[1], vec!["cell2", "view2"]);
}

/// Test parse_skill_list with flat list.
#[test]
fn test_parse_skill_list_flat() {
    let input = "(1.0 2.0 3.0)";
    let result = parse_skill_list(input);

    // Flat list becomes single row
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], vec!["1.0", "2.0", "3.0"]);
}

/// Test parse_skill_list with mixed numbers and strings.
#[test]
fn test_parse_skill_list_mixed() {
    let input = r#"((1.0 "TT") (2.0 "FF") (3.0 "SS"))"#;
    let result = parse_skill_list(input);

    assert_eq!(result.len(), 3);
    assert_eq!(result[0][0], "1.0");
    assert_eq!(result[0][1], "TT");
}

/// Test corner_skill generates valid multi-corner SKILL.
#[test]
fn test_corner_skill_structure() {
    let config = CornerConfig {
        simulator: Some("spectre".to_string()),
        design: DesignTarget {
            lib: "myLib".to_string(),
            cell: "myCell".to_string(),
            view: "schematic".to_string(),
        },
        model_file: "models.so".to_string(),
        analysis: AnalysisConfig {
            analysis_type: "tran".to_string(),
            params: HashMap::new(),
        },
        corners: vec![Corner {
            name: "tt".to_string(),
            section: "tt".to_string(),
            temp: 27.0, // Use 27.0 so it outputs "27" not "-40"
            vars: HashMap::new(),
        }],
        measures: vec![Measure {
            name: "vgs".to_string(),
            expr: "VT(\"M1\")".to_string(),
        }],
    };

    let skill = corner_skill(&config);

    // Should call simulator and design
    assert!(skill.contains("simulator('spectre)"));
    assert!(skill.contains(r#"design("myLib" "myCell" "schematic")"#));
    // Should include analysis
    assert!(skill.contains("analysis('tran"));
    // Should include modelFile
    assert!(skill.contains("modelFile"));
    // Should include temp (format: temp(27))
    assert!(skill.contains("temp("));
    assert!(skill.contains("27"));
    // Should call run
    assert!(skill.contains("run()"));
    // Should collect results
    assert!(skill.contains("reverse(results)"));
}
