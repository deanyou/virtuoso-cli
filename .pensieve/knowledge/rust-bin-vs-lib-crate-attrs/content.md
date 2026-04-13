# Rust: bin crate `#![allow]` is separate from lib crate

## Source
CI failure in virtuoso-cli: `#![allow(dead_code)]` in `lib.rs` did not suppress dead_code errors in `vcli` binary.

## Summary
When a Rust project has both `lib.rs` and `main.rs` with `mod` declarations, they are separate crate roots — crate-level attributes in one do not affect the other.

## Content
virtuoso-cli has:
- `src/lib.rs` — library crate, re-exports modules
- `src/main.rs` — binary crate (`vcli`), uses `mod client; mod commands;` etc.

Both compile the same source files but as **separate crates**. Adding `#![allow(dead_code)]` to `lib.rs` only suppresses warnings in the lib crate. The bin crate (`main.rs`) does its own dead_code analysis independently.

**Fix**: Add `#![allow(dead_code)]` to both `lib.rs` and `main.rs` if scaffolding APIs exist that aren't yet wired to commands.

## When to Use
- When crate-level attributes seem to have no effect on compilation warnings
- When CI reports dead_code errors that don't appear locally (different crate root being checked)
