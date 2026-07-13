---
name: unit-test-agent
description: Run and verify all unit tests and integration tests
tools:
  - Bash
  - Read
---

# Unit Test Agent

Run comprehensive tests for the virtuoso-cli project.

## Test Strategy

1. **Unit Tests**: `cargo test` in the project root
2. **Integration Tests**: Test CLI commands against live Virtuoso session
3. **Clippy**: `cargo clippy -- -D warnings`
4. **Format Check**: `cargo fmt --check`

## Test Categories

### Unit Tests
- Test all modules with `#[cfg(test)]` sections
- Mock any external dependencies (Virtuoso, file system)
- Verify error paths

### Integration Tests
- Test against real Virtuoso session at `meowu-meow-32851` on port 32851
- Use `VCLI_CAPABILITY=admin,schematic,maestro,window,cell,simulation,transaction`
- Test all new RPC methods

### Code Quality
- Clippy with warnings as errors
- Format compliance

## Expected Results

```
cargo test: all tests pass
cargo clippy: 0 warnings  
cargo fmt: clean
integration tests: all commands work
```

## Output Format

Report:
- Number of tests passed/failed
- Any test failures with details
- Clippy warnings count
- Integration test results
