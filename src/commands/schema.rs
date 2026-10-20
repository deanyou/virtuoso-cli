//! `vcli schema` — machine-readable description of the CLI surface.
//!
//! The schema is **derived from the clap command tree** the parser itself
//! uses, so `--all` can never drift from the commands the binary actually
//! exposes. It used to be a hand-written subset that covered 5 of the 22
//! top-level commands while the help text claimed "the full command tree".

use clap::builder::ArgAction;
use clap::{Arg, Command};
use serde_json::{json, Map, Value};

/// Exit codes the CLI can return — mirrors `src/exit_codes.rs`.
fn exit_codes() -> Value {
    json!({
        "0": "success",
        "1": "general error",
        "2": "invalid arguments / usage error",
        "3": "resource not found",
        "4": "permission denied",
        "5": "conflict / already exists",
        "10": "dry-run passed",
    })
}

/// Best-effort JSON type name for a clap argument.
///
/// clap does not expose the concrete Rust type of an argument, so this
/// reports what an agent actually needs: whether the flag is a boolean
/// switch, a closed set of values, or free-form text.
fn arg_type(arg: &Arg) -> &'static str {
    match arg.get_action() {
        ArgAction::SetTrue | ArgAction::SetFalse => return "bool",
        _ => {}
    }
    if !arg.get_possible_values().is_empty() {
        return "enum";
    }
    "string"
}

/// Describe a single clap argument.
fn arg_schema(arg: &Arg) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!(arg_type(arg)));
    m.insert("required".into(), json!(arg.is_required_set()));

    if let Some(help) = arg.get_help() {
        let text = help.to_string();
        if !text.is_empty() {
            m.insert("description".into(), json!(text));
        }
    }

    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().into_owned())
        .collect();
    if !defaults.is_empty() {
        m.insert("default".into(), json!(defaults[0]));
    }

    let possible: Vec<String> = arg
        .get_possible_values()
        .iter()
        .map(|v| v.get_name().to_string())
        .collect();
    if !possible.is_empty() {
        m.insert("values".into(), json!(possible));
    }

    if let Some(names) = arg.get_value_names() {
        if let Some(name) = names.first() {
            m.insert("value_name".into(), json!(name.to_string()));
        }
    }

    Value::Object(m)
}

/// Key used for a flag in the schema map: `--long` when present, else `-short`.
fn flag_key(arg: &Arg) -> Option<String> {
    if let Some(long) = arg.get_long() {
        return Some(format!("--{long}"));
    }
    arg.get_short().map(|c| format!("-{c}"))
}

/// Extract the `Examples:` block from a clap about/long_about text.
fn examples_from_about(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("examples:") {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        // Example lines are indented under the "Examples:" header; the first
        // non-indented line ends the block.
        if line.starts_with(' ') || line.starts_with('\t') {
            out.push(trimmed.to_string());
        } else {
            break;
        }
    }
    out
}

/// Short one-line description for a command.
fn command_description(cmd: &Command) -> String {
    if let Some(about) = cmd.get_about() {
        let text = about.to_string();
        if !text.is_empty() {
            return text;
        }
    }
    if let Some(long) = cmd.get_long_about() {
        let text = long.to_string();
        if let Some(first) = text.lines().next() {
            let first = first.trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    String::new()
}

/// Convert a clap command (and its subtree) into the schema JSON shape.
fn command_schema(cmd: &Command) -> Value {
    let mut m = Map::new();

    let description = command_description(cmd);
    if !description.is_empty() {
        m.insert("description".into(), json!(description));
    }

    let mut flags = Map::new();
    let mut args = Map::new();
    for arg in cmd.get_arguments() {
        // Implicit args clap adds for itself are not part of the user-facing
        // contract, and hidden args are internal.
        if arg.is_hide_set() {
            continue;
        }
        let id = arg.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }
        if arg.is_positional() {
            args.insert(id.to_string(), arg_schema(arg));
        } else if let Some(key) = flag_key(arg) {
            flags.insert(key, arg_schema(arg));
        }
    }
    if !flags.is_empty() {
        m.insert("flags".into(), Value::Object(flags));
    }
    if !args.is_empty() {
        m.insert("args".into(), Value::Object(args));
    }

    let mut subcommands = Map::new();
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        subcommands.insert(sub.get_name().to_string(), command_schema(sub));
    }
    if !subcommands.is_empty() {
        m.insert("subcommands".into(), Value::Object(subcommands));
    }

    let examples = cmd
        .get_long_about()
        .map(|l| examples_from_about(&l.to_string()))
        .unwrap_or_default();
    if !examples.is_empty() {
        m.insert("examples".into(), json!(examples));
    }

    Value::Object(m)
}

/// Build the full schema document from a clap command tree.
pub fn build(cli: &Command) -> Value {
    // `build()` propagates global args (--format, --session, ...) down to every
    // subcommand, so the schema shows the flags a caller can actually pass.
    let mut cli = cli.clone();
    cli.build();

    let mut commands = Map::new();
    for sub in cli.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        commands.insert(sub.get_name().to_string(), command_schema(sub));
    }

    // Global flags are declared on the root and inherited everywhere.
    let mut global_flags = Map::new();
    for arg in cli.get_arguments() {
        if !arg.is_global_set() || arg.is_hide_set() {
            continue;
        }
        let id = arg.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }
        if let Some(key) = flag_key(arg) {
            global_flags.insert(key, arg_schema(arg));
        }
    }

    json!({
        "name": "virtuoso",
        "version": env!("CARGO_PKG_VERSION"),
        "commands": Value::Object(commands),
        "global_flags": Value::Object(global_flags),
        "exit_codes": exit_codes(),
    })
}

/// Render the schema for a specific noun / verb, or the whole tree.
pub fn show(cli: &Command, noun: Option<&str>, verb: Option<&str>) -> Value {
    let full_schema = build(cli);

    match (noun, verb) {
        (None, _) => full_schema,
        (Some(n), None) => {
            if let Some(cmd) = full_schema["commands"].get(n) {
                json!({ n: cmd })
            } else {
                json!({ "error": "not_found", "message": format!("unknown command: {n}") })
            }
        }
        (Some(n), Some(v)) => {
            if let Some(sub) = full_schema["commands"]
                .get(n)
                .and_then(|c| c.get("subcommands"))
                .and_then(|s| s.get(v))
            {
                json!({ format!("{n} {v}"): sub })
            } else {
                json!({ "error": "not_found", "message": format!("unknown command: {n} {v}") })
            }
        }
    }
}
