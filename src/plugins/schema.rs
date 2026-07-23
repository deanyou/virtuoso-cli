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
            prop.insert("type".into(), Value::String(param.ptype.clone()));
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

        let mut skill = self.skill_template.clone();
        for (key, value) in args {
            let placeholder = format!("{{{}}}", key);
            let definition = &self.params[key];
            let replacement = Self::serialize_param(key, &definition.ptype, value)?;
            skill = skill.replace(&placeholder, &replacement);
        }

        if skill.contains('{') || skill.contains('}') {
            return Err(VirtuosoError::Execution(
                "unreplaced plugin parameter placeholder".into(),
            ));
        }

        Ok(skill)
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
