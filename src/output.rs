use serde::Serialize;
use serde_json::Value;
use std::io::{self, IsTerminal, Write};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Json,
    Table,
}

impl OutputFormat {
    pub fn resolve(explicit: Option<&str>) -> Self {
        match explicit {
            Some("json") => OutputFormat::Json,
            Some("table") => OutputFormat::Table,
            Some(_) => OutputFormat::Table,
            None => {
                if io::stdout().is_terminal() {
                    OutputFormat::Table
                } else {
                    OutputFormat::Json
                }
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CliError {
    pub error: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    pub retryable: bool,
}

impl CliError {
    pub fn print(&self, format: OutputFormat) {
        let stderr = io::stderr();
        let mut out = stderr.lock();
        match format {
            OutputFormat::Json => {
                let _ = serde_json::to_writer(&mut out, self);
                let _ = writeln!(out);
            }
            OutputFormat::Table => {
                let _ = writeln!(out, "error: {}", self.message);
                if let Some(ref suggestion) = self.suggestion {
                    let _ = writeln!(out, "suggestion: {suggestion}");
                }
            }
        }
    }
}

pub fn print_json(value: &Value) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = serde_json::to_writer_pretty(&mut out, value);
    let _ = writeln!(out);
}

#[allow(dead_code)]
pub fn print_table(rows: &[(&str, &str)]) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for (key, value) in rows {
        let _ = writeln!(out, "  {key}: {value}");
    }
}

pub fn print_value(value: &Value, format: OutputFormat) {
    match format {
        OutputFormat::Json => print_json(value),
        OutputFormat::Table => {
            if let Some(obj) = value.as_object() {
                for (k, v) in obj {
                    let display = match v {
                        Value::String(s) => s.clone(),
                        Value::Null => "n/a".to_string(),
                        other => other.to_string(),
                    };
                    println!("  {k}: {display}");
                }
            } else {
                println!("{}", value);
            }
        }
    }
}

#[allow(dead_code)]
pub fn print_section(title: &str, value: &Value, format: OutputFormat) {
    match format {
        OutputFormat::Json => {} // JSON mode: caller aggregates into single object
        OutputFormat::Table => {
            println!("{title}:");
            print_value(value, format);
            println!();
        }
    }
}
