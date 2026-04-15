# Changelog

All notable changes to this project will be documented in this file.

## [0.1.4] - 2026-04-15

### Added
- **`vcli window` subcommand group** — `list`, `dismiss-dialog`, `screenshot`
  - `list`: enumerate all open Virtuoso windows with derived mode labels (`ade-editing`, `ade-reading`, `schematic`, `layout`, `other`); handles SKILL octal escapes (`\256` = ®) that break standard JSON parsers
  - `dismiss-dialog [--action ok|cancel] [--dry-run]`: programmatically cancel or confirm a blocking GUI dialog
  - `screenshot --path FILE [--window PATTERN]`: capture via X11 ImageMagick `import -window root` (IC23.1 fallback — `hiGetWindowScreenDump` is IC25+ only)
- **`vcli maestro set-analysis`** — enable an analysis type (ac/dc/tran/noise/…) on a setup by session name; resolves setup internally via `maeGetSetup`

### Fixed
- **`maestro add-output`** — parameter order was completely wrong: session ID was passed as SKILL output name and user-supplied name as setup name, causing `maeAddOutput` to always return nil; now resolves setup from session automatically
- **`maestro get-analyses`** — `maeGetEnabledAnalysis` takes a positional setup name (not `?session` keyword) in IC23.1; setup name is now resolved via `maeGetSetup` internally
- **`--session` global arg no longer clobbers `VB_SESSION`** — bridge session ID and Maestro session name can coexist without conflict

## [0.1.3] - 2026-04-15

### Fixed
- **format tracing::debug line in bridge.rs** — fix log formatting issue
- **maestro: align SKILL function signatures with IC25.1 official documentation** — fixes Maestro operations compatibility

### Added
- **New skills** — `circuit-optimizer`, `sim-plot`, `schematic-gen`, `spectre-netlist-gotchas` — see [.claude/skills/](.claude/skills/)
- **Maestro skill** and Virtuoso reference documentation

### Dependencies
- Updated various dependencies for stability

## [0.1.2] - 2026-04-13

### Added
- **Interactive TUI Dashboard** — `vtui` binary with Sessions/Jobs/Config tabs
- **Remote Session Auto-Discovery** — `vcli tunnel start` syncs remote sessions
- **Remote Async Spectre Simulation** — `vcli sim run-async` works via SSH nohup
- **SSH Configuration** — `VB_SSH_PORT`, `VB_SSH_KEY` support
- **IC23.1+ Maestro Explorer Support** — `vcli maestro` commands

## [0.1.1] - Previous release

See [release history](https://github.com/deanyou/virtuoso-cli/releases)
