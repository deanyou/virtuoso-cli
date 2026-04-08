pub mod corner;

use crate::client::bridge::escape_skill_string;
use corner::{AnalysisConfig, CornerConfig};
use std::collections::HashMap;

pub fn setup_skill(lib: &str, cell: &str, view: &str, simulator: &str) -> String {
    let lib = escape_skill_string(lib);
    let cell = escape_skill_string(cell);
    let view = escape_skill_string(view);
    // Only call simulator() if not already set to avoid resetting session state (modelFile etc.)
    format!(
        "unless(simulator() == '{simulator} simulator('{simulator}))\ndesign(\"{lib}\" \"{cell}\" \"{view}\")\nresultsDir()"
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

pub fn run_skill() -> String {
    "run()".into()
}

pub fn measure_skill(analysis_type: &str, exprs: &[String]) -> String {
    if exprs.len() == 1 {
        format!("selectResult('{analysis_type})\n{}", exprs[0])
    } else {
        let body = exprs
            .iter()
            .map(|e| format!("  {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("selectResult('{analysis_type})\nlist(\n{body}\n)")
    }
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
