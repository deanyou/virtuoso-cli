# Upstream Hardening Design

## Scope

This change hardens the newly pulled Maestro `dec` injection path, restores the repository's Clippy gate, and adds an opt-in strict PSF access layer. It does not add remote-host roles, CIW bootstrap, waveform export, new CLI commands, or new dependencies.

## Maestro command safety

`MaestroOps::run_with_dec` must retain the current IC25 behavior: configure the selected analysis, save the setup, insert `dec=<N>` into the generated AC netlist, and launch the simulation in one privileged SKILL expression.

The generated shell command must quote every dynamic path as a POSIX single-quoted shell word. The generated SKILL program will define a local quoting procedure that surrounds a value with single quotes and replaces each embedded single quote with the POSIX sequence (`'"'"'`). It applies this procedure to `netPath` before constructing `sed` and `grep` commands. Numeric `dec` remains a `u32` and is not interpreted as text.

The command layer must not write the privileged SKILL program to `/tmp` or any other diagnostic file. Failure continues to propagate through `ok_or_exec`; this task does not change capability or whitelist policy.

Tests must verify that the generated SKILL template defines a POSIX shell-word quoting procedure, applies it to the runtime-derived `netPath`, and never interpolates raw `netPath` operands into `sed` or `grep`. Rust unit tests cannot execute a real runtime path containing spaces or apostrophes because that value exists only inside Virtuoso; live-path execution is outside this task. Existing Maestro tests remain compatible.

## Clippy gate restoration

Replace the seven string parameters of `LayoutOps::xstream_out` with an `XstreamOutRequest<'a>` value while preserving the generated SKILL expression and all escaping. Update its existing callers and tests.

Use the unit struct `LibraryOps` directly instead of constructing it through `Default`. Collapse the nested empty-window/helper-error condition in `src/transport/x11.rs`, and replace the boolean equality assertion in `src/spectre/runner.rs` with a direct negated assertion. These four lint sites are the complete baseline reported by `cargo clippy --all-targets --all-features -- -D warnings`; no unrelated lint cleanup is in scope.

## Strict PSF access layer

The legacy `parse_psf_ascii` and sweep parsers remain unchanged and available. New strict APIs live in `src/spectre/parsers.rs`:

```rust
pub struct PsfDataset {
    signals: HashMap<String, Vec<f64>>,
}

pub fn read_psf_ascii_strict(path: &Path) -> Result<PsfDataset>;
pub fn find_exact_result_file(root: &Path, relative_name: &Path) -> Result<PathBuf>;

impl PsfDataset {
    pub fn require_scalar(&self, key: &str) -> Result<f64>;
    pub fn require_vector(&self, key: &str) -> Result<&[f64]>;
    pub fn require_frequency_hz(&self, key: &str) -> Result<&[f64]>;
}
```

`read_psf_ascii_strict` accepts one regular file. It returns an explicit error for unreadable input, empty input, malformed numeric lines, empty vectors, or non-finite values. It supports the two formats already recognized by the legacy parser: one numeric value per non-comment line, and a `SWEEP` section with an optional quoted header followed by numeric values. It derives the signal key from the file stem.

`find_exact_result_file` accepts a root directory and a relative file name, not a glob. It rejects absolute paths, `..`, symlinks, non-files, and any canonical result outside the canonical root. It returns the canonicalized contained file path, and callers must open that returned path. The check-to-open race remains a known residual risk acceptable for a local CLI reading simulation output. Missing result files and missing dataset keys use `VirtuosoError::NotFound`; unsafe path input uses `VirtuosoError::Config`; malformed data, invalid cardinality, and invalid numeric values use `VirtuosoError::Execution`; filesystem failures retain `VirtuosoError::Io`.

`require_scalar` requires exactly one finite value. `require_vector` requires an existing, non-empty, all-finite vector. `require_frequency_hz` additionally requires every value to be non-negative and strictly increasing.

## Validation

Each behavior is developed test-first. Acceptance requires:

```bash
cargo test --lib client::maestro_ops
cargo test --lib spectre::parsers
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

No commit or push is part of implementation.
