use std::collections::HashMap;
use std::fs;

use crate::client::bridge::VirtuosoClient;
use crate::client::editor::SchematicEditor;
use crate::error::{Result, VirtuosoError};
use serde::Deserialize;
use serde_json::{json, Value};

/// Cadence symbol orientation. Exactly the 8 values SKILL accepts.
#[derive(Debug, Clone, Copy, Deserialize, clap::ValueEnum)]
#[clap(rename_all = "verbatim")]
pub enum Orient {
    R0,
    R90,
    R180,
    R270,
    MX,
    MY,
    MXR90,
    MYR90,
}

impl Orient {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::R0 => "R0",
            Self::R90 => "R90",
            Self::R180 => "R180",
            Self::R270 => "R270",
            Self::MX => "MX",
            Self::MY => "MY",
            Self::MXR90 => "MXR90",
            Self::MYR90 => "MYR90",
        }
    }
}

// ── Atomic commands ─────────────────────────────────────────────────

pub fn open(lib: &str, cell: &str, view: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.open_cellview(lib, cell, view);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "lib": lib, "cell": cell, "view": view,
        "output": r.output,
    }))
}

pub fn place(
    master: &str,
    name: &str,
    x: i64,
    y: i64,
    orient: Orient,
    params: &[(String, String)],
) -> Result<Value> {
    let (lib, cell) = master
        .split_once('/')
        .ok_or_else(|| VirtuosoError::Config("--master must be lib/cell format".into()))?;
    let client = VirtuosoClient::from_env()?;
    let mut ed = SchematicEditor::new(&client);
    ed.add_instance(lib, cell, "symbol", name, (x, y), orient.as_str());
    for (k, v) in params {
        ed.set_param(name, k, v);
    }
    let r = ed.execute()?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "instance": name, "master": master,
        "output": r.output,
    }))
}

pub fn wire_from_strings(net: &str, points: &[String]) -> Result<Value> {
    let pts: Vec<(i64, i64)> = points
        .iter()
        .map(|s| {
            let (x, y) = s
                .split_once(',')
                .ok_or_else(|| VirtuosoError::Config(format!("Point '{s}' must be x,y")))?;
            Ok((
                x.parse()
                    .map_err(|_| VirtuosoError::Config(format!("Bad x: {x}")))?,
                y.parse()
                    .map_err(|_| VirtuosoError::Config(format!("Bad y: {y}")))?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    wire(net, &pts)
}

pub fn wire(net: &str, points: &[(i64, i64)]) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.create_wire(points, "wire", net);
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "net": net, "output": r.output,
    }))
}

pub fn conn(net: &str, from: &str, to: &str) -> Result<Value> {
    let (inst1, term1) = from
        .split_once(':')
        .ok_or_else(|| VirtuosoError::Config("--from must be inst:term format".into()))?;
    let (inst2, term2) = to
        .split_once(':')
        .ok_or_else(|| VirtuosoError::Config("--to must be inst:term format".into()))?;
    let client = VirtuosoClient::from_env()?;
    let mut ed = SchematicEditor::new(&client);
    ed.assign_net(inst1, term1, net);
    ed.assign_net(inst2, term2, net);
    let r = ed.execute()?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "net": net, "from": from, "to": to,
        "output": r.output,
    }))
}

pub fn label(net: &str, x: i64, y: i64) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.create_wire_label(net, (x, y));
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "net": net, "output": r.output,
    }))
}

pub fn pin(net: &str, pin_type: &str, x: i64, y: i64) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.create_pin(net, pin_type, (x, y));
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "net": net, "type": pin_type, "output": r.output,
    }))
}

pub fn check() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.check();
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "output": r.output,
    }))
}

pub fn save() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.save();
    let r = client.execute_skill(&skill, None)?;
    Ok(json!({
        "status": if r.skill_ok() { "success" } else { "error" },
        "output": r.output,
    }))
}

// ── Build (batch from JSON spec) ────────────────────────────────────

#[derive(Deserialize)]
pub struct SchematicSpec {
    pub target: SpecTarget,
    #[serde(default)]
    pub instances: Vec<SpecInstance>,
    #[serde(default)]
    pub connections: Vec<SpecConnection>,
    #[serde(default)]
    pub globals: Vec<SpecGlobal>,
    #[serde(default)]
    pub pins: Vec<SpecPin>,
}

#[derive(Deserialize)]
pub struct SpecTarget {
    pub lib: String,
    pub cell: String,
    #[serde(default = "default_view")]
    pub view: String,
}

fn default_view() -> String {
    "schematic".into()
}

#[derive(Deserialize)]
pub struct SpecInstance {
    pub name: String,
    pub master: String, // "lib/cell"
    #[serde(default)]
    pub x: i64,
    #[serde(default)]
    pub y: i64,
    #[serde(default = "default_orient")]
    pub orient: Orient,
    #[serde(default)]
    pub params: HashMap<String, String>,
}

fn default_orient() -> Orient {
    Orient::R0
}

#[derive(Deserialize)]
pub struct SpecConnection {
    pub net: String,
    pub from: String, // "inst:term"
    pub to: String,
}

#[derive(Deserialize)]
pub struct SpecGlobal {
    pub net: String,
    pub insts: Vec<String>, // ["M5:S", "M5:B"]
}

#[derive(Deserialize)]
pub struct SpecPin {
    pub net: String,
    #[serde(rename = "type")]
    pub pin_type: String,
    #[serde(default)]
    pub connect: Option<String>, // "M2:G"
    #[serde(default)]
    pub x: i64,
    #[serde(default)]
    pub y: i64,
}

pub fn build(spec_path: &str) -> Result<Value> {
    let spec_str = fs::read_to_string(spec_path)
        .map_err(|e| VirtuosoError::Config(format!("Cannot read spec file {spec_path}: {e}")))?;
    let spec: SchematicSpec = serde_json::from_str(&spec_str)
        .map_err(|e| VirtuosoError::Config(format!("Invalid spec JSON: {e}")))?;

    let client = VirtuosoClient::from_env()?;

    // 1. Open/create cellview
    let open_skill =
        client
            .schematic
            .open_cellview(&spec.target.lib, &spec.target.cell, &spec.target.view);
    let r = client.execute_skill(&open_skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to open cellview: {}",
            r.output
        )));
    }

    // 2. Place instances + set params
    let mut ed = SchematicEditor::new(&client);
    for inst in &spec.instances {
        let (lib, cell) = inst.master.split_once('/').ok_or_else(|| {
            VirtuosoError::Config(format!(
                "Instance {} master '{}' must be lib/cell",
                inst.name, inst.master
            ))
        })?;
        ed.add_instance(lib, cell, "symbol", &inst.name, (inst.x, inst.y), inst.orient.as_str());
        for (k, v) in &inst.params {
            ed.set_param(&inst.name, k, v);
        }
    }
    let r = ed.execute()?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to place instances: {}",
            r.output
        )));
    }

    // 3. Connections — generate .il script with RB_connectTerminal calls
    //    Bridge has limitations with complex SKILL, so we write to file and load().
    {
        let mut assignments: Vec<(String, String, String)> = Vec::new();
        for c in &spec.connections {
            let (i1, t1) = c.from.split_once(':').ok_or_else(|| {
                VirtuosoError::Config(format!("Bad from '{}' in connection", c.from))
            })?;
            let (i2, t2) = c
                .to
                .split_once(':')
                .ok_or_else(|| VirtuosoError::Config(format!("Bad to '{}' in connection", c.to)))?;
            assignments.push((i1.into(), t1.into(), c.net.clone()));
            assignments.push((i2.into(), t2.into(), c.net.clone()));
        }
        for g in &spec.globals {
            for inst_term in &g.insts {
                let (inst, term) = inst_term.split_once(':').ok_or_else(|| {
                    VirtuosoError::Config(format!("Bad global '{}' in {}", inst_term, g.net))
                })?;
                assignments.push((inst.into(), term.into(), g.net.clone()));
            }
        }

        // Load the RB_connectTerminal helper procedure
        let helper_path = "/tmp/rb_schematic_helper.il";
        fs::write(
            helper_path,
            include_str!("../../resources/rb_connect_terminal.il"),
        )
        .map_err(|e| VirtuosoError::Config(format!("Cannot write helper: {e}")))?;
        let r = client.execute_skill(&format!(r#"load("{helper_path}")"#), None)?;
        if !r.skill_ok() {
            return Err(VirtuosoError::Execution(format!(
                "Failed to load connection helper: {}",
                r.output
            )));
        }

        // Generate connection script
        let mut lines = vec!["let((cv)".to_string(), "cv = RB_SCH_CV".to_string()];
        for (inst, term, net) in &assignments {
            lines.push(format!(
                r#"RB_connectTerminal(cv "{inst}" "{term}" "{net}")"#
            ));
        }
        lines.push("t)".to_string());

        let script_path = "/tmp/rb_schematic_conn.il";
        fs::write(script_path, lines.join("\n"))
            .map_err(|e| VirtuosoError::Config(format!("Cannot write script: {e}")))?;
        let r = client.execute_skill(&format!(r#"load("{script_path}")"#), None)?;
        if !r.skill_ok() {
            return Err(VirtuosoError::Execution(format!(
                "Failed to create connections: {}",
                r.output
            )));
        }
    }

    // 4. Pins
    if !spec.pins.is_empty() {
        let mut ed = SchematicEditor::new(&client);
        for p in &spec.pins {
            ed.add_pin(&p.net, &p.pin_type, (p.x, p.y));
        }
        let r = ed.execute()?;
        if !r.skill_ok() {
            return Err(VirtuosoError::Execution(format!(
                "Failed to create pins: {}",
                r.output
            )));
        }
    }

    // 5. Save + check
    let save_skill = client.schematic.save();
    client.execute_skill(&save_skill, None)?;
    let check_skill = client.schematic.check();
    let r = client.execute_skill(&check_skill, None)?;

    Ok(json!({
        "status": "success",
        "target": format!("{}/{}/{}", spec.target.lib, spec.target.cell, spec.target.view),
        "instances": spec.instances.len(),
        "connections": spec.connections.len() + spec.globals.len(),
        "pins": spec.pins.len(),
        "check": r.output,
    }))
}

// ── Read commands ───────────────────────────────────────────────────

/// Parse SKILL JSON output: bridge returns `"\"[...]\""`  — strip outer quotes, unescape inner.
/// Returns `Err` if the output cannot be parsed as JSON after unescaping.
pub fn parse_skill_json(output: &str) -> Result<Value> {
    // output is like: "\"[{\\\"name\\\":\\\"M1\\\"}]\""
    // Step 1: strip outer quotes from SKILL string
    let s = output.trim_matches('"');
    // Step 2: try parsing directly (works if no extra escaping)
    if let Ok(v) = serde_json::from_str(s) {
        return Ok(v);
    }
    // Step 3: unescape \" → " and \\\\ → \ then retry
    let unescaped = s.replace("\\\"", "\"").replace("\\\\", "\\");
    serde_json::from_str(&unescaped).map_err(|e| {
        VirtuosoError::Execution(format!(
            "Failed to parse SKILL JSON output: {e}. Raw: {output}"
        ))
    })
}

pub fn list_instances() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.list_instances();
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to list instances: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

pub fn list_nets() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.list_nets();
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to list nets: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

pub fn list_pins() -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.list_pins();
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to list pins: {}",
            r.output
        )));
    }
    parse_skill_json(&r.output)
}

pub fn get_params(inst: &str) -> Result<Value> {
    let client = VirtuosoClient::from_env()?;
    let skill = client.schematic.get_instance_params(inst);
    let r = client.execute_skill(&skill, None)?;
    if !r.skill_ok() {
        return Err(VirtuosoError::Execution(format!(
            "Failed to get params for '{}': {}",
            inst, r.output
        )));
    }
    Ok(json!({"instance": inst, "params": parse_skill_json(&r.output)?}))
}
