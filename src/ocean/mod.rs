pub mod corner;

use crate::client::bridge::escape_skill_string;
use corner::{AnalysisConfig, CornerConfig};
use std::collections::HashMap;

pub fn setup_skill(lib: &str, cell: &str, view: &str, simulator: &str) -> String {
    let lib = escape_skill_string(lib);
    let cell = escape_skill_string(cell);
    let view = escape_skill_string(view);
    // progn() wraps three expressions into one — evalstring() only evaluates the
    // first top-level expression in a string, so newline-separated calls are silently
    // ignored.  Unless simulator is already set to avoid resetting modelFile etc.
    format!(
        r#"progn(unless(simulator()=='{simulator} simulator('{simulator})) design("{lib}" "{cell}" "{view}") resultsDir())"#
    )
}

pub fn analysis_skill(config: &AnalysisConfig) -> String {
    let typ = &config.analysis_type;
    let mut skill = format!("analysis('{typ}");
    for (k, v) in &config.params {
        let val = match v {
            serde_json::Value::String(s) => format!(" ?{k} \"{s}\""),
            serde_json::Value::Number(n) => format!(" ?{k} {n}"),
            other => format!(" ?{k} \"{other}\""),
        };
        skill.push_str(&val);
    }
    skill.push(')');
    skill
}

pub fn analysis_skill_simple(typ: &str, params: &HashMap<String, String>) -> String {
    let mut skill = format!("analysis('{typ}");
    for (k, v) in params {
        // Don't quote booleans (t/nil) or numbers
        if v == "t" || v == "nil" || v.parse::<f64>().is_ok() {
            skill.push_str(&format!(" ?{k} {v}"));
        } else {
            skill.push_str(&format!(" ?{k} \"{v}\""));
        }
    }
    skill.push(')');
    skill
}

pub fn sweep_skill(
    var: &str,
    values: &[f64],
    analysis_type: &str,
    measure_exprs: &[String],
) -> String {
    let var = escape_skill_string(var);
    let values_str = values
        .iter()
        .map(|v| format!("{v:e}"))
        .collect::<Vec<_>>()
        .join(" ");

    let measures = measure_exprs
        .iter()
        .map(|e| format!("      {e}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"let((results)
  results = nil
  foreach(val '({values_str})
    desVar("{var}" val)
    run()
    selectResult('{analysis_type})
    results = cons(list(val
{measures}
    ) results)
  )
  reverse(results)
)"#
    )
}

pub fn corner_skill(config: &CornerConfig) -> String {
    let model_file = escape_skill_string(&config.model_file);
    let analysis = analysis_skill(&config.analysis);

    // Build corner data list
    let _corner_entries: Vec<String> = config
        .corners
        .iter()
        .map(|c| {
            let _name = escape_skill_string(&c.name);
            let section = escape_skill_string(&c.section);
            // Collect extra vars
            let vars: Vec<String> = c
                .vars
                .iter()
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::String(s) => format!("\"{s}\""),
                        other => other.to_string(),
                    };
                    format!("    desVar(\"{k}\" {val})")
                })
                .collect();
            let vars_code = vars.join("\n");
            format!(
                r#"    ;; Corner: {name}
    modelFile('("{model_file}" "") "{section}")
    temp({temp})
{vars_code}"#,
                name = c.name,
                temp = c.temp,
            )
        })
        .collect();

    let measures = config
        .measures
        .iter()
        .map(|m| format!("      {}", m.expr))
        .collect::<Vec<_>>()
        .join("\n");

    // Build corner names for identification
    let _corner_names: Vec<String> = config
        .corners
        .iter()
        .map(|c| format!("\"{}\"", escape_skill_string(&c.name)))
        .collect();

    let mut skill = format!(
        "simulator('{sim})\ndesign(\"{lib}\" \"{cell}\" \"{view}\")\n{analysis}\n",
        sim = config.simulator.as_deref().unwrap_or("spectre"),
        lib = escape_skill_string(&config.design.lib),
        cell = escape_skill_string(&config.design.cell),
        view = escape_skill_string(&config.design.view),
    );

    skill.push_str("let((results)\n  results = nil\n");

    for corner in config.corners.iter() {
        let name = escape_skill_string(&corner.name);
        let section = escape_skill_string(&corner.section);
        let vars_code: String = corner
            .vars
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => format!("\"{s}\""),
                    other => other.to_string(),
                };
                format!("  desVar(\"{k}\" {val})\n")
            })
            .collect();

        skill.push_str(&format!(
            r#"  ;; {name}
  modelFile('("{model_file}" "") "{section}")
  temp({temp})
{vars_code}  run()
  selectResult('{analysis_type})
  results = cons(list("{name}" {temp}
{measures}
  ) results)
"#,
            temp = corner.temp,
            analysis_type = config.analysis.analysis_type,
        ));
    }

    skill.push_str("  reverse(results)\n)");
    skill
}

/// Parse a SKILL list result like `((1.0 2.0) (3.0 4.0))` into Vec<Vec<String>>
pub fn parse_skill_list(output: &str) -> Vec<Vec<String>> {
    let output = output.trim();
    if output.is_empty() || output == "nil" {
        return Vec::new();
    }

    let mut results = Vec::new();
    let mut depth = 0i32;
    let mut current_row = Vec::new();
    let mut current_token = String::new();

    for ch in output.chars() {
        match ch {
            '(' => {
                depth += 1;
                if depth == 1 {
                    // outer list start
                    continue;
                }
                if depth == 2 {
                    // inner list start
                    current_row.clear();
                    continue;
                }
                current_token.push(ch);
            }
            ')' => {
                depth -= 1;
                if depth == 1 {
                    // inner list end
                    if !current_token.is_empty() {
                        current_row.push(current_token.trim().trim_matches('"').to_string());
                        current_token.clear();
                    }
                    if !current_row.is_empty() {
                        results.push(current_row.clone());
                    }
                    continue;
                }
                if depth == 0 {
                    // outer list end — handle flat list case
                    if !current_token.is_empty() {
                        current_row.push(current_token.trim().trim_matches('"').to_string());
                        current_token.clear();
                    }
                    if !current_row.is_empty() && results.is_empty() {
                        results.push(current_row.clone());
                    }
                    continue;
                }
                current_token.push(ch);
            }
            ' ' | '\t' | '\n' => {
                if !current_token.is_empty() {
                    current_row.push(current_token.trim().trim_matches('"').to_string());
                    current_token.clear();
                }
            }
            _ => {
                current_token.push(ch);
            }
        }
    }

    // Handle single value case
    if results.is_empty() && !output.starts_with('(') {
        results.push(vec![output.trim_matches('"').to_string()]);
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn setup_skill_format() {
        let s = setup_skill("myLib", "myCell", "schematic", "spectre");
        // Must be a single top-level expression so evalstring() evaluates all parts.
        assert!(s.starts_with("progn("), "{s}");
        assert!(
            s.contains("design(\"myLib\" \"myCell\" \"schematic\")"),
            "{s}"
        );
        assert!(s.contains("spectre"), "{s}");
        assert!(s.contains("resultsDir()"), "{s}");
    }

    #[test]
    fn setup_skill_escapes_lib() {
        let s = setup_skill(r#"my"Lib"#, "cell", "schematic", "spectre");
        assert!(s.contains(r#"my\"Lib"#), "{s}");
    }

    #[test]
    fn analysis_skill_simple_boolean_unquoted() {
        let mut params = HashMap::new();
        params.insert("start".into(), "1e-9".into());
        params.insert("stop".into(), "1e-6".into());
        params.insert("conservative".into(), "t".into());
        let s = analysis_skill_simple("tran", &params);
        assert!(s.starts_with("analysis('tran"), "{s}");
        // boolean 't' must not be quoted
        assert!(s.contains("?conservative t"), "{s}");
    }

    #[test]
    fn analysis_skill_simple_string_quoted() {
        let mut params = HashMap::new();
        params.insert("errpreset".into(), "moderate".into());
        let s = analysis_skill_simple("tran", &params);
        assert!(s.contains("?errpreset \"moderate\""), "{s}");
    }

    #[test]
    fn sweep_skill_uses_desvar() {
        let values = vec![1.0, 1.2, 1.8];
        let exprs = vec!["VT(\"M1\" \"VGS\")".to_string()];
        let s = sweep_skill("Vdd", &values, "dc", &exprs);
        assert!(s.contains("desVar(\"Vdd\" val)"), "{s}");
        assert!(s.contains("run()"), "{s}");
        assert!(s.contains("reverse(results)"), "{s}");
    }

    #[test]
    fn parse_skill_list_nested() {
        let rows = parse_skill_list("((1.0 2.0) (3.0 4.0))");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["1.0", "2.0"]);
        assert_eq!(rows[1], vec!["3.0", "4.0"]);
    }

    #[test]
    fn parse_skill_list_single_value() {
        let rows = parse_skill_list("42");
        assert_eq!(rows, vec![vec!["42"]]);
    }
}
