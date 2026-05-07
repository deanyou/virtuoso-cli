//! Schematic snapshot — captures a point-in-time view of a cellview.

use crate::client::bridge::VirtuosoClient;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Snapshot of a schematic cellview at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchematicSnapshot {
    pub transaction_id: String,
    pub timestamp: String,
    pub lib: String,
    pub cell: String,
    pub view: String,
    pub instances: Vec<InstanceSnapshot>,
    pub nets: Vec<String>,
    pub pins: Vec<PinSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceSnapshot {
    pub name: String,
    pub lib: String,
    pub cell: String,
    pub x: f64,
    pub y: f64,
    pub orient: String,
    #[serde(default)]
    pub params: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinSnapshot {
    pub name: String,
    pub direction: String,
}

impl SchematicSnapshot {
    /// Capture a snapshot of the currently open cellview.
    pub fn capture(
        _client: &VirtuosoClient,
        transaction_id: &str,
        lib: &str,
        cell: &str,
        view: &str,
    ) -> Result<Self> {
        let inst_val = crate::commands::schematic::list_instances()?;
        let nets_val = crate::commands::schematic::list_nets()?;
        let pins_val = crate::commands::schematic::list_pins()?;

        let instances: Vec<InstanceSnapshot> = inst_val
            .as_array()
            .ok_or_else(|| crate::error::VirtuosoError::Execution("instances not an array".into()))?
            .iter()
            .filter_map(|v| {
                let name = v["name"].as_str()?;
                let master = v["master"].as_str()?;
                let (lib_cell, _view_name) = master.split_once('/')?;
                let (lib, cell) = lib_cell.rsplit_once('/').unwrap_or((lib, cell));
                Some(InstanceSnapshot {
                    name: name.to_string(),
                    lib: lib.to_string(),
                    cell: cell.to_string(),
                    x: v["x"].as_f64().unwrap_or(0.0),
                    y: v["y"].as_f64().unwrap_or(0.0),
                    orient: v
                        .get("orient")
                        .and_then(|o| o.as_str())
                        .unwrap_or("R0")
                        .to_string(),
                    params: HashMap::new(),
                })
            })
            .collect();

        let nets: Vec<String> = nets_val
            .as_array()
            .ok_or_else(|| crate::error::VirtuosoError::Execution("nets not an array".into()))?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();

        let pins: Vec<PinSnapshot> = pins_val
            .as_array()
            .ok_or_else(|| crate::error::VirtuosoError::Execution("pins not an array".into()))?
            .iter()
            .filter_map(|v| {
                Some(PinSnapshot {
                    name: v["name"].as_str()?.to_string(),
                    direction: v
                        .get("direction")
                        .and_then(|d| d.as_str())
                        .unwrap_or("input")
                        .to_string(),
                })
            })
            .collect();

        Ok(Self {
            transaction_id: transaction_id.into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            lib: lib.into(),
            cell: cell.into(),
            view: view.into(),
            instances,
            nets,
            pins,
        })
    }

    /// Save snapshot atomically: write to .tmp then rename.
    pub fn save(&self) -> std::io::Result<()> {
        let path = self.path();
        let json =
            serde_json::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, &path)
    }

    /// Delete the snapshot file from disk.
    pub fn delete(&self) -> std::io::Result<()> {
        std::fs::remove_file(self.path())
    }

    fn path(&self) -> PathBuf {
        Self::path_for(&self.transaction_id)
    }

    fn path_for(transaction_id: &str) -> PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("virtuoso_bridge")
            .join("snapshots")
            .join(format!("{transaction_id}.json"))
    }
}
