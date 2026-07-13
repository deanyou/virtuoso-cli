//! Integration tests for Ocean module SKILL generation.
//!
//! Tests the ocean module's SKILL generation functions.

use std::collections::HashMap;
use virtuoso_cli::ocean::corner::{AnalysisConfig, Corner, CornerConfig, DesignTarget, Measure};
use virtuoso_cli::ocean::{analysis_skill_simple, parse_skill_list, setup_skill, sweep_skill};

/// Test setup_skill generates valid SKILL code.
#[test]
fn test_setup_skill_generates_progn() {
    let skill = setup_skill("myLib", "myCell", "schematic", "spectre");

    // Should use progn to wrap multiple expressions
    assert!(skill.starts_with("progn("));
    // Should set simulator
    assert!(skill.contains("spectre"));
    // Should call design
    assert!(skill.contains(r#"design("myLib" "myCell" "schematic")"#));
    // Should include resultsDir
    assert!(skill.contains("resultsDir()"));
}

/// Test setup_skill escapes double quotes.
#[test]
fn test_setup_skill_escapes_quotes() {
    let skill = setup_skill(r#"lib"with"quotes"#, "cell", "schematic", "spectre");

    // Quotes should be escaped
    assert!(skill.contains(r#"\"#));
}

/// Test analysis_skill_simple with numeric parameters.
#[test]
fn test_analysis_skill_simple_numbers() {
    let mut params = HashMap::new();
    params.insert("start".to_string(), "1e-9".to_string());
    params.insert("stop".to_string(), "1e-3".to_string());

    let skill = analysis_skill_simple("tran", &params);

    assert!(skill.starts_with("analysis('tran"));
    assert!(skill.contains("?start 1e-9"));
    assert!(skill.contains("?stop 1e-3"));
}

/// Test analysis_skill_simple with boolean t/nil (not quoted).
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

/// Test analysis_skill_simple with string parameters (quoted).
#[test]
fn test_analysis_skill_simple_strings() {
    let mut params = HashMap::new();
    params.insert("errpreset".to_string(), "moderate".to_string());

    let skill = analysis_skill_simple("tran", &params);

    // Strings should be quoted
    assert!(skill.contains(r#"?errpreset "moderate""#));
}

/// Test sweep_skill generates sweep loop with desVar.
#[test]
fn test_sweep_skill_desvar() {
    let values = vec![0.8, 0.9, 1.0];
    let exprs = vec!["VT(\"M1\")".to_string()];
    let skill = sweep_skill("VDD", &values, "dc", &exprs);

    assert!(skill.contains("let((results)"));
    assert!(skill.contains("foreach(val '("));
    assert!(skill.contains(r#"desVar("VDD" val)"#));
    assert!(skill.contains("run()"));
    assert!(skill.contains("selectResult('dc)"));
    assert!(skill.contains("reverse(results)"));
}

/// Test parse_skill_list with nested list.
#[test]
fn test_parse_skill_list_nested() {
    let input = "((1.0 2.0) (3.0 4.0))";
    let result = parse_skill_list(input);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0], vec!["1.0", "2.0"]);
    assert_eq!(result[1], vec!["3.0", "4.0"]);
}

/// Test parse_skill_list with single value.
#[test]
fn test_parse_skill_list_single_value() {
    let result = parse_skill_list("42");
    assert_eq!(result, vec![vec!["42"]]);
}

/// Test parse_skill_list with nil returns empty.
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
}

/// Test CornerConfig deserialization.
#[test]
fn test_corner_config_structure() {
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
            temp: 27.0,
            vars: HashMap::new(),
        }],
        measures: vec![Measure {
            name: "delay".to_string(),
            expr: "VT(\"out\" 1e-6)".to_string(),
        }],
    };

    assert_eq!(config.design.lib, "myLib");
    assert_eq!(config.corners.len(), 1);
    assert_eq!(config.measures.len(), 1);
}
