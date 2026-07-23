//! Plugin schema definitions — TOML parsing structures.

use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

use crate::client::skill_runtime::string_literal;
use crate::error::{Result, VirtuosoError};

/// Parameter definition from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct ParamDef {
    #[serde(rename = "type")]
    pub ptype: String,
    pub required: bool,
    pub description: String,
}

/// A single tool defined by a plugin.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginTool {
    pub domain: String,
    pub name: String,
    pub description: String,
    /// SKILL template with {param} placeholders.
    /// Example: `layoutSkill('align_pins ?dir {direction})`
    pub skill_template: String,
    #[serde(default)]
    pub params: HashMap<String, ParamDef>,
}

impl PluginTool {
    /// Convert plugin tool to MCP tool name.
    /// Format: `{domain}_{name}` e.g. "layout_align_pins"
    pub fn mcp_name(&self) -> String {
        format!("{}_{}", self.domain, self.name)
    }

    /// Convert plugin tool to RPC method name.
    /// Format: `{domain}.{name}` e.g. "layout.align_pins"
    pub fn rpc_method(&self) -> String {
        format!("{}.{}", self.domain, self.name)
    }

    /// Generate JSON Schema for the tool input from params.
    pub fn input_schema(&self) -> Value {
        let mut props = Map::new();
        let mut required = Vec::new();

        for (name, param) in &self.params {
            let mut prop = Map::new();
            prop.insert(
                "type".into(),
                Value::String(json_schema_type(&param.ptype).into()),
            );
            prop.insert(
                "description".into(),
                Value::String(param.description.clone()),
            );
            props.insert(name.clone(), Value::Object(prop));

            if param.required {
                required.push(name.clone());
            }
        }

        let mut schema = Map::new();
        schema.insert("type".into(), Value::String("object".into()));
        schema.insert("properties".into(), Value::Object(props));
        if !required.is_empty() {
            schema.insert(
                "required".into(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }
        Value::Object(schema)
    }

    /// Render the SKILL string by substituting type-safe {param} placeholders.
    pub fn render_skill(&self, args: &Map<String, Value>) -> Result<String> {
        self.validate()?;

        for name in args.keys() {
            if !self.params.contains_key(name) {
                return Err(VirtuosoError::Execution(format!(
                    "unknown parameter: {name}"
                )));
            }
        }

        for (name, def) in &self.params {
            let value = args.get(name);
            if def.required && value.is_none() {
                return Err(VirtuosoError::Execution(format!(
                    "missing required parameter: {name}"
                )));
            }
            if let Some(value) = value {
                Self::serialize_param(name, &def.ptype, value)?;
            }
        }

        self.render_template(args)
    }

    /// Parameter names are part of the template language even when a declared
    /// optional parameter is not used by this particular template.
    /// Validate plugin declarations before exposing them through RPC/MCP schemas.
    pub(crate) fn validate(&self) -> Result<()> {
        for name in self.params.keys() {
            if !is_parameter_identifier(name) {
                return Err(VirtuosoError::Execution(format!(
                    "invalid plugin parameter name '{name}': expected [A-Za-z_][A-Za-z0-9_]*"
                )));
            }
        }
        Ok(())
    }

    /// Render valid `{identifier}` tokens in a single pass. This deliberately
    /// never re-scans rendered values, so a string argument containing braces
    /// cannot become a second placeholder expansion.
    fn render_template(&self, args: &Map<String, Value>) -> Result<String> {
        let template = self.skill_template.as_str();
        let mut rendered = String::with_capacity(template.len());
        let mut index = 0;

        while index < template.len() {
            let character = template[index..]
                .chars()
                .next()
                .expect("index is within a UTF-8 string");
            if character != '{' {
                rendered.push(character);
                index += character.len_utf8();
                continue;
            }

            let token_start = index + 1;
            let Some(first) = template[token_start..].chars().next() else {
                rendered.push('{');
                break;
            };
            if !is_placeholder_start(first) {
                rendered.push('{');
                index += 1;
                continue;
            }

            let mut token_end = token_start + first.len_utf8();
            while let Some(next) = template[token_end..].chars().next() {
                if !is_placeholder_continue(next) {
                    break;
                }
                token_end += next.len_utf8();
            }

            if token_end == template.len() {
                return Err(VirtuosoError::Execution(
                    "malformed plugin parameter placeholder".into(),
                ));
            }
            if template[token_end..].starts_with('}') {
                let name = &template[token_start..token_end];
                let definition = self.params.get(name).ok_or_else(|| {
                    VirtuosoError::Execution(format!(
                        "unknown plugin parameter placeholder: {name}"
                    ))
                })?;
                let value = args.get(name).ok_or_else(|| {
                    VirtuosoError::Execution(format!("missing required parameter: {name}"))
                })?;
                rendered.push_str(&Self::serialize_param(name, &definition.ptype, value)?);
                index = token_end + 1;
            } else if template[token_end..].starts_with('-') {
                return Err(VirtuosoError::Execution(
                    "malformed plugin parameter placeholder".into(),
                ));
            } else {
                // This is a literal brace sequence (for example a JSON object),
                // not a plugin token. Keep scanning so nested valid tokens work.
                rendered.push('{');
                index += 1;
            }
        }

        Ok(rendered)
    }

    fn serialize_param(name: &str, ptype: &str, value: &Value) -> Result<String> {
        match (ptype, value) {
            ("string", Value::String(s)) => Ok(string_literal(s)),
            ("number", Value::Number(n)) => Ok(n.to_string()),
            ("integer", Value::Number(n)) if n.is_i64() || n.is_u64() => Ok(n.to_string()),
            ("boolean" | "bool", Value::Bool(true)) => Ok("t".into()),
            ("boolean" | "bool", Value::Bool(false)) => Ok("nil".into()),
            ("string" | "number" | "integer" | "boolean" | "bool", _) => Err(
                VirtuosoError::Execution(format!("parameter '{name}' must be {ptype}")),
            ),
            _ => Err(VirtuosoError::Execution(format!(
                "parameter '{name}' has unsupported type '{ptype}'"
            ))),
        }
    }
}

fn is_placeholder_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_'
}

fn is_placeholder_continue(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn is_parameter_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if is_placeholder_start(first) => characters.all(is_placeholder_continue),
        _ => false,
    }
}

fn json_schema_type(ptype: &str) -> &str {
    match ptype {
        "bool" => "boolean",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_name() {
        let tool = PluginTool {
            domain: "layout".into(),
            name: "align_pins".into(),
            description: "Test".into(),
            skill_template: "test()".into(),
            params: HashMap::new(),
        };
        assert_eq!(tool.mcp_name(), "layout_align_pins");
        assert_eq!(tool.rpc_method(), "layout.align_pins");
    }

    #[test]
    fn test_render_skill() {
        let mut params = HashMap::new();
        params.insert(
            "direction".into(),
            ParamDef {
                ptype: "string".into(),
                required: true,
                description: "Dir".into(),
            },
        );

        let tool = PluginTool {
            domain: "layout".into(),
            name: "align_pins".into(),
            description: "Test".into(),
            skill_template: "layoutSkill('align_pins ?dir {direction})".into(),
            params,
        };

        let mut args = Map::new();
        args.insert("direction".into(), Value::String("left".into()));

        let skill = tool.render_skill(&args).unwrap();
        assert_eq!(skill, "layoutSkill('align_pins ?dir \"left\")");
    }

    #[test]
    fn render_skill_quotes_string_values_to_prevent_skill_injection() {
        let mut params = HashMap::new();
        params.insert(
            "name".into(),
            ParamDef {
                ptype: "string".into(),
                required: true,
                description: "Name".into(),
            },
        );
        let tool = PluginTool {
            domain: "test".into(),
            name: "echo".into(),
            description: "Test".into(),
            skill_template: "echo({name})".into(),
            params,
        };
        let mut args = Map::new();
        args.insert(
            "name".into(),
            Value::String("x\") ; dangerousCall() ; (\"".into()),
        );

        assert_eq!(
            tool.render_skill(&args).unwrap(),
            r#"echo("x\") ; dangerousCall() ; (\"")"#
        );
    }

    #[test]
    fn render_skill_rejects_unknown_parameters_and_type_mismatches() {
        let mut params = HashMap::new();
        params.insert(
            "count".into(),
            ParamDef {
                ptype: "number".into(),
                required: true,
                description: "Count".into(),
            },
        );
        let tool = PluginTool {
            domain: "test".into(),
            name: "count".into(),
            description: "Test".into(),
            skill_template: "count({count})".into(),
            params,
        };

        let mut unknown = Map::new();
        unknown.insert("extra".into(), Value::Number(1.into()));
        assert!(tool.render_skill(&unknown).is_err());

        let mut wrong_type = Map::new();
        wrong_type.insert("count".into(), Value::String("1; dangerousCall()".into()));
        assert!(tool.render_skill(&wrong_type).is_err());
    }

    #[test]
    fn render_skill_rejects_missing_and_unreplaced_placeholders() {
        let mut params = HashMap::new();
        params.insert(
            "name".into(),
            ParamDef {
                ptype: "string".into(),
                required: true,
                description: "Name".into(),
            },
        );
        let tool = PluginTool {
            domain: "test".into(),
            name: "echo".into(),
            description: "Test".into(),
            skill_template: "echo({name}, {missing})".into(),
            params,
        };

        assert!(tool.render_skill(&Map::new()).is_err());

        let mut args = Map::new();
        args.insert("name".into(), Value::String("ok".into()));
        assert!(tool.render_skill(&args).is_err());
    }

    #[test]
    fn render_skill_serializes_booleans_as_skill_atoms() {
        let mut params = HashMap::new();
        params.insert(
            "enabled".into(),
            ParamDef {
                ptype: "boolean".into(),
                required: true,
                description: "Enabled".into(),
            },
        );
        let tool = PluginTool {
            domain: "test".into(),
            name: "toggle".into(),
            description: "Test".into(),
            skill_template: "toggle({enabled})".into(),
            params,
        };
        let mut args = Map::new();
        args.insert("enabled".into(), Value::Bool(true));
        assert_eq!(tool.render_skill(&args).unwrap(), "toggle(t)");
    }

    #[test]
    fn render_skill_preserves_braces_inside_rendered_string_values() {
        let tool = tool_with_params("echo({first}, {other})", &["first", "other"]);
        let mut args = Map::new();
        args.insert("first".into(), Value::String("{other}".into()));
        args.insert("other".into(), Value::String("safe".into()));

        assert_eq!(
            tool.render_skill(&args).unwrap(),
            r#"echo("{other}", "safe")"#
        );
    }

    #[test]
    fn render_skill_preserves_literal_json_braces_and_repeats_placeholders() {
        let tool = tool_with_params(
            r#"emit({"kind": "tag", "value": {name}, "again": {name}})"#,
            &["name"],
        );
        let mut args = Map::new();
        args.insert("name".into(), Value::String("signal".into()));

        assert_eq!(
            tool.render_skill(&args).unwrap(),
            r#"emit({"kind": "tag", "value": "signal", "again": "signal"})"#
        );
    }

    #[test]
    fn render_skill_rejects_unknown_and_malformed_template_placeholders() {
        let unknown = tool_with_params("echo({missing})", &["name"]);
        let malformed = tool_with_params("echo({bad-name})", &["name"]);
        let mut args = Map::new();
        args.insert("name".into(), Value::String("signal".into()));

        assert!(unknown.render_skill(&args).is_err());
        assert!(malformed.render_skill(&args).is_err());
    }

    #[test]
    fn render_skill_rejects_invalid_declared_parameter_names() {
        for name in ["1", "foo-bar", "参数"] {
            let tool = tool_with_params("echo()", &[name]);
            let mut args = Map::new();
            args.insert(name.into(), Value::String("value".into()));
            let error = tool.render_skill(&args).unwrap_err();
            assert!(error.to_string().contains("invalid plugin parameter name"));
        }
    }

    #[test]
    fn validate_rejects_invalid_declared_parameter_names_before_rendering() {
        let tool = tool_with_params("echo()", &["foo-bar"]);
        assert!(tool.validate().is_err());
    }

    #[test]
    fn render_skill_accepts_underscore_identifier_and_repeated_placeholders() {
        let tool = tool_with_params("echo({signal_1}, {signal_1})", &["signal_1"]);
        let mut args = Map::new();
        args.insert("signal_1".into(), Value::String("ok".into()));
        assert_eq!(tool.render_skill(&args).unwrap(), r#"echo("ok", "ok")"#);
    }

    #[test]
    fn input_schema_normalizes_bool_alias_to_boolean() {
        let mut params = HashMap::new();
        params.insert(
            "enabled".into(),
            ParamDef {
                ptype: "bool".into(),
                required: true,
                description: "Enabled".into(),
            },
        );
        let tool = PluginTool {
            domain: "test".into(),
            name: "toggle".into(),
            description: "Test".into(),
            skill_template: "toggle({enabled})".into(),
            params,
        };
        assert_eq!(
            tool.input_schema()["properties"]["enabled"]["type"],
            "boolean"
        );
    }

    fn tool_with_params(template: &str, names: &[&str]) -> PluginTool {
        let params = names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    ParamDef {
                        ptype: "string".into(),
                        required: true,
                        description: "Test".into(),
                    },
                )
            })
            .collect();
        PluginTool {
            domain: "test".into(),
            name: "render".into(),
            description: "Test".into(),
            skill_template: template.into(),
            params,
        }
    }

    #[test]
    fn test_input_schema() {
        let mut params = HashMap::new();
        params.insert(
            "direction".into(),
            ParamDef {
                ptype: "string".into(),
                required: true,
                description: "Pin direction".into(),
            },
        );

        let tool = PluginTool {
            domain: "layout".into(),
            name: "align_pins".into(),
            description: "Align pins".into(),
            skill_template: "test()".into(),
            params,
        };

        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["direction"].is_object());
        assert!(schema["required"].is_array());
    }

    #[test]
    fn test_toml_parsing_single_tool() {
        let toml_content = r#"
domain = "test"
name = "echo"
description = "Echo back"
skill_template = 'sprintf(nil "echo: {msg}")'

[params]
msg = { type = "string", required = true, description = "Message" }
"#;
        let tool: PluginTool = toml::from_str(toml_content).unwrap();
        assert_eq!(tool.domain, "test");
        assert_eq!(tool.name, "echo");
        assert_eq!(tool.params.len(), 1);
        assert!(tool.params.contains_key("msg"));
    }

    #[test]
    fn test_toml_parsing_inline_params() {
        // Test with inline table format (what users might actually write)
        let toml_content = r#"
domain = "test"
name = "hello"
description = "Hello"
skill_template = 'sprintf(nil "hello: {name}")'
params = { name = { type = "string", required = true, description = "Name" } }
"#;
        let tool: PluginTool = toml::from_str(toml_content).unwrap();
        assert_eq!(tool.params.len(), 1);
        assert!(tool.params.contains_key("name"));
    }
}
