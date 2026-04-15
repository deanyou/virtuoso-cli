# Changelog

All notable changes to this project will be documented in this file.

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
