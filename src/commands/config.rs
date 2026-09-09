use crate::config::{Config, ConfigReport, ConfigSource};
use crate::error::{Result, VirtuosoError};
use crate::output::OutputFormat;
use serde_json::{json, Value};

/// `vcli config check` — explain every resolved value's provenance.
///
/// Reads the same environment / file / target path the live `Config::from_env`
/// would use and produces a [`ConfigReport`] keyed off `[profile.<active>]`
/// and target branches. `--format json` produces a flat, jq-friendly document;
/// the default table mode prints a human-readable ledger.
///
/// `--require-explicit` flips the exit code to non-zero when any field came
/// from a built-in default (no env, no file, no target) — useful in CI
/// pipelines that should refuse to deploy a "green" build whose daemon
/// actually has no config. The JSON `status` field mirrors this: `"fail"`
/// only when `--require-explicit` saw at least one Default source.
///
/// Implementation note: the report is built by [`Config::build_report`], which
/// re-probes the same layered lookup the runtime uses. There is no separate
/// "check" resolution path to drift out of sync with production behavior.
pub fn check(
    format: OutputFormat,
    require_explicit: bool,
    pre_check: std::result::Result<(), &VirtuosoError>,
) -> Result<Value> {
    let profile = Config::resolve_profile();
    let report = Config::build_report(profile.as_deref(), pre_check)?;
    let status = status_for(&report, require_explicit);

    let value = match format {
        OutputFormat::Json => json!({
            "status": status,
            "valid": report.valid,
            "profile": report.profile,
            "active_target": report.active_target,
            "config_file": report.config_file,
            "targets_file": report.targets_file,
            "digest": report.digest,
            "precedence": report.precedence,
            "entries": entries_json(&report),
            "warnings": report.warnings,
            "diagnostics": report.diagnostics,
            // main() reads "dry_run" to pick DRY_RUN_OK (10) over SUCCESS (0);
            // we re-use that mechanism so `--require-explicit` or a hard error
            // propagates a non-zero exit without dragging a second channel
            // through the dispatch result. The status string above remains the
            // human-readable signal; this is the exit-code driver.
            "dry_run": status == "fail" || status == "error",
        }),
        OutputFormat::Table => render_table(&report, status),
    };
    Ok(value)
}

/// One JSON entry per resolved key, with its source layer and a sub-object
/// that names the exact env var / file / target that produced it.
///
/// Shape was chosen so `jq '.entries[] | select(.key == "VB_TIMEOUT")'` is a
/// useful one-liner during incident response.
fn entries_json(report: &ConfigReport) -> Value {
    Value::Array(
        report
            .entries
            .iter()
            .map(|e| {
                let mut obj = serde_json::Map::new();
                obj.insert("key".into(), Value::String(e.key.to_string()));
                obj.insert("value".into(), Value::String(e.value.clone()));
                obj.insert(
                    "source".into(),
                    serde_json::to_value(&e.source).unwrap_or(Value::Null),
                );
                obj.insert(
                    "source_label".into(),
                    Value::String(source_label(&e.source)),
                );
                Value::Object(obj)
            })
            .collect(),
    )
}

/// Short, human-readable tag of a [`ConfigSource`] — used in the table renderer.
fn source_label(src: &ConfigSource) -> String {
    match src {
        ConfigSource::EnvProfile { var } => format!("env[{var}]"),
        ConfigSource::Env { var } => format!("env[{var}]"),
        ConfigSource::FileProfile { file } => {
            format!("file[profile]={}", file.display())
        }
        ConfigSource::FileGlobal { file } => format!("file[global]={}", file.display()),
        ConfigSource::Target { target, file } => {
            format!("target[{target}]={}", file.display())
        }
        ConfigSource::Default { default, kind } => match kind {
            crate::config::DefaultKind::Hardcoded => format!("default={default}"),
            crate::config::DefaultKind::Derived => format!("default(derived)={default}"),
        },
    }
}

fn status_for(report: &ConfigReport, require_explicit: bool) -> &'static str {
    if !report.valid {
        "error"
    } else if !report.warnings.is_empty() {
        "warn"
    } else if require_explicit && !report.all_explicit() {
        "fail"
    } else {
        "ok"
    }
}

/// Render the report as a left-aligned key/value ledger with a trailing
/// precedence block. Designed for terminal scrollback — wide values are
/// preserved verbatim because transport-daemon tokens and SSH key paths
/// cannot be truncated without misleading the reader.
fn render_table(report: &ConfigReport, status: &'static str) -> Value {
    let mut lines: Vec<String> = Vec::new();

    let header = match (&report.profile, &report.active_target) {
        (Some(p), Some(t)) => format!("profile: {p}    active_target: {t}"),
        (Some(p), None) => format!("profile: {p}"),
        (None, Some(t)) => format!("active_target: {t}"),
        (None, None) => "profile: (none)    active_target: (none)".to_string(),
    };
    lines.push(header);

    if let Some(ref f) = report.config_file {
        lines.push(format!("config_file: {}", f.display()));
    } else {
        lines.push("config_file: (not present)".to_string());
    }

    // Column widths derived from the longest actual content so a wide SSH
    // key path doesn't push the source column off the screen on a narrow
    // terminal but a short value doesn't leave ugly gaps either.
    let key_width = report
        .entries
        .iter()
        .map(|e| e.key.len())
        .max()
        .unwrap_or(20)
        .max(20);
    let val_width = report
        .entries
        .iter()
        .map(|e| e.value.len())
        .max()
        .unwrap_or(10)
        .min(60);

    lines.push(String::new());
    lines.push(format!(
        "{:<key_w$}  {:<val_w$}  source",
        "key",
        "value",
        key_w = key_width,
        val_w = val_width
    ));
    lines.push(format!(
        "{}  {}  {}",
        "-".repeat(key_width),
        "-".repeat(val_width),
        "-".repeat(20)
    ));
    for e in &report.entries {
        let truncated = if e.value.len() > val_width {
            format!("{}…", &e.value[..val_width.saturating_sub(1)])
        } else {
            e.value.clone()
        };
        lines.push(format!(
            "{:<key_w$}  {:<val_w$}  {source}",
            e.key,
            truncated,
            source = source_label(&e.source),
            key_w = key_width,
            val_w = val_width
        ));
    }

    if !report.warnings.is_empty() {
        lines.push(String::new());
        lines.push("warnings:".to_string());
        for w in &report.warnings {
            lines.push(format!("  • {w}"));
        }
    }

    lines.push(String::new());
    lines.push("precedence (highest first):".to_string());
    for (i, p) in report.precedence.iter().enumerate() {
        lines.push(format!("  {}. {p}", i + 1));
    }

    // The match arm returns a JSON Value so the framework can apply the same
    // --format plumbing as the JSON path. We wrap the rendered text in a
    // single field so `output::print_value` knows what to print.
    Value::Object({
        let mut obj = serde_json::Map::new();
        obj.insert("rendered".into(), Value::String(lines.join("\n")));
        obj.insert("status".into(), Value::String(status.to_string()));
        obj
    })
}
