//! Typed RPC layer — method/params dispatch replacing raw SKILL strings.
//!
//! Instead of:
//!   `vcli skill exec 'geOpenCellView(?libName "myLib" ...)'`
//!
//! Users call:
//!   `vcli rpc call schematic.open_cell_view '{"lib":"myLib","cell":"myCell","view":"schematic"}'`
//!
//! The dispatcher maps `{method, params}` → SKILL expression, executes, returns typed JSON.
//! This gives AI agents discoverable schema without SKILL knowledge.

pub mod dispatcher;
pub mod schema;
