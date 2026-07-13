//! Schematic diff — compute and represent changes between two snapshots.

use crate::transaction::snapshot::SchematicSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Structured diff between two SchematicSnapshots.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchematicDiff {
    pub before_id: String,
    pub after_id: String,
    pub instances_added: Vec<String>,
    pub instances_removed: Vec<String>,
    /// name → {param: {old, new}}
    pub instances_modified: Vec<InstanceModify>,
    pub nets_added: Vec<String>,
    pub nets_removed: Vec<String>,
    pub pins_added: Vec<String>,
    pub pins_removed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceModify {
    pub name: String,
    pub changes: HashMap<String, ParamChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamChange {
    pub old: String,
    pub new: String,
}

impl SchematicDiff {
    /// Compute the diff between two snapshots.
    pub fn compute(before: &SchematicSnapshot, after: &SchematicSnapshot) -> Self {
        let before_inst: HashMap<&str, _> = before
            .instances
            .iter()
            .map(|i| (i.name.as_str(), i))
            .collect();
        let after_inst: HashMap<&str, _> = after
            .instances
            .iter()
            .map(|i| (i.name.as_str(), i))
            .collect();

        let instances_added: Vec<String> = after_inst
            .keys()
            .filter(|k| !before_inst.contains_key(*k))
            .map(|s| s.to_string())
            .collect();

        let instances_removed: Vec<String> = before_inst
            .keys()
            .filter(|k| !after_inst.contains_key(*k))
            .map(|s| s.to_string())
            .collect();

        let instances_modified: Vec<InstanceModify> = before_inst
            .keys()
            .filter(|k| after_inst.contains_key(*k))
            .filter_map(|k| {
                let b = before_inst[k];
                let a = after_inst[*k];
                let mut changes = HashMap::new();
                if (b.x - a.x).abs() > 1e-6 || (b.y - a.y).abs() > 1e-6 {
                    changes.insert(
                        "position".to_string(),
                        ParamChange {
                            old: format!("({},{})", b.x, b.y),
                            new: format!("({},{})", a.x, a.y),
                        },
                    );
                }
                if b.orient != a.orient {
                    changes.insert(
                        "orient".to_string(),
                        ParamChange {
                            old: b.orient.clone(),
                            new: a.orient.clone(),
                        },
                    );
                }
                if !changes.is_empty() {
                    Some(InstanceModify {
                        name: k.to_string(),
                        changes,
                    })
                } else {
                    None
                }
            })
            .collect();

        let nets_added: Vec<String> = after
            .nets
            .iter()
            .filter(|n| !before.nets.contains(n))
            .cloned()
            .collect();

        let nets_removed: Vec<String> = before
            .nets
            .iter()
            .filter(|n| !after.nets.contains(n))
            .cloned()
            .collect();

        let pins_added: Vec<String> = after
            .pins
            .iter()
            .filter(|p| !before.pins.iter().any(|bp| bp.name == p.name))
            .map(|p| p.name.clone())
            .collect();

        let pins_removed: Vec<String> = before
            .pins
            .iter()
            .filter(|p| !after.pins.iter().any(|ap| ap.name == p.name))
            .map(|p| p.name.clone())
            .collect();

        Self {
            before_id: before.transaction_id.clone(),
            after_id: after.transaction_id.clone(),
            instances_added,
            instances_removed,
            instances_modified,
            nets_added,
            nets_removed,
            pins_added,
            pins_removed,
        }
    }

    /// True if there are no changes.
    pub fn is_empty(&self) -> bool {
        self.instances_added.is_empty()
            && self.instances_removed.is_empty()
            && self.instances_modified.is_empty()
            && self.nets_added.is_empty()
            && self.nets_removed.is_empty()
            && self.pins_added.is_empty()
            && self.pins_removed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::snapshot::{InstanceSnapshot, SchematicSnapshot};
    use std::collections::HashMap;

    fn make_snapshot(
        id: &str,
        instances: Vec<InstanceSnapshot>,
        nets: Vec<&str>,
    ) -> SchematicSnapshot {
        SchematicSnapshot {
            transaction_id: id.into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            lib: "myLib".into(),
            cell: "myCell".into(),
            view: "schematic".into(),
            instances,
            nets: nets.into_iter().map(String::from).collect(),
            pins: vec![],
        }
    }

    #[test]
    fn diff_detects_added_instance() {
        let before = make_snapshot("before", vec![], vec!["VSS"]);
        let after = make_snapshot(
            "after",
            vec![InstanceSnapshot {
                name: "M1".into(),
                lib: "analogLib".into(),
                cell: "nmos4".into(),
                x: 0.0,
                y: 0.0,
                orient: "R0".into(),
                params: HashMap::new(),
            }],
            vec!["VSS", "VDD"],
        );
        let diff = SchematicDiff::compute(&before, &after);
        assert!(diff.instances_added.contains(&"M1".into()));
        assert!(diff.nets_added.contains(&"VDD".into()));
        assert!(!diff.is_empty());
    }

    #[test]
    fn diff_detects_removed_instance() {
        let before = make_snapshot(
            "before",
            vec![InstanceSnapshot {
                name: "M1".into(),
                lib: "analogLib".into(),
                cell: "nmos4".into(),
                x: 0.0,
                y: 0.0,
                orient: "R0".into(),
                params: HashMap::new(),
            }],
            vec!["VSS"],
        );
        let after = make_snapshot("after", vec![], vec!["VSS"]);
        let diff = SchematicDiff::compute(&before, &after);
        assert!(diff.instances_removed.contains(&"M1".into()));
    }

    #[test]
    fn diff_no_change() {
        let snap = make_snapshot(
            "snap",
            vec![InstanceSnapshot {
                name: "M1".into(),
                lib: "analogLib".into(),
                cell: "nmos4".into(),
                x: 0.0,
                y: 0.0,
                orient: "R0".into(),
                params: HashMap::new(),
            }],
            vec!["VSS"],
        );
        let diff = SchematicDiff::compute(&snap, &snap);
        assert!(diff.is_empty());
    }
}
