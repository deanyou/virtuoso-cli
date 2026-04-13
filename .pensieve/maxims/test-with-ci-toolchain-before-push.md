# Test With CI Toolchain Before Push

## One-line Conclusion
> Run `cargo test && cargo clippy -- -D warnings` locally before every push; never assume `cargo fix` caught everything.

## Guidance
- `cargo fix` skips lints not present in current rustc version
- Nightly may lack lints that stable has (and vice versa)
- Tests that construct Config/model structs break when new fields are added — always update test helpers
- Mutex poisoning in env-var tests cascades across test suite — use `unwrap_or_else(|e| e.into_inner())`

## Boundaries
- Trivial doc-only changes can skip this

## Context Links
- Based on: [[ci-must-match-target-toolchain]]
- Related: [[rust-bin-vs-lib-crate-attrs]]
