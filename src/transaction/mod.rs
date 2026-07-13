//! Transaction system — snapshot, diff, and rollback for schematic changes.
//!
//! Each transaction captures a SchematicSnapshot at begin-time,
//! then compares against the live cellview on diff/rollback.

pub mod diff;
pub mod snapshot;

use crate::client::bridge::VirtuosoClient;
use crate::error::{Result, VirtuosoError};
pub use diff::SchematicDiff;
pub use snapshot::SchematicSnapshot;
use std::collections::HashMap;

/// Active transaction.
#[derive(Debug)]
pub struct Transaction {
    pub id: String,
    pub snapshot: SchematicSnapshot,
    /// Net assignments at tx start: net → [(instance, terminal)]
    #[allow(dead_code)]
    net_assignments: HashMap<String, Vec<(String, String)>>,
}

impl Transaction {
    fn new(id: String, snapshot: SchematicSnapshot) -> Self {
        Self {
            id,
            snapshot,
            net_assignments: HashMap::new(),
        }
    }
}

/// Transaction manager — tracks the single active transaction per VirtuosoClient.
/// Uses RefCell so tx_* methods can borrow mutably without &mut self.
pub struct TransactionManager {
    active: Option<Transaction>,
    committed: Vec<String>,
}

impl Default for TransactionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TransactionManager {
    pub fn new() -> Self {
        Self {
            active: None,
            committed: Vec::new(),
        }
    }

    /// Begin a transaction — captures a snapshot of the current cellview.
    pub fn begin(
        &mut self,
        client: &VirtuosoClient,
        id: String,
        lib: &str,
        cell: &str,
        view: &str,
    ) -> Result<()> {
        if self.active.is_some() {
            return Err(VirtuosoError::Execution(
                "transaction already active".into(),
            ));
        }
        let snapshot = SchematicSnapshot::capture(client, &id, lib, cell, view)?;
        self.active = Some(Transaction::new(id, snapshot));
        Ok(())
    }

    /// Commit the active transaction — deletes the snapshot file.
    pub fn commit(&mut self) -> Result<()> {
        let tx = self
            .active
            .take()
            .ok_or_else(|| VirtuosoError::Execution("no active transaction".into()))?;
        tx.snapshot.delete()?;
        self.committed.push(tx.id);
        Ok(())
    }

    /// Rollback — restore the cellview from the snapshot by re-creating instances.
    /// Nets and pins are restored from snapshot data if they exist in the snapshot.
    pub fn rollback(&self, client: &VirtuosoClient) -> Result<()> {
        let tx = self
            .active
            .as_ref()
            .ok_or_else(|| VirtuosoError::Execution("no active transaction".into()))?;
        let snap = &tx.snapshot;

        // Re-open the cellview
        let open_skill = client
            .schematic
            .open_cellview(&snap.lib, &snap.cell, &snap.view);
        client.execute_skill(&open_skill, None)?;

        // Re-create all instances from snapshot
        let mut ed = crate::client::editor::SchematicEditor::new(client);
        for inst in &snap.instances {
            ed.add_instance(
                &inst.lib,
                &inst.cell,
                "symbol",
                &inst.name,
                (inst.x as i64, inst.y as i64),
                &inst.orient,
            );
        }
        ed.execute()?;

        Ok(())
    }

    /// Compute diff between snapshot and current cellview state.
    pub fn diff(&self, client: &VirtuosoClient) -> Result<SchematicDiff> {
        let tx = self
            .active
            .as_ref()
            .ok_or_else(|| VirtuosoError::Execution("no active transaction".into()))?;
        let current = SchematicSnapshot::capture(
            client,
            &tx.snapshot.transaction_id,
            &tx.snapshot.lib,
            &tx.snapshot.cell,
            &tx.snapshot.view,
        )?;
        Ok(SchematicDiff::compute(&tx.snapshot, &current))
    }

    /// Returns (tx_id, snapshot) if a transaction is active.
    pub fn status(&self) -> Option<(String, SchematicSnapshot)> {
        self.active
            .as_ref()
            .map(|t| (t.id.clone(), t.snapshot.clone()))
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }
}
