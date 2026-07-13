//! Plugin schema definitions — TOML parsing structures.

use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

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

    /// Render the SKILL string by substituting {param} placeholders.
    pub fn render_skill(&self, args: &Map<String, Value>) -> String {
        let mut skill = self.skill_template.clone();
        for (key, value) in args {
            let placeholder = format!("{{{}}}", key);
            let replacement = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                other => other.to_string(),
            };
            skill = skill.replace(&placeholder, &replacement);
        }
        skill
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

        let skill = tool.render_skill(&args);
        assert_eq!(skill, "layoutSkill('align_pins ?dir left)");
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
