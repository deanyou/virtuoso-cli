//! Integration tests for Maestro CSV result parsing.
//!
//! Tests the MaestroResult::parse_csv function with real-world CSV formats.

use virtuoso_cli::spectre::maestro_csv::MaestroResult;

/// Test parsing a basic single-point simulation result.
#[test]
fn test_maestro_single_point_ac() {
    let csv = r#"Point,Test,Output,Nominal,Spec,Weight,Pass/Fail
Parameters: VDD=0.9, VIN=0.45
1,ac_test,Gain_dB,21.63,,,
1,ac_test,Phase_deg,-135,,,passed
1,ac_test,GBW_Hz,1.2G,,,passed"#;

    let result = MaestroResult::parse_csv(csv, "Interactive.1");

    assert_eq!(result.history, "Interactive.1");
    assert_eq!(result.tests, vec!["ac_test"]);
    assert_eq!(result.num_points(), 1);
    assert!(result.all_pass());

    let point = &result.points[0];
    assert_eq!(point.parameters.get("VDD"), Some(&"0.9".to_string()));
    assert_eq!(point.parameters.get("VIN"), Some(&"0.45".to_string()));
    assert_eq!(point.outputs.get("Gain_dB").unwrap().value, "21.63");
    assert_eq!(result.get_scalar(1, "Gain_dB"), Some(21.63));
}

/// Test parsing multi-point sweep results (VDD variation).
#[test]
fn test_maestro_multi_point_sweep() {
    let csv = r#"Point,Test,Output,Nominal,Spec,Weight,Pass/Fail
Parameters: VDD=0.8
1,dc_test,Ileak_nA,1.2,,,passed
Parameters: VDD=0.9
2,dc_test,Ileak_nA,2.5,,,passed
Parameters: VDD=1.0
3,dc_test,Ileak_nA,5.8,,,passed
Parameters: VDD=1.1
4,dc_test,Ileak_nA,12.1,,,failed"#;

    let result = MaestroResult::parse_csv(csv, "Corner.1");

    assert_eq!(result.num_points(), 4);
    assert_eq!(
        result.points[0].parameters.get("VDD"),
        Some(&"0.8".to_string())
    );
    assert_eq!(
        result.points[3].parameters.get("VDD"),
        Some(&"1.1".to_string())
    );
    assert!(!result.all_pass()); // Last point failed

    // Check raw value (get_scalar won't parse with suffix)
    let value = &result.points[0].outputs.get("Ileak_nA").unwrap().value;
    assert_eq!(value, "1.2");
}

/// Test parsing with spec limits and weights.
#[test]
fn test_maestro_with_specs_and_weights() {
    let csv = r#"Point,Test,Output,Nominal,Spec,Weight,Pass/Fail
1,slew_test,slew_rate,1.8e9,> 1e9,1,passed
1,slew_test,settling_ns,45.2,< 50ns,2,passed
1,power_test,Idd_uA,120.5,< 200uA,1,passed"#;

    let result = MaestroResult::parse_csv(csv, "TT.1");

    let slew = result.points[0].outputs.get("slew_rate").unwrap();
    assert_eq!(slew.spec.as_deref(), Some("> 1e9"));
    assert_eq!(slew.weight.as_deref(), Some("1"));

    let settling = result.points[0].outputs.get("settling_ns").unwrap();
    assert_eq!(settling.weight.as_deref(), Some("2"));
}

/// Test parsing with quoted parameters (Excel/CSV export artifact).
#[test]
fn test_maestro_quoted_parameters() {
    let csv = r#""Parameters: VDD=0.9"
1,test,out,1.0,,,passed"#;

    let result = MaestroResult::parse_csv(csv, "Test.1");

    assert_eq!(
        result.points[0].parameters.get("VDD"),
        Some(&"0.9".to_string())
    );
}

/// Test parsing with scientific notation values.
#[test]
fn test_maestro_scientific_notation() {
    let csv = r#"1,test,cap,1.2e-15,,,
1,test,freq,9.8e9,,,
1,test,delay,45.2e-9,,,"#;

    let result = MaestroResult::parse_csv(csv, "Sci.1");

    assert_eq!(result.get_scalar(1, "cap"), Some(1.2e-15_f64));
    assert_eq!(result.get_scalar(1, "freq"), Some(9.8e9_f64));
    assert_eq!(result.get_scalar(1, "delay"), Some(45.2e-9_f64));
}

/// Test parsing empty CSV returns empty result.
#[test]
fn test_maestro_empty_csv() {
    let result = MaestroResult::parse_csv("", "Empty.1");

    assert!(result.points.is_empty());
    assert!(result.tests.is_empty());
    assert_eq!(result.num_points(), 0);
}

/// Test parsing CSV with only header rows.
#[test]
fn test_maestro_header_only() {
    let csv = r#"Point,Test,Output,Nominal,Spec,Weight,Pass/Fail
,,Parameter,Nominal,,,"#;

    let result = MaestroResult::parse_csv(csv, "HeaderOnly.1");

    assert!(result.points.is_empty());
}

/// Test parsing with multiple tests in same simulation.
#[test]
fn test_maestro_multiple_tests() {
    let csv = r#"Parameters: VDD=0.9
1,opamp_test,DC_gain,80.5,,,
1,opamp_test,GBW_MHz,120,,,
1,stability_test,PM_deg,65,,,
1,stability_test,GM_MHz,45,,,"#;

    let result = MaestroResult::parse_csv(csv, "OpAmp.1");

    assert_eq!(result.tests, vec!["opamp_test", "stability_test"]);
    assert_eq!(result.num_points(), 1);
    assert_eq!(result.points[0].outputs.len(), 4);
}

/// Test summary() method formats correctly.
#[test]
fn test_maestro_summary() {
    let csv = r#"1,t,g,1.0,,,passed
2,t,g,2.0,,,failed
3,t,g,3.0,,,passed"#;

    let result = MaestroResult::parse_csv(csv, "SumTest.1");
    let summary = result.summary();

    assert!(summary.contains("SumTest.1"));
    assert!(summary.contains("passed=2/3"));
    assert!(summary.contains("points=3"));
}

/// Test parsing with trailing newlines in values (CSV artifact).
#[test]
fn test_maestro_trailing_n_artifact() {
    let csv = "Parameters: VDD=1.2\n1,test,val,3.5,,,passedn";

    let result = MaestroResult::parse_csv(csv, "TrailingN.1");

    assert_eq!(
        result.points[0].parameters.get("VDD"),
        Some(&"1.2".to_string())
    );
    // Value should have trailing n removed
    let value = &result.points[0].outputs.get("val").unwrap().value;
    assert_eq!(value, "3.5");
}

/// Test point numbers are correctly tracked across Parameters lines.
#[test]
fn test_maestro_point_numbering() {
    let csv = r#"Parameters: A=1
1,t,v,1.0,,,
Parameters: A=2
2,t,v,2.0,,,
Parameters: A=3
3,t,v,3.0,,,"#;

    let result = MaestroResult::parse_csv(csv, "PointNum.1");

    assert_eq!(result.points[0].point, 1);
    assert_eq!(result.points[1].point, 2);
    assert_eq!(result.points[2].point, 3);
    assert_eq!(result.get_scalar(2, "v"), Some(2.0));
}
