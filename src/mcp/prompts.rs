//! MCP prompts — three workflow prompts assembled from the existing tools.
//!
//! A prompt is a *workflow*, not a new capability: every prompt below only
//! names tools the server already exposes (`maestro_*`, `schematic_*`,
//! `cell_*`, `skill_exec`, ...). Nothing here adds an RPC method.

use serde_json::{json, Map, Value};

/// One argument of a workflow prompt.
#[derive(Debug, Clone, Copy)]
pub struct PromptArgument {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

/// One workflow prompt.
#[derive(Debug, Clone, Copy)]
pub struct WorkflowPrompt {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments: &'static [PromptArgument],
    /// Message body. `{{argument}}` placeholders are filled from the
    /// `prompts/get` arguments.
    pub template: &'static str,
}

/// Placeholder substituted for an argument the caller did not supply.
pub const MISSING_ARGUMENT: &str = "(not provided)";

const MAESTRO_RUN_AND_CHECK: WorkflowPrompt = WorkflowPrompt {
    name: "maestro-run-and-check",
    description: "Open or reuse a Maestro session, set a design variable, run the simulation, and read back the output values.",
    arguments: &[
        PromptArgument {
            name: "session",
            description: "Existing Maestro session id (e.g. fnxSession0). Omit to open a new one.",
            required: false,
        },
        PromptArgument {
            name: "lib",
            description: "Library of the cell to simulate (used when opening a new session).",
            required: false,
        },
        PromptArgument {
            name: "cell",
            description: "Cell to simulate (used when opening a new session).",
            required: false,
        },
        PromptArgument {
            name: "variable",
            description: "Design variable to set before the run, e.g. wn.",
            required: false,
        },
        PromptArgument {
            name: "value",
            description: "Value for the design variable, e.g. 2u.",
            required: false,
        },
        PromptArgument {
            name: "output",
            description: "Output expression to read back after the run, e.g. gain.",
            required: false,
        },
    ],
    template: r#"Run a Maestro simulation and read its result.

Target session: {{session}}
Library: {{lib}}
Cell: {{cell}}
Design variable: {{variable}} = {{value}}
Output expression: {{output}}

Follow this order and stop at the first failure:

1. If no session id was given, call `maestro_open_session` with lib/cell to get one; otherwise call `maestro_list_sessions` and reuse the session named above.
2. Confirm the session is live with `maestro_list_sessions`. A session id that is not listed must not be used.
3. If a design variable was given, set it with `maestro_set_var` (name, value) and verify with `maestro_get_var` — a variable that reads back with a different value means the run below is meaningless.
4. Call `maestro_run` on that session, then `maestro_get_history_list` to find the run that was just produced.
5. Read the result with `maestro_get_output_value` (name = output expression, test = the history/test name). If it returns nil, do not report a number: report that the value is unavailable and which history was read.
6. Summarize the run: variables actually applied, history/test used, the measured value, and any errors the run reported.

Never report a measurement you did not read from `maestro_get_output_value`."#,
};

const SPECTRE_FAILURE_TRIAGE: WorkflowPrompt = WorkflowPrompt {
    name: "spectre-failure-triage",
    description: "Triage a failed Spectre/Maestro simulation by following the evidence order: history output first, then variables and netlist.",
    arguments: &[
        PromptArgument {
            name: "session",
            description: "Maestro session id that owns the failed run.",
            required: false,
        },
        PromptArgument {
            name: "history",
            description: "History id (run name) of the failed simulation.",
            required: false,
        },
    ],
    template: r#"Triage a failed simulation.

Maestro session: {{session}}
History id: {{history}}

Work the evidence in this order — later steps are worthless if an earlier one was skipped:

1. Locate the failure. Call `maestro_get_history_list` and match the history id above (or take the latest run of the session when no id was given). Record the run's status and error text verbatim.
2. Read the run output before touching the circuit. Use `maestro_snapshot` to pull the run artifacts into a local directory, then inspect the captured log/netlist files, or read them with `skill_exec` on the Virtuoso host. The first concrete error line decides the branch — do not guess a cause from the status alone.
3. Check the setup that produced the run. Call `maestro_list_vars` and `maestro_get_var` for the variables in play; a stale design variable or a missing model/view is a common root cause. Compare the variables against the values the run was supposed to use.
4. Check the netlist. Confirm it was regenerated for the current schematic (`skill_exec` on the host if needed) and that model files and corners resolve. A stale or missing netlist explains "no output" far more often than a circuit problem.
5. Only then consider a circuit-level cause, and state which piece of evidence supports it.

Report: the exact error line, the branch you took, the evidence for it, and the next concrete action. If the evidence does not identify a cause, say so and name the artifact that would settle it — do not present a guess as a diagnosis."#,
};

const CELL_DESIGN_REVIEW: WorkflowPrompt = WorkflowPrompt {
    name: "cell-design-review",
    description: "Review a cellview end to end: cell info, schematic nets/pins/instances, and the schematic check result.",
    arguments: &[
        PromptArgument {
            name: "lib",
            description: "Library name of the cell to review.",
            required: true,
        },
        PromptArgument {
            name: "cell",
            description: "Cell name to review.",
            required: true,
        },
        PromptArgument {
            name: "view",
            description: "View to review (default: schematic).",
            required: false,
        },
    ],
    template: r#"Review the design of a cellview.

Library: {{lib}}
Cell: {{cell}}
View: {{view}}

1. Open the cellview with `cell_open` (lib, cell, view) and confirm what you opened with `cell_info` — the lib/cell/view triple must match the request before anything else is read.
2. Read the topology: `schematic_list_instances` for the devices, `schematic_list_nets` for the connections, `schematic_list_pins` for the interface.
3. Check it: run `schematic_check` and report every error and warning it returns. Do not summarize it as "clean" unless the check actually reported no findings.
4. Cross-check the interface: every pin must appear in the net list, and every net either reaches a pin or is deliberately internal. Report dangling nets and unconnected pins explicitly.
5. Summarize the review as: what the cell contains (by instance), its interface (pins), the nets that carry signal, and the check findings. Flag anything you could not verify from the tools above."#,
};

/// Every prompt the server publishes.
pub fn all_prompts() -> &'static [WorkflowPrompt] {
    &[
        MAESTRO_RUN_AND_CHECK,
        SPECTRE_FAILURE_TRIAGE,
        CELL_DESIGN_REVIEW,
    ]
}

/// Look a prompt up by name.
pub fn find_prompt(name: &str) -> Option<&'static WorkflowPrompt> {
    all_prompts().iter().find(|p| p.name == name)
}

/// `prompts/list` payload.
pub fn list_json() -> Value {
    let prompts: Vec<Value> = all_prompts()
        .iter()
        .map(|p| {
            let arguments: Vec<Value> = p
                .arguments
                .iter()
                .map(|a| {
                    json!({
                        "name": a.name,
                        "description": a.description,
                        "required": a.required,
                    })
                })
                .collect();
            json!({
                "name": p.name,
                "description": p.description,
                "arguments": arguments,
            })
        })
        .collect();
    json!({ "prompts": prompts })
}

/// Render a prompt body by substituting `{{argument}}` placeholders.
pub fn render(prompt: &WorkflowPrompt, arguments: &Map<String, Value>) -> String {
    let mut text = prompt.template.to_string();
    for argument in prompt.arguments {
        let value = arguments
            .get(argument.name)
            .map(scalar_to_string)
            .unwrap_or_else(|| MISSING_ARGUMENT.to_string());
        text = text.replace(&format!("{{{{{}}}}}", argument.name), &value);
    }
    text
}

/// `prompts/get` payload for a known prompt.
pub fn get_json(prompt: &WorkflowPrompt, arguments: &Map<String, Value>) -> Value {
    json!({
        "description": prompt.description,
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": render(prompt, arguments),
                }
            }
        ]
    })
}

/// Flatten a JSON argument to the text that is substituted into a prompt.
fn scalar_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => MISSING_ARGUMENT.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_three_workflow_prompts_are_published() {
        let prompts = all_prompts();
        assert_eq!(prompts.len(), 3);
        let names: Vec<&str> = prompts.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            vec![
                "maestro-run-and-check",
                "spectre-failure-triage",
                "cell-design-review"
            ]
        );
    }

    #[test]
    fn every_prompt_names_only_existing_tools() {
        let known: Vec<String> = crate::mcp::tools::all_tools_unfiltered()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        for prompt in all_prompts() {
            for tool in mentioned_tools(prompt.template) {
                assert!(
                    known.contains(&tool),
                    "prompt '{}' references unknown tool '{tool}'",
                    prompt.name
                );
            }
        }
    }

    #[test]
    fn render_fills_arguments_and_marks_the_missing_ones() {
        let prompt = find_prompt("cell-design-review").unwrap();
        let mut args = Map::new();
        args.insert("lib".into(), json!("myLib"));
        args.insert("cell".into(), json!("myCell"));

        let text = render(prompt, &args);
        assert!(text.contains("myLib"));
        assert!(text.contains("myCell"));
        assert!(text.contains(MISSING_ARGUMENT));
        assert!(!text.contains("{{lib}}"));
    }

    #[test]
    fn list_json_exposes_name_description_and_arguments() {
        let value = list_json();
        let prompts = value["prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 3);
        for prompt in prompts {
            assert!(prompt["name"].is_string());
            assert!(prompt["description"].is_string());
            assert!(prompt["arguments"].is_array());
        }
        let review = prompts
            .iter()
            .find(|p| p["name"] == "cell-design-review")
            .unwrap();
        assert_eq!(review["arguments"][0]["name"], "lib");
        assert_eq!(review["arguments"][0]["required"], true);
    }

    #[test]
    fn get_json_returns_a_single_user_message() {
        let prompt = find_prompt("maestro-run-and-check").unwrap();
        let value = get_json(prompt, &Map::new());
        let messages = value["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"]["type"], "text");
        assert!(messages[0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("maestro_run"));
    }

    /// Extract `tool_name` mentions from a prompt body.
    fn mentioned_tools(template: &str) -> Vec<String> {
        template
            .split('`')
            .filter(|part| {
                part.contains('_')
                    && part
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            })
            .map(|part| part.to_string())
            .collect()
    }
}
