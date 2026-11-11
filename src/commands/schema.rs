//! `vcli schema` — machine-readable description of the CLI surface.
//!
//! The schema is **derived from the clap command tree** the parser itself
//! uses, so `--all` can never drift from the commands the binary actually
//! exposes. It used to be a hand-written subset that covered 5 of the 22
//! top-level commands while the help text claimed "the full command tree".

use clap::builder::ArgAction;
use clap::{Arg, Command};
use serde_json::{json, Map, Value};
use std::any::TypeId;
use std::path::PathBuf;

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

/// JSON type name for a clap argument.
///
/// clap does not hand out the Rust type of an argument, but
/// `ValueParser::type_id()` returns an id that compares equal to a
/// `std::any::TypeId`, which recovers the concrete type without naming
/// clap's internal `AnyValueId`. That type's `Debug` output would have been
/// the easy way in, but it is only populated in debug builds — reading it
/// would make the schema differ between a debug and a release binary.
fn arg_type(arg: &Arg) -> &'static str {
    // A closed set of values is an enum whatever the Rust type behind it.
    if !arg.get_possible_values().is_empty() {
        return "enum";
    }
    // `SetTrue`/`SetFalse` are boolean switches by construction.
    match arg.get_action() {
        ArgAction::SetTrue | ArgAction::SetFalse => return "bool",
        _ => {}
    }
    let id = arg.get_value_parser().type_id();
    if id == TypeId::of::<bool>() {
        "bool"
    } else if id == TypeId::of::<PathBuf>() {
        "path"
    } else if id == TypeId::of::<u8>() {
        "u8"
    } else if id == TypeId::of::<u16>() {
        "u16"
    } else if id == TypeId::of::<u32>() {
        "u32"
    } else if id == TypeId::of::<u64>() {
        "u64"
    } else if id == TypeId::of::<usize>() {
        "usize"
    } else if id == TypeId::of::<i8>() {
        "i8"
    } else if id == TypeId::of::<i16>() {
        "i16"
    } else if id == TypeId::of::<i32>() {
        "i32"
    } else if id == TypeId::of::<i64>() {
        "i64"
    } else if id == TypeId::of::<isize>() {
        "isize"
    } else if id == TypeId::of::<f32>() {
        "f32"
    } else if id == TypeId::of::<f64>() {
        "f64"
    } else {
        "string"
    }
}

/// The default value of an argument, typed the way its own parser types it.
///
/// A boolean switch defaults to a JSON boolean and a numeric argument to a
/// JSON number, so a consumer reads the scalars they are instead of parsing
/// a string. A stringified `false` is not merely ugly: an agent that copies
/// the default onto the command line writes `--dry-run=false`, which clap
/// rejects outright ("unexpected value 'false' for '--dry-run'").
fn default_value(arg: &Arg, kind: &str) -> Option<Value> {
    let text = arg
        .get_default_values()
        .first()?
        .to_string_lossy()
        .into_owned();
    match kind {
        "bool" => match text.as_str() {
            "true" => Some(json!(true)),
            "false" => Some(json!(false)),
            _ => Some(json!(text)),
        },
        "u8" | "u16" | "u32" | "u64" | "usize" => text.parse::<u64>().ok().map(Value::from),
        "i8" | "i16" | "i32" | "i64" | "isize" => text.parse::<i64>().ok().map(Value::from),
        "f32" | "f64" => text.parse::<f64>().ok().map(Value::from),
        _ => Some(json!(text)),
    }
}

/// Describe a single clap argument.
fn arg_schema(arg: &Arg) -> Value {
    let mut m = Map::new();
    let kind = arg_type(arg);
    m.insert("type".into(), json!(kind));
    m.insert("required".into(), json!(arg.is_required_set()));

    if let Some(help) = arg.get_help() {
        let text = help.to_string();
        if !text.is_empty() {
            m.insert("description".into(), json!(text));
        }
    }

    if let Some(default) = default_value(arg, kind) {
        m.insert("default".into(), default);
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
