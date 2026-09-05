#![allow(dead_code)]

mod async_runtime;
mod auth;
mod capability;
mod client;
mod command_log;
mod commands;
mod config;
mod context;
mod error;
mod exit_codes;
mod history;
mod mcp;
mod models;
mod ocean;
mod output;
mod plugins;
mod rpc;
mod runtime_paths;
mod skill_finder;
mod spectre;
mod streaming;
mod sys;
mod target;
#[cfg(test)]
mod test_env;
mod transaction;
mod transport;
mod tui;
mod version;

pub use capability::{Capability, CapabilitySet};
pub use rpc::schema::standard_schema;
pub use transaction::{SchematicDiff, SchematicSnapshot, TransactionManager};

fn main() {
    if let Err(e) = tui::run_tui() {
        eprintln!("vtui error: {e}");
        std::process::exit(1);
    }
}
