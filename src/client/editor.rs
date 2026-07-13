#![allow(dead_code)]

use crate::client::bridge::VirtuosoClient;
use crate::client::layout_ops::LayoutOps;
use crate::client::schematic_ops::SchematicOps;
use crate::error::Result;
use crate::models::VirtuosoResult;

pub struct LayoutEditor<'a> {
    client: &'a VirtuosoClient,
    lib: String,
    cell: String,
    commands: Vec<String>,
}

impl<'a> LayoutEditor<'a> {
    pub fn new(client: &'a VirtuosoClient, lib: &str, cell: &str) -> Self {
        Self {
            client,
            lib: lib.into(),
            cell: cell.into(),
            commands: Vec::new(),
        }
    }

    pub fn add_rect(&mut self, layer: &str, purpose: &str, bbox: [(i64, i64); 2]) {
        let ops = LayoutOps;
        self.commands.push(ops.create_rect(layer, purpose, &bbox));
    }

    pub fn add_polygon(&mut self, layer: &str, purpose: &str, points: Vec<(i64, i64)>) {
        let ops = LayoutOps;
        self.commands
            .push(ops.create_polygon(layer, purpose, &points));
    }

    pub fn add_path(&mut self, layer: &str, purpose: &str, width: i64, points: Vec<(i64, i64)>) {
        let ops = LayoutOps;
        self.commands
            .push(ops.create_path(layer, purpose, width, &points));
    }

    pub fn add_via(&mut self, via_def: &str, origin: (i64, i64)) {
        let ops = LayoutOps;
        self.commands.push(ops.create_via(via_def, origin));
    }

    pub fn add_instance(
        &mut self,
        lib: &str,
        cell: &str,
        view: &str,
        origin: (i64, i64),
        orient: &str,
    ) {
        let ops = LayoutOps;
        self.commands
            .push(ops.create_instance(lib, cell, view, origin, orient));
    }

    pub fn execute(self) -> Result<VirtuosoResult> {
        self.client.execute_operations(&self.commands)
    }
}

pub struct SchematicEditor<'a> {
    client: &'a VirtuosoClient,
    commands: Vec<String>,
}

impl<'a> SchematicEditor<'a> {
    pub fn new(client: &'a VirtuosoClient) -> Self {
        Self {
            client,
            commands: Vec::new(),
        }
    }

    pub fn add_instance(
        &mut self,
        lib: &str,
        cell: &str,
        view: &str,
        name: &str,
        origin: (i64, i64),
        orient: &str,
    ) {
        let ops = SchematicOps;
        self.commands
            .push(ops.create_instance(lib, cell, view, name, origin, orient));
    }

    pub fn add_wire(&mut self, points: Vec<(i64, i64)>, layer: &str, net_name: &str) {
        let ops = SchematicOps;
        self.commands
            .push(ops.create_wire(&points, layer, net_name));
    }

    pub fn add_label(&mut self, net_name: &str, origin: (i64, i64)) {
        let ops = SchematicOps;
        self.commands.push(ops.create_wire_label(net_name, origin));
    }

    pub fn add_pin(&mut self, net_name: &str, pin_type: &str, origin: (i64, i64)) {
        let ops = SchematicOps;
        self.commands
            .push(ops.create_pin(net_name, pin_type, origin));
    }

    pub fn set_param(&mut self, inst_name: &str, param: &str, value: &str) {
        let ops = SchematicOps;
        self.commands
            .push(ops.set_instance_param(inst_name, param, value));
    }

    pub fn assign_net(&mut self, inst_name: &str, term_name: &str, net_name: &str) {
        let ops = SchematicOps;
        self.commands
            .push(ops.assign_net(inst_name, term_name, net_name));
    }

    pub fn execute(self) -> Result<VirtuosoResult> {
        self.client.execute_operations(&self.commands)
    }
}
