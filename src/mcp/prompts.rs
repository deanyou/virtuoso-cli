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
3. If a design variable was given, set it with `maestro_set_var` (name, value). A `maestro_get_var` readback does NOT verify that the variable reached the simulator: `maestro_set_var` and `maestro_get_var` share one Maestro-internal scope, so a readback that agrees with what you just wrote only proves that the write was recorded in that scope. The ground truth is the generated netlist — after the run setup has produced it, the `parameters` line of the run's `input.scs` (grep it on the host) must show the new value. If it still shows the old value, the run did not use the new value: say so, and do not report that run's measurements as belonging to the new value.
4. Call `maestro_run` on that session, then `maestro_get_history_list` to find the run that was just produced. `maestro_get_history_list` lists the histories of the CURRENT session only.
5. Read the result with `maestro_get_output_value` (name = output expression, test = the TEST name, e.g. tb_cmp_SA — a test name, never a history name such as Interactive.0). The MCP surface cannot open a specific history, so the value comes from whichever results are currently loaded: if the history you just produced is not the currently loaded one, do NOT report a number — say that the value could not be attributed to this run and point the user at the `maestro-read-results` skill, which reads the PSF results directly. If the call returns nil, do not report a number either: report that the value is unavailable and which history was read.
6. Summarize the run: variables actually applied (per the `parameters` line of `input.scs`, not per a variable readback), history/test used, the measured value, and any errors the run reported.

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

1. Align on the session first. Call `maestro_list_sessions` and work on the session named above (or the only live session when no id was given). Everything below is scoped to the CURRENT session: `maestro_get_history_list` lists that session's histories and `maestro_list_vars` reads that session's variables — neither can see a different session. If the failed run belongs to another session, say which one and stop rather than reading the current session's data as if it were the failed run's.
2. Locate the failure. Call `maestro_get_history_list` and match the history id above (or take the latest run of the session when no id was given). Record the run's status and error text verbatim.
3. Read the run output before touching the circuit. Use `maestro_snapshot` to pull the run artifacts into a local directory, then inspect the captured log/netlist files, or read them with `skill_exec` on the Virtuoso host. The first concrete error line decides the branch — do not guess a cause from the status alone.
4. Check the setup that produced the run. Call `maestro_list_vars` and `maestro_get_var` for the variables in play — both are current-session scoped, so they only describe the session aligned in step 1; a stale design variable or a missing model/view is a common root cause. Compare the variables against the values the run was supposed to use.
5. Check the netlist. Confirm it was regenerated for the current schematic (`skill_exec` on the host if needed) and that model files and corners resolve. A stale or missing netlist explains "no output" far more often than a circuit problem.
6. Only then consider a circuit-level cause, and state which piece of evidence supports it.

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
            description: "View the caller asked to review. The review reads schematic data, so the cellview is opened with view \"schematic\" in read mode.",
            required: false,
        },
    ],
    template: r#"Review the design of a cellview.

Library: {{lib}}
Cell: {{cell}}
Requested view: {{view}}

1. Open the cellview with `cell_open` and confirm what you opened with `cell_info` — the lib/cell/view triple must match what you asked `cell_open` for, before anything else is read. Always pass view "schematic" and mode "r" to `cell_open`: its own defaults are view "layout" and mode "a" (append), and append takes an edit lock — it can even create and save a missing cellview, which a review must never do. If the caller asked for a view other than schematic, review the schematic view and say in the summary that the requested view was not the one reviewed.
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

/// The tool names a prompt body mentions.
///
/// A prompt is a workflow over the tools the server already exposes, so the
/// tools it names are its real dependency list: a prompt may only be published
/// when every one of them is available under the server's capabilities.
///
/// The mentions are the backticked snake_case identifiers of the body — the
/// same convention the prompt bodies use to write tool names.
pub fn mentioned_tools(template: &str) -> Vec<String> {
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

/// Whether every tool a prompt names is available.
pub fn prompt_tools_available(prompt: &WorkflowPrompt, available_tools: &[String]) -> bool {
    mentioned_tools(prompt.template)
        .iter()
        .all(|tool| available_tools.contains(tool))
}

/// The prompts that may be published for the given available tool names.
///
/// A prompt that names a tool the server does not expose (for example a
/// tool that needs the Admin capability) is hidden: publishing it would
/// advertise a workflow the caller cannot finish.
pub fn available_prompts(available_tools: &[String]) -> Vec<&'static WorkflowPrompt> {
    all_prompts()
        .iter()
        .filter(|p| prompt_tools_available(p, available_tools))
        .collect()
}

/// `prompts/list` payload, filtered to the prompts whose tools are available.
pub fn list_json(available_tools: &[String]) -> Value {
    let prompts: Vec<Value> = available_prompts(available_tools)
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

/// The required arguments of a prompt that the caller did not supply.
///
/// A missing key and an explicit `null` are the same thing here: the
/// argument has no value, and the prompt must not be rendered with a
/// placeholder standing in for it.
pub fn missing_required_arguments(
    prompt: &WorkflowPrompt,
    arguments: &Map<String, Value>,
) -> Vec<&'static str> {
    prompt
        .arguments
        .iter()
        .filter(|a| a.required)
        .filter(|a| match arguments.get(a.name) {
            None => true,
            Some(Value::Null) => true,
            Some(_) => false,
        })
        .map(|a| a.name)
        .collect()
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

    /// The tool names the server publishes under the default capabilities.
    fn default_available_tools() -> Vec<String> {
        crate::mcp::tools::all_tools(&crate::capability::CapabilitySet::from_env())
            .iter()
            .map(|t| t.name.clone())
            .collect()
    }

    /// The tool names the server publishes under the Admin capability.
    fn admin_available_tools() -> Vec<String> {
        crate::mcp::tools::all_tools(&crate::capability::CapabilitySet::from_capabilities(&[
            crate::capability::Capability::Admin,
        ]))
        .iter()
        .map(|t| t.name.clone())
        .collect()
    }

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
        // The published tool list is capability-filtered, so this checks the
        // prompts that are actually published against the tools that are
        // actually available.
        let available = default_available_tools();
        for prompt in available_prompts(&available) {
            for tool in mentioned_tools(prompt.template) {
                assert!(
                    available.contains(&tool),
                    "published prompt '{}' references unavailable tool '{tool}'",
                    prompt.name
                );
            }
        }
    }

    #[test]
    fn no_prompt_names_an_unknown_tool() {
        // Even a prompt that is hidden under the default capabilities may not
        // name a tool that does not exist at all.
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
        let value = list_json(
            &crate::mcp::tools::all_tools_unfiltered()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
        );
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
    fn prompts_that_name_an_unavailable_tool_are_hidden() {
        // `spectre-failure-triage` names `skill_exec`, which needs the Admin
        // capability — the default capability set does not have it.
        let default_tools = default_available_tools();
        let names: Vec<&str> = available_prompts(&default_tools)
            .iter()
            .map(|p| p.name)
            .collect();
        assert!(
            !names.contains(&"spectre-failure-triage"),
            "the triage prompt must be hidden without Admin: {names:?}"
        );

        let admin_tools = admin_available_tools();
        let names: Vec<&str> = available_prompts(&admin_tools)
            .iter()
            .map(|p| p.name)
            .collect();
        assert!(
            names.contains(&"spectre-failure-triage"),
            "the triage prompt must be published with Admin: {names:?}"
        );
    }

    #[test]
    fn missing_required_arguments_are_reported_by_name() {
        let prompt = find_prompt("cell-design-review").unwrap();

        let missing = missing_required_arguments(prompt, &Map::new());
        assert_eq!(missing, vec!["lib", "cell"]);

        let mut args = Map::new();
        args.insert("lib".into(), json!("myLib"));
        assert_eq!(missing_required_arguments(prompt, &args), vec!["cell"]);

        // An explicit null is as good as missing.
        let mut args = Map::new();
        args.insert("lib".into(), json!("myLib"));
        args.insert("cell".into(), Value::Null);
        assert_eq!(missing_required_arguments(prompt, &args), vec!["cell"]);

        args.insert("cell".into(), json!("myCell"));
        assert!(missing_required_arguments(prompt, &args).is_empty());
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

    /// The variable step must not claim that a `maestro_get_var` readback
    /// proves the value reached the simulator — the readback is same-scope.
    #[test]
    fn the_variable_step_names_the_netlist_as_the_ground_truth() {
        let prompt = find_prompt("maestro-run-and-check").unwrap();
        let text = prompt.template;
        assert!(
            text.contains("input.scs") && text.contains("parameters"),
            "the netlist check must be named: {text}"
        );
        assert!(
            text.contains("does NOT verify"),
            "the same-scope readback must be called out: {text}"
        );
    }

    /// The history/test conflation must be gone: `test` is a test name.
    #[test]
    fn the_output_value_step_uses_the_test_name() {
        let prompt = find_prompt("maestro-run-and-check").unwrap();
        let text = prompt.template;
        assert!(
            text.contains("TEST name"),
            "the test parameter must be named: {text}"
        );
        assert!(
            text.contains("maestro-read-results"),
            "the PSF read fallback must be named: {text}"
        );
        assert!(
            text.contains("CURRENT session"),
            "the history list scope must be stated: {text}"
        );
    }

    /// The review prompt must open the cellview read-only: `cell_open`'s own
    /// defaults are view layout and mode append.
    #[test]
    fn the_review_prompt_opens_the_schematic_read_only() {
        let prompt = find_prompt("cell-design-review").unwrap();
        let text = prompt.template;
        assert!(
            text.contains("\"schematic\"") && text.contains("\"r\""),
            "the review must pass view schematic and mode r: {text}"
        );
        assert!(
            !prompt
                .arguments
                .iter()
                .find(|a| a.name == "view")
                .unwrap()
                .description
                .contains("default: schematic"),
            "the view argument must not claim a schematic default"
        );
    }

    /// The triage prompt must align on the session before reading anything.
    #[test]
    fn the_triage_prompt_aligns_on_the_session_first() {
        let prompt = find_prompt("spectre-failure-triage").unwrap();
        let text = prompt.template;
        let first_step = text
            .split("\n1. ")
            .nth(1)
            .and_then(|rest| rest.split("\n2. ").next())
            .unwrap_or_default();
        assert!(
            first_step.contains("maestro_list_sessions"),
            "the first step must align on the session: {first_step}"
        );
        assert!(
            text.contains("current-session scoped"),
            "the current-session scope must be stated: {text}"
        );
    }
}
