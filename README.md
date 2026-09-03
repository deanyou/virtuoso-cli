# vcli — Virtuoso CLI

<p align="center">
  <a href="https://crates.io/crates/virtuoso-cli"><img src="https://img.shields.io/crates/v/virtuoso-cli.svg" alt="crates.io"/></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.75+-blue.svg" alt="Rust 1.75+"/></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green.svg" alt="License: MIT"/></a>
  <a href="https://github.com/deanyou/virtuoso-cli/actions"><img src="https://github.com/deanyou/virtuoso-cli/actions/workflows/ci.yml/badge.svg" alt="CI"/></a>
</p>

<p align="center">
  <a href="#english">English</a> | <a href="#中文">中文</a>
</p>

---

## English

Control Cadence Virtuoso from anywhere — locally or remotely. Designed for AI Agents and humans alike.

> **Based on** [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite) by Arcadia-1.
> `vcli` is a full Rust rewrite and major extension of that project, adding multi-session support, dynamic port assignment, session registry, an agent-native CLI, and Spectre simulation integration.

### Overview

`vcli` is a lightweight Rust-based bridge tool for executing SKILL code outside of Virtuoso. It starts a Rust daemon inside Virtuoso via `ramic_bridge.il`, which accepts commands over TCP, calls `evalstring`, and returns results.

### Key Features

- **Multi-session support** — Multiple Virtuoso instances on the same server each get a unique session ID and random port, with no conflicts
- **Multi-session broadcast** — `vcli skill broadcast` fans out to all live sessions concurrently via scoped threads; subagent mode supports heterogeneous multi-step per-session workflows
- **Dynamic port assignment** — Daemon binds port 0 (OS assigns), eliminating port collision
- **Session auto-discovery** — Single session connects automatically; multiple sessions require `--session` or `VB_SESSION`; stale session files for dead daemons are silently filtered out
- **CLI/daemon version unification** — `vcli session show` reports `daemon_version` and a `version_skew` warning when the running vcli binary and the deployed daemon don't match, so you know when to rebuild
- **Stale-daemon recovery** — `ramic_bridge.il` checks `ipcIsAliveProcess` on `load` and silently reaps dead daemons before starting a new one; no manual cleanup required
- **Session history** — Per-session SKILL execution log and CLI command history; survives reconnection (`vcli session history <id>`)
- **Three programming modes** — Raw SKILL expressions, high-level API, or load `.il` files directly
- **Local + remote modes** — Direct local connection or SSH tunnel with ControlMaster multiplexing
- **Native cross-arch tunnel deploy** — `vcli tunnel start` detects remote CPU arch and uploads the matching `resources/daemons/virtuoso-daemon-{x86_64,aarch64}` binary — no need to build on the remote
- **Non-destructive tunnel attach** — `vcli tunnel attach` connects to a daemon already running inside Virtuoso via SSH session discovery, without deploying a new one; `tunnel detach` drops the local tunnel while leaving the remote daemon untouched (4-verb model: start/stop are destructive, attach/detach are not)
- **Per-client scratch scoping** — Concurrent vcli invocations from different `VB_CLIENT_ID` / `VB_PROFILE` env values get isolated `/tmp/virtuoso_bridge/<client>/` scratch dirs, so two operators on the same host never collide
- **Skill Finder** — Fuzzy / prefix / suffix / exact / regex search over Cadence's `~/.cdsinit` and `/opt/cadence/.../finder/SKILL/*.fnd` files (`vcli skill find query --mode fuzzy`)
- **Admin capability gate** — `VCLI_CAPABILITY=admin` unlocks `vcli skill broadcast` and raw SKILL exec for system-wide operations
- **Agent-native CLI** — Noun-verb command structure, JSON structured output, schema introspection, semantic exit codes
- **Schematic editing & reading** — Create, place, wire, connect + read instances, nets, pins, parameters
- **Maestro ADE management** — Open/close Explorer (`maestro`) view sessions, set variables, run simulations, export results (IC23.1+ unified ADE)
- **Spectre simulation** — Sync/async simulation, job registry with status tracking and atomic file writes, PSF parser
- **Multi-profile support** — `--profile` flag for concurrent connections to multiple Virtuoso instances
- **Optional pure-Rust native SSH backend** (`native-ssh` Cargo feature) — a `russh`-based single-hop transport with in-tree host-key verification, public-key auth, SFTP streaming for single files, and a connection pool. OpenSSH stays the default; selecting `native` is explicit via `VB_SSH_BACKEND=native` and there is no automatic backend migration. See `### SSH Backend Selection` below for the capability matrix.
- **Command logging** — All SKILL executions logged to `~/.cache/virtuoso_bridge/logs/commands.log`
- **Interactive TUI** — `vtui` terminal dashboard showing sessions, jobs, tunnel status
- **X11 GUI automation** — `vcli window action-x11` drives Virtuoso forms (click, type, key, drag, screenshot, scroll) with server-side window identity re-validation; `vcli window list-windows-x11` discovers windows by PID/display. Paired with the `virtuoso-gui-debug` skill (`.claude/skills/virtuoso-gui-debug/`) providing a strict JSON DSL, fake/live/local executors, and a multi-method operation playbook (SKILL coordinate reverse-engineering via `hiGetFieldInfo`, xdotool `--window` relative clicks, Tab navigation, CIW direct field assignment). Validated on real Virtuoso IC25.1 dynamic forms with radio field callbacks, file dialogs, and modal-dialog interception handling. [📊 Interactive architecture guide](https://htmlpreview.github.io/?https://github.com/deanyou/virtuoso-cli/blob/main/docs/virtuoso-gui-debug-architecture.html) · [📈 Capability retrospective](https://htmlpreview.github.io/?https://github.com/deanyou/virtuoso-cli/blob/main/docs/vcli-gui-debug-retrospective.html)

### Installation

**From crates.io (recommended):**

```bash
cargo install virtuoso-cli                          # vcli (main CLI)
cargo install virtuoso-cli --bin vtui               # vtui (interactive TUI dashboard)
cargo install virtuoso-cli --features daemon        # virtuoso-daemon (bridge backend)
```

**From source:**

```bash
git clone https://github.com/deanyou/virtuoso-cli.git
cd virtuoso-cli
cargo install --path .
```

All binaries (`vcli`, `vtui`) are installed to `~/.cargo/bin/`.

> **Note**: Do not name the binary `virtuoso` — it conflicts with Cadence's `virtuoso` executable.

### System Dependencies

The X11 GUI automation features (`vcli window action-x11`, `action-x11-batch`, `list-windows-x11`) and the `virtuoso-gui-debug` skill's local executor require the following tools on the Virtuoso host (the machine running the X11 display):

| Tool | Minimum version | Required for | Install |
|------|----------------|--------------|---------|
| **xdotool** | **3.20140419.1** | All GUI input: `--window` flag on `key`/`type`/`mousemove`/`click`/`mousedown`/`mouseup`, `--repeat`/`--delay` on `click`, `search --onlyvisible`, `mousemove_relative` | `apt install xdotool` / `yum install xdotool` |
| **xwininfo** | any (x11-utils) | Geometry precheck (`--direct` mode), window bounds validation | `apt install x11-utils` / `yum install xorg-x11-utils` |
| **ImageMagick** (`import`) | 6.x+ | Screenshots (`action-x11 --operation screenshot`, skill local executor) | `apt install imagemagick` / `yum install ImageMagick` |

**xdotool version note**: The critical feature is the `--window` flag on input commands, introduced in **3.20140419.1** (2014). Versions older than this reject `--window` on `key`/`type`/`click` and GUI automation will fail. Recommended: **3.20160805.1** or later. Tested on **3.20200624.1** (Ubuntu 20.04+). Verify with `xdotool --version`.

> **`maximize` operation version requirement**: True window maximization uses `xdotool windowstate --add MAXIMIZED_HORZ --add MAXIMIZED_VERT`. The `windowstate` subcommand was added in **xdotool 3.20210804.1** (2021), which is newer than the global minimum above. We deliberately do **not** raise the global minimum for the whole CLI to cover this single operation — `minimize`, `click-abs`, `double-click`, and every other operation still work on 3.20140419.1+. On xdotool older than 3.20210804.1, `maximize` returns xdotool's own "unknown command" error. There is no `windowmaximize` xdotool command; `windowmove`/`windowsize` do not set the maximized WM state.

`xwininfo` and ImageMagick are optional — `vcli window action-x11 --direct` works with xdotool alone (geometry precheck falls back gracefully). Screenshots require ImageMagick `import`.

### Quick Start

**1. Load RAMIC Bridge in Virtuoso CIW:**

```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
```

`load` automatically stops any existing daemon, resets the path to `~/.cargo/bin/virtuoso-daemon`, starts fresh, and prints the Ready banner — works for first load and for reloading after updates.

Output:
```
┌─────────────────────────────────────────┐
│  vcli (Virtuoso CLI Bridge) — Ready     │
├─────────────────────────────────────────┤
│  Session : eda-meow-1                   │
│  Port    : 42109                        │
│  SSH     : 22                           │
│  Version : 1.0.0                        │
│  Daemon  : ~/.cargo/bin/virtuoso-daemon │
├─────────────────────────────────────────┤
│  Terminal: vcli skill exec 'version()'  │
│  Sessions: vcli session list            │
└─────────────────────────────────────────┘
```

`SSH` is the port used for SSH tunnel connections (host network mode → 22; bridge mode with port mapping users can override via `RB_SSH_PORT` in the Virtuoso shell env before `load`). `Port` is the daemon's own TCP listener (OS-assigned, never collides).

Add to `~/.cdsinit` for automatic loading on Virtuoso startup:
```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
```

**2. Connect from terminal:**

```bash
vcli session list                                        # list active sessions
vcli skill exec 'getCurrentTime()'                       # auto-connects if single session
vcli --session eda-meow-2 skill exec 'getCurrentTime()' # specify session explicitly
```

**Remote mode (deploy new daemon):**
```bash
vcli init           # generate .env template
# edit .env: set VB_REMOTE_HOST, VB_SPECTRE_CMD (absolute path)
vcli tunnel start
vcli skill exec 'getCurrentTime()'
vcli tunnel stop
```

**Remote mode (connect to existing Virtuoso daemon — non-destructive):**
```bash
# Virtuoso + daemon already running on the remote host
vcli tunnel attach                       # discover + connect; does NOT deploy a new daemon
vcli skill exec 'getCurrentTime()'
vcli tunnel detach                       # drop the SSH tunnel; daemon keeps listening
```

The 4-verb tunnel model:

| Verb | Lifecycle | Remote effect |
|------|-----------|---------------|
| `tunnel start`  | Deploy    | uploads daemon binary, starts daemon on remote, builds tunnel |
| `tunnel stop`   | Destroy   | kills daemon + tunnel, removes `/tmp/virtuoso_bridge_*/` |
| `tunnel attach` | Connect   | scans `~/.cache/virtuoso_bridge/sessions/*.json`, builds tunnel to live daemon |
| `tunnel detach` | Disconnect | kills tunnel only; daemon stays alive |

`start`/`stop` are destructive (you own what you deployed); `attach`/`detach` are non-destructive (Virtuoso owns the daemon). Use `attach` when Virtuoso is already running and you just want CLI access.

**Reloading SKILL without typing in CIW:**
```bash
export VCLI_CAPABILITY=admin
vcli skill load ./my_script.il
vcli skill exec "isCallable('myFunction)"
vcli skill exec 'myFunction()'
```

On the Virtuoso host (including `ssh host 'vcli ...'`), `skill load` loads the
original absolute path. With a vcli-managed SSH tunnel, it uploads into a private,
unique `${TMPDIR:-/tmp}/vcli-skill-*` directory on the remote host. It preserves
the filename and `.ils` language suffix; dependencies must also be accessible on
that host. The JSON `loaded_path` identifies the source retained for debugging;
uploaded sources are not removed by `tunnel detach`.

Raw `skill exec`, `skill eval`, and `skill load` require Admin. Explicit
`skill exec --readonly` retains the existing pattern restrictions even for Admin;
it is not a complete SKILL interpreter sandbox. A failed file load returns a
nonzero exit/error for both CLI and RPC, including a SKILL `nil` result. In
contrast, a raw expression returning `nil` is valid query data.

If you manage SSH forwarding yourself and use `VB_REMOTE_HOST=localhost` on your
laptop, vcli has no SSH file-transfer channel. Run vcli on the Virtuoso host, or
use `tunnel attach` with `VB_REMOTE_HOST` set to the compute host before loading
local files. Use `--session <id>` to select an existing session explicitly.

**Remote async simulation:**
```bash
vcli sim run-async --netlist my_tb.scs   # launch on remote server, return immediately
vcli sim job-list                        # check all jobs (auto-refreshes status via SSH)
vcli sim job-status <id>                 # detailed status for one job
vcli sim job-cancel <id>                 # kill remote spectre process
```

**Maestro ADE Explorer (IC23.1+):**
```bash
# IC23.1 unified ADE uses "maestro" view (formerly adexl/ade_xl)
vcli maestro open --lib myLib --cell myCell            # defaults to view=maestro
vcli maestro set-var --session fnxSession4 --name W --value 10u
vcli maestro run --session fnxSession4                 # async run
vcli maestro export --session fnxSession4 --path out.csv
```

**Multi-Session Operations:**
```bash
vcli skill broadcast 'getVersion(t)'            # Same SKILL on all sessions
VB_SESSION=eda-meow-1 vcli maestro run ...      # Different tasks via subagents
```

### Multi-Session Architecture

```
Virtuoso-1 → vcli() → daemon on port 42109 → session: eda-meow-1
Virtuoso-2 → vcli() → daemon on port 51337 → session: eda-meow-2

Terminal A: vcli skill exec '...'                  # auto-selects (single session)
Terminal B: vcli --session eda-meow-2 skill exec   # explicit selection
```

Session files: `~/.cache/virtuoso_bridge/sessions/<id>.json`

### Command Reference

```
vcli [--profile P] [--session S] [--format json|table]
├── init                              Generate .env config template
├── session                           Manage bridge sessions
│   ├── list                              List all active sessions
│   ├── show [id]                         Show session details (with daemon_version + version_skew check)
│   ├── current                           Show which session would be auto-selected
│   ├── cleanup                           Remove stale session files for dead daemons
│   └── history <id> [--skill] [--cmd] [--limit N]   SKILL + CLI history for a session
├── tunnel                            Manage SSH tunnel
│   ├── start [--timeout N] [--dry-run]    Deploy new daemon + tunnel (destructive)
│   ├── stop [--force] [--dry-run]         Destroy what start created
│   ├── restart [--timeout N]
│   ├── attach [--dry-run]                 Connect to an existing Virtuoso daemon (non-destructive)
│   ├── detach                             Drop attach tunnel; daemon keeps listening
│   ├── status
│   └── diagnose                          Full connection diagnostics
├── skill                             Execute SKILL code
│   ├── exec <code> [--timeout N]
│   ├── load <file>
│   ├── broadcast <code>              Fan out to all live sessions in parallel (requires VCLI_CAPABILITY=admin)
│   ├── find <query> [--mode M] [--include-desc]   Search Cadence .fnd (fuzzy/prefix/suffix/exact/regex)
│   └── info <name>                     Detailed info for one SKILL function
├── cell                              Manage cellviews
│   ├── open --lib L --cell C [--view V] [--mode M] [--dry-run]
│   ├── save / close / info
├── schematic                         Schematic editing & reading
│   ├── open / save / check / build --spec file.json
│   ├── place / wire / conn / label / pin
│   ├── list-instances / list-nets / list-pins
│   └── get-params --inst M1
├── maestro                           Maestro ADE Explorer (maestro view) sessions
│   ├── open --lib L --cell C
│   ├── close / list-sessions / save
│   ├── set-var / get-analyses / add-output
│   ├── run / export
├── sim                               Simulation
│   ├── setup / run / measure / sweep / corner
│   ├── run-async --netlist file.scs
│   ├── job-status / job-list / job-cancel
│   └── results / netlist
├── design                            gm/Id sizing tools
│   ├── size / explore
├── process                           Process characterization
│   └── char [--netlist]
└── schema [--all] [noun] [verb]      Output command schema (for Agent discovery)
```

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `VB_SESSION` | — | Target session ID (for multi-instance) |
| `VB_PORT` | per-user hash | Direct port (fallback when no session file) |
| `VB_REMOTE_HOST` | — | SSH remote hostname or alias |
| `VB_REMOTE_USER` | current user | SSH login username |
| `VB_JUMP_HOST` | — | Bastion/jump host address |
| `VB_TIMEOUT` | `30` | Connection/execution timeout (seconds) |
| `VB_SSH_PORT` | `22` | SSH port on the remote host (used by `vcli tunnel`); distinct from `VB_PORT` which is the daemon's TCP listener |
| `VB_SSH_KEY` | — | Identity file, passed to ssh as `-i <path>` |
| `VB_SSH_CONFIG` | — | Custom SSH config file, passed to ssh as `-F <path>` |
| `VB_SSH_BACKEND` | `openssh` | SSH backend: `openssh` (default) or `native` |
| `VB_PROFILE` | — | Config profile (reads `VB_*_<profile>` vars) |
| `VB_CLIENT_ID` | `$VB_PROFILE` or `gethostname()` | Per-client remote scratch scoping (e.g. `vcli-A`, `vcli-B`); isolates `/tmp/virtuoso_bridge/<client>/` paths between concurrent operators |
| `VCLI_CAPABILITY` | `user` | Set to `admin` to unlock `vcli skill broadcast` and raw SKILL exec |
| `RB_DAEMON_PATH` | auto-detected | Override daemon binary path |

### SSH Remote Connection Setup

`vcli tunnel start` needs a working SSH path to the compute host first.

**1. Generate an SSH key (optional):**
```bash
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_vcli
ssh-copy-id -i ~/.ssh/id_ed25519_vcli user@remote-host
```

**2. Configure `~/.ssh/config`:**

```ssh-config
Host my-server
    HostName 192.168.1.100
    User username
    IdentityFile ~/.ssh/id_ed25519_vcli
    StrictHostKeyChecking accept-new
    ConnectTimeout 10
    # Important: bypass a local HTTP proxy (Clash / Surge) for SSH
    ProxyCommand none
```

> **Host key checking**: `accept-new` trusts a host on first contact but still
> refuses to connect if its key later changes, so MITM protection is preserved.
> Only downgrade to `StrictHostKeyChecking no` on a trusted internal network —
> never in production.

> **Proxy**: if a system proxy (macOS Network Preferences) intercepts SSH,
> `ProxyCommand none` is required to bypass it.

**3. Test the SSH connection:**
```bash
ssh my-server "uname -m"   # expect x86_64 or aarch64
```

**4. Connect with vcli:**
```bash
# Option 1 — use the Host alias from ~/.ssh/config
VB_REMOTE_HOST=my-server vcli tunnel start

# Option 2 — address the host directly (set the user separately, NOT as user@host)
VB_REMOTE_HOST=192.168.1.100 VB_REMOTE_USER=username vcli tunnel start

# Option 3 — point at a specific key or SSH config file
VB_SSH_KEY=~/.ssh/id_ed25519_vcli VB_REMOTE_HOST=my-server vcli tunnel start
VB_SSH_CONFIG=~/.ssh/config_vcli VB_REMOTE_HOST=my-server vcli tunnel start
```

> `VB_REMOTE_HOST` and `VB_REMOTE_USER` are combined into `user@host`, so do
> **not** put `user@` in `VB_REMOTE_HOST` while also setting `VB_REMOTE_USER` —
> that yields `user@user@host`. Use one form or the other.

> `VB_REMOTE_HOST` must name the host **running Virtuoso** (the compute host),
> not the bastion — the bastion goes in `VB_JUMP_HOST`. `vcli tunnel status`
> reports a mismatch between the configured and actual hostname.

**5. Fallback — run vcli directly on the remote host:**
```bash
# If the tunnel hits a permission problem, invoke the remote vcli over SSH
ssh my-server 'vcli skill exec "version()"'
ssh my-server 'vcli session list'

# Handy alias
alias rvcli='ssh my-server vcli'
rvcli session list
```

**Troubleshooting:**
```bash
# Inspect proxy settings (macOS)
networksetup -getwebproxy "Wi-Fi"

# Test a direct connection, bypassing the proxy
ssh -o "ProxyCommand=none" my-server "echo ok"

# Verbose vcli logging
vcli tunnel start -v
```

### SSH Backend Selection

`vcli` ships two SSH transports behind the same `RemoteTransport` contract:

| Backend | Selected via | What it actually runs |
|---------|--------------|-----------------------|
| `openssh` (default) | unset / `VB_SSH_BACKEND=openssh` | System `ssh` client — ControlMaster multiplexing, full `~/.ssh/config` (Host / ProxyJump / IdentityAgent / Match) |
| `native` | `VB_SSH_BACKEND=native` | Pure-Rust `russh` client (gated behind the `native-ssh` Cargo feature) |

**There is no automatic backend migration.** Switching backends requires an explicit `VB_SSH_BACKEND` change; existing tunnels keep using whatever backend created their `state.json` until `tunnel stop && tunnel start`.

**Feature-gated builds.** Official `cargo install virtuoso-cli` binaries are built with `--features native-ssh`. Custom builders who strip the feature can still select `openssh`; selecting `native` on such a build returns a structured `UnsupportedBackend` error rather than silently falling back to OpenSSH.

`vcli tunnel status` JSON reports the active selection — look for:

```json
{
  "config": {
    "backend": {
      "selected": "openssh",
      "supported_in_build": true
    }
  },
  "tunnel": { "backend": "openssh" }
}
```

If `config.backend.selected` and `tunnel.backend` disagree, the JSON also carries a `config.backend_warning` field describing the drift — your `state.json` was written by a different backend than the one you're running now.

#### Capability matrix (current)

| Capability | `openssh` | `native` |
|------------|-----------|----------|
| Host-key verification (`known_hosts`) | yes (delegated to `ssh`) | yes (in-tree, supports legacy + `|1|salt|hash` entries) |
| Public-key auth | yes | yes |
| SSH agent forwarding | yes | no (selecting this returns `UnsupportedOperation`) |
| Password / keyboard-interactive | yes (via `ssh`) | no (requires `VB_SSH_KEY`) |
| `ProxyJump` / chained hops | yes (delegated to `ssh`) | no (single-hop only; returns `UnsupportedOperation`) |
| Custom `~/.ssh/config` (`-F`) | yes (`VB_SSH_CONFIG`) | no — the native backend does not read `VB_SSH_CONFIG`; setting it has no effect on the native path |
| Single-file SFTP streaming | yes (delegated to `sftp`/`scp`) | yes (russh-sftp subsystem, 64 KiB chunk window) |
| Directory transfer | tar-over-exec | tar-over-exec (matches `openssh`) |
| Concurrent connection reuse | ControlMaster | in-tree endpoint pool + channel scheduler |
| Auto-reconnect with backoff | handled by `ssh` | yes (`VB_SSH_RECONNECT_MAX_ATTEMPTS` / `_MAX_DELAY`) |
| Keepalive probing | delegated to `ssh` (`ServerAliveInterval`) | yes (`VB_SSH_KEEPALIVE_INTERVAL` / `_FAILURES`) |
| SOCKS5 forwarding | no (planned) | no (planned) |
| RAMIC / X11 forwarding | yes (delegated to `ssh`) | no |

If you depend on anything in the right column of the `no` rows above, stay on `openssh` for now. If your work is single-hop, you want a self-contained Rust binary (no `/usr/bin/ssh` on the host), or you need the in-tree connection pool's latency profile, opt into `native`.

### GUI Debugging

`vcli` can drive Virtuoso's X11 GUI forms directly — no screen-scraping or OCR required. This enables AI agents to test SKILL GUI code (forms, dialogs, callbacks) end-to-end on a real Virtuoso session.

#### Window commands

```bash
# Discover windows on a display (JSON: window_id, pid, title, geometry)
vcli window list-windows-x11 --display :5.0 --format json

# Interact with a specific window (server-side re-validates window identity on every call)
vcli window action-x11 \
  --window-id 0x26037c7 --pid 114668 --display :5.0 \
  --operation click-rel --x 300 --y 167

# Supported operations: click-rel, click-abs, type, key, drag-rel, activate, screenshot, scroll, minimize, maximize, double-click, close
vcli window action-x11 --window-id 0x26037c7 --pid 114668 --display :5.0 \
  --operation type --text "METAL1"
```

> **Important**: `vcli skill exec` runs in a daemon context that has **no UI library loaded** (`hiCreateAppForm`, `hiDisplayForm`, etc. are nil). Therefore GUI forms must be created/shown via the **CIW** (type SKILL into the CIW input line with xdotool), and GUI interaction goes through `vcli window action-x11` or xdotool. Reading/setting form field values works via the CIW (`form->field->value = "..."`).

#### virtuoso-gui-debug skill

The repo ships `.claude/skills/virtuoso-gui-debug/` — a deterministic GUI debugging skill with:

- **Strict JSON DSL** for replayable scenarios (validate before run)
- **Three executors**: `fake` (offline regression), `live` (real vcli), `local` (direct xdotool)
- **Fail-closed prechecks**: session/port match, PID positivity, DISPLAY equality, unique window binding, exclusive X11 lock
- **Multi-method operation playbook** — every operation has ≥2 independently verified stable methods

#### Multi-method operation matrix (validated on IC25.1)

| Operation | Method 1 (recommended) | Method 2 | Avoid |
|-----------|----------------------|----------|-------|
| Window discovery | `vcli window list-windows-x11 --format json` | `xdotool search --name "title"` | — |
| Coordinate acquisition | SKILL reverse-engineering: `hiGetFieldInfo(form (quote field))` → `((x y) (w h))`, center = `(x+w/2, y+h/2)` | ImageMagick `import -window` + `convert -crop -resize` pixel-level | OCR percent boxes (±40px drift) |
| Click | `xdotool mousemove --window <wid> <cx> <cy>; xdotool click 1` (window-relative, no xwininfo needed) | `vcli window action-x11 --operation click-rel` | — |
| Text input | `xdotool type --clearmodifiers --delay 50 "text"` (after focusing field) | Tab-key navigation to field + xdotool type; or CIW direct `form->field->value = "text"` | `vcli window action-x11 --operation type` (injects garbled content on IC25.1) |
| Submit / confirm | `xdotool key Return` (dialogs: equivalent to Open/OK) | Click button (click-rel or xdotool) | — |
| Close / cancel | `xdotool key Escape` | CIW `hiFormCancel(form)` | — |

#### Modal dialog handling

Modal dialogs (e.g. "Choose a File" from `hiDisplayFileDialog`) **intercept all input** — clicks on the parent form silently fail. After any Browse/Open action, run `vcli window list-windows-x11` to detect unexpected dialogs, then dismiss with `Return` (submit) or `Escape` (cancel) **before** resuming parent-form operations.

#### Quick GUI debug loop

```
1. validate SKILL → 2. scp to remote → 3. CIW: load("file.il")
4. CIW: showForm()  → 5. vcli list-windows-x11 (get wid)
6. CIW: hiGetFieldInfo(form (quote field))  → exact coords
7. xdotool mousemove --window + click       → interact
8. xdotool type / CIW assign                → input
9. ImageMagick crop screenshot              → visual verify
10. CIW screenshot → read callback output   → behavioral verify
11. Modal dialog? → handle first
12. Bug found → fix SKILL → repeat from 2
```

### How It Works

```
Terminal                      Virtuoso Process
────────                      ────────────────

vcli skill exec "1+2"
      │
      │ TCP: {"skill":"1+2"}
      ├──────────────────► virtuoso-daemon (port 42109)
      │                          │
      │                          │ evalstring("1+2")
      │                          │
      │ TCP: "3"
      ◄──────────────────────────┘
```

Session registration flow:
```
vcli() in CIW
  → RBStart(): ipcBeginProcess(daemon, port=0)
  → OS assigns port N; daemon prints "PORT:N" to stderr
  → RBIpcErrHandler: RBPort=N, writes session file
  → ~/.cache/virtuoso_bridge/sessions/<id>.json

vcli session list  # reads session files
vcli skill exec    # connects to port N
```

---

## 中文

从任何地方控制 Cadence Virtuoso，本地或远程均可。为 AI Agent 和人类共同设计。

> **基于** [virtuoso-bridge-lite](https://github.com/Arcadia-1/virtuoso-bridge-lite)（作者 Arcadia-1）重构。
> `vcli` 是对该项目的完整 Rust 重写与大幅扩展，新增了多 session 支持、动态端口分配、session 注册表、Agent 原生 CLI 以及 Spectre 仿真集成。

### 简介

`vcli` 是一个用 Rust 编写的轻量级桥接工具，用于在 Virtuoso 外部执行 SKILL 代码。它通过 `ramic_bridge.il` 在 Virtuoso 内启动一个 Rust daemon，并通过 TCP 接收来自 CLI 的命令，调用 `evalstring` 执行 SKILL 并返回结果。

### 核心特性

- **多 session 支持** — 同一台服务器上可同时运行多个 Virtuoso 实例，每个实例自动分配唯一 session_id 和随机端口，互不干扰
- **多 session 并发广播** — `vcli skill broadcast` 通过 scoped threads 并发广播到所有活跃 session；subagent 模式支持不同 session 执行不同多步工作流
- **动态端口分配** — daemon 绑定端口 0（OS 自动分配），彻底避免端口冲突
- **session 自动发现** — 只有一个 session 时无需指定；多个 session 时通过 `--session` 或 `VB_SESSION` 选择；已死亡的 daemon 对应的 session 文件自动过滤
- **CLI/daemon 版本统一** — `vcli session show` 显示 `daemon_version` 字段，若 vcli 二进制版本与运行中的 daemon 不一致会报 `version_skew` 警告，提示需重新部署
- **陈旧 daemon 自动恢复** — `ramic_bridge.il` 在 `load` 时检查 `ipcIsAliveProcess`，静默清理已死的 daemon 后再启动新实例；无需手动清理
- **Session 历史记录** — 每个 session 独立保存 SKILL 执行日志和 CLI 命令历史，断线重连后可恢复（`vcli session history <id>`）
- **三种编程方式** — 原始 SKILL 表达式、高阶 API、或直接加载 .il 文件
- **本地+远程模式** — 支持本地直连或 SSH 隧道（ControlMaster 连接复用）
- **隧道跨架构原生部署** — `vcli tunnel start` 自动检测远端 CPU 架构，并上传对应的 `resources/daemons/virtuoso-daemon-{x86_64,aarch64}` 二进制，无需在远端 build
- **非破坏性隧道连接** — `vcli tunnel attach` 通过 SSH session 发现机制连接 Virtuoso 内已运行的 daemon，不部署新 daemon；`tunnel detach` 仅断开本地隧道，远端 daemon 保持运行（4 动词模型：start/stop 破坏性，attach/detach 非破坏性）
- **每客户端 scratch 隔离** — 不同 `VB_CLIENT_ID` / `VB_PROFILE` 环境变量下的并发 vcli 调用拥有独立的 `/tmp/virtuoso_bridge/<client>/` 目录，多个操作员在同一主机操作时互不冲突
- **Skill Finder** — 模糊 / 前缀 / 后缀 / 精确 / 正则 五种模式搜索 Cadence `~/.cdsinit` 与 `/opt/cadence/.../finder/SKILL/*.fnd` 文件（`vcli skill find query --mode fuzzy`）
- **Admin 权限门** — `VCLI_CAPABILITY=admin` 解锁 `vcli skill broadcast` 与原始 SKILL 执行权限
- **Agent 原生 CLI** — noun-verb 命令结构、JSON 结构化输出、schema 自省、语义化退出码
- **原理图编辑与读取** — 创建、放置、连线 + 读取实例/网络/引脚/参数
- **Maestro ADE 管理** — 打开/关闭 Explorer（`maestro` view）session、设置变量、运行仿真、导出结果（IC23.1+ 统一 ADE）
- **Spectre 仿真** — 同步/异步仿真、Job 注册与状态跟踪、PSF 结果解析
- **多 Profile 支持** — `--profile` 参数支持同时连接多个 Virtuoso 实例
- **可选的纯 Rust 原生 SSH 后端**（`native-ssh` Cargo feature）— 基于 `russh` 的单跳传输，自带主机密钥校验、公钥认证、单文件 SFTP streaming 与连接池。OpenSSH 仍是默认；选择 `native` 必须显式通过 `VB_SSH_BACKEND=native`，**没有自动迁移**。能力差异表见下文 `### SSH 后端选择`。
- **命令日志** — 所有 SKILL 调用记录到 `~/.cache/virtuoso_bridge/logs/commands.log`
- **交互式 TUI** — `vtui` 终端仪表盘，实时显示 session、仿真 job、隧道状态
- **X11 GUI 自动化** — `vcli window action-x11` 驱动 Virtuoso 表单（点击、输入、按键、拖拽、截图、滚轮），服务端每次操作重新校验窗口身份；`vcli window list-windows-x11` 按 PID/display 发现窗口。配套 `virtuoso-gui-debug` 技能（`.claude/skills/virtuoso-gui-debug/`）提供严格 JSON DSL、fake/live/local 三种执行器，以及多方法操作手册（通过 `hiGetFieldInfo` 反推字段坐标、xdotool `--window` 相对点击、Tab 键导航、CIW 直接赋值字段）。已在真实 Virtuoso IC25.1 动态表单上验证，覆盖 radio 回调、文件对话框、模态对话框拦截处理等场景。

### 安装

**从 crates.io 安装（推荐）：**

```bash
cargo install virtuoso-cli                          # vcli（主 CLI）
cargo install virtuoso-cli --bin vtui               # vtui（交互式 TUI 仪表盘）
cargo install virtuoso-cli --features daemon        # virtuoso-daemon（bridge 后端）
```

**从源码安装：**

```bash
git clone https://github.com/deanyou/virtuoso-cli.git
cd virtuoso-cli
cargo install --path .
```

安装后 `vcli` 和 `virtuoso-daemon` 均位于 `~/.cargo/bin/`。

> **注意**：不要将 CLI 命名为 `virtuoso`，与 Cadence Virtuoso 二进制名冲突。

### 系统依赖

X11 GUI 自动化功能（`vcli window action-x11`、`action-x11-batch`、`list-windows-x11`）以及 `virtuoso-gui-debug` 技能的 local 执行器，需要在 Virtuoso 所在主机（运行 X11 display 的机器）上安装以下工具：

| 工具 | 最低版本 | 用途 | 安装命令 |
|------|---------|------|---------|
| **xdotool** | **3.20140419.1** | 所有 GUI 输入：`key`/`type`/`mousemove`/`click`/`mousedown`/`mouseup` 的 `--window` 标志、`click` 的 `--repeat`/`--delay`、`search --onlyvisible`、`mousemove_relative` | `apt install xdotool` / `yum install xdotool` |
| **xwininfo** | 任意（x11-utils） | 几何预检（`--direct` 模式）、窗口边界校验 | `apt install x11-utils` / `yum install xorg-x11-utils` |
| **ImageMagick**（`import`） | 6.x+ | 截图（`action-x11 --operation screenshot`、技能 local 执行器） | `apt install imagemagick` / `yum install ImageMagick` |

**xdotool 版本说明**：关键特性是输入命令的 `--window` 标志，于 **3.20140419.1**（2014 年）引入。更早版本会拒绝 `key`/`type`/`click` 的 `--window` 参数，导致 GUI 自动化失败。推荐 **3.20160805.1** 或更高版本。已在 **3.20200624.1**（Ubuntu 20.04+）上验证。用 `xdotool --version` 检查。

> **`maximize` 操作的版本要求**：真正的窗口最大化使用 `xdotool windowstate --add MAXIMIZED_HORZ --add MAXIMIZED_VERT`。`windowstate` 子命令于 **xdotool 3.20210804.1**（2021 年）新增，比上面的全局最低版本更新。我们**刻意不**为了这一个操作抬高整个 CLI 的全局最低版本——`minimize`、`click-abs`、`double-click` 以及其它所有操作在 3.20140419.1+ 上仍然可用。xdotool 低于 3.20210804.1 时，`maximize` 会返回 xdotool 自身的「unknown command」错误。xdotool 没有 `windowmaximize` 命令；`windowmove`/`windowsize` 也不会设置最大化 WM 状态。

`xwininfo` 和 ImageMagick 为可选——`vcli window action-x11 --direct` 仅需 xdotool 即可工作（几何预检会优雅降级）。截图功能需要 ImageMagick `import`。

### 快速开始

**第一步：在 Virtuoso CIW 中加载 RAMIC Bridge：**

```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
```

`load` 会自动停止旧 daemon、将路径重置为 `~/.cargo/bin/virtuoso-daemon` 并重启，首次加载和更新后重载均适用。

输出：
```
┌─────────────────────────────────────────┐
│  vcli (Virtuoso CLI Bridge) — Ready     │
├─────────────────────────────────────────┤
│  Session : eda-meow-1                   │
│  Port    : 42109                        │
│  SSH     : 22                           │
│  Version : 1.0.0                        │
│  Daemon  : ~/.cargo/bin/virtuoso-daemon │
├─────────────────────────────────────────┤
│  Terminal: vcli skill exec 'version()'  │
│  Sessions: vcli session list            │
└─────────────────────────────────────────┘
```

`SSH` 是 SSH 隧道连接用的端口（host 网络模式下为 22；bridge 模式 + 端口映射的用户可在 `load` 前通过 Virtuoso shell 环境变量 `RB_SSH_PORT` 覆盖）。`Port` 是 daemon 自身的 TCP 监听端口（由 OS 分配，永不冲突）。

在 `~/.cdsinit` 中加入以下内容，实现 Virtuoso 启动时自动加载：
```skill
load("/path/to/virtuoso-cli/resources/ramic_bridge.il")
```

**第二步：从终端连接：**

```bash
vcli session list                                        # 查看所有活跃 session
vcli skill exec 'getCurrentTime()'                       # 单 session 时自动连接
vcli --session eda-meow-2 skill exec 'getCurrentTime()' # 多 session 时指定目标
```

**远程模式（部署新 daemon）：**
```bash
vcli init           # 生成 .env 配置模板
# 编辑 .env：设置 VB_REMOTE_HOST、VB_SPECTRE_CMD（绝对路径）
vcli tunnel start
vcli skill exec 'getCurrentTime()'
vcli tunnel stop
```

**远程模式（连接已存在的 Virtuoso daemon — 非破坏性）：**
```bash
# Virtuoso + daemon 已在远端主机运行
vcli tunnel attach                       # 自动发现 + 连接；不会部署新 daemon
vcli skill exec 'getCurrentTime()'
vcli tunnel detach                       # 断开 SSH 隧道；daemon 继续监听
```

隧道 4-动词模型：

| 动词 | 生命周期 | 远端副作用 |
|------|---------|-----------|
| `tunnel start`  | 部署   | 上传 daemon 二进制，远端启动 daemon，建隧道 |
| `tunnel stop`   | 销毁   | 杀 daemon + 隧道，删除 `/tmp/virtuoso_bridge_*/` |
| `tunnel attach` | 连接   | 扫描 `~/.cache/virtuoso_bridge/sessions/*.json`，建隧道到活跃 daemon |
| `tunnel detach` | 断开   | 仅杀隧道；daemon 保持运行 |

`start`/`stop` 是破坏性的（你部署的你能清）；`attach`/`detach` 是非破坏性的（daemon 归 Virtuoso 所有）。当 Virtuoso 已在跑、你只想用 CLI 连进去时，用 `attach`。

**远程异步仿真：**
```bash
vcli sim run-async --netlist my_tb.scs   # 在远程服务器启动仿真，立即返回
vcli sim job-list                        # 查看所有 job（通过 SSH 自动刷新状态）
vcli sim job-status <id>                 # 查看单个 job 详情
vcli sim job-cancel <id>                 # 终止远程 spectre 进程
```

**Maestro ADE Explorer（IC23.1+）：**
```bash
# IC23.1 统一 ADE 使用 "maestro" view（旧版本为 adexl/ade_xl）
vcli maestro open --lib myLib --cell myCell            # 默认 view=maestro
vcli maestro set-var --session fnxSession4 --name W --value 10u
vcli maestro run --session fnxSession4                 # 异步运行
vcli maestro export --session fnxSession4 --path out.csv
```

**多 session 并发操作：**
```bash
vcli skill broadcast 'getVersion(t)'            # 同一 SKILL 广播到所有 session
VB_SESSION=eda-meow-1 vcli maestro run ...      # 通过 subagent 实现不同任务
```

### 多 Session 工作原理

```
Virtuoso-1 → vcli() → daemon on port 42109 → session: eda-meow-1
Virtuoso-2 → vcli() → daemon on port 51337 → session: eda-meow-2

终端 A: vcli skill exec '...'                  # 自动连接（单 session）
终端 B: vcli --session eda-meow-2 skill exec   # 显式指定
```

Session 注册文件保存在 `~/.cache/virtuoso_bridge/sessions/<id>.json`。

### 命令参考

```
vcli [--profile P] [--session S] [--format json|table]
├── init                              创建 .env 配置模板
├── session                           管理 bridge session
│   ├── list                              列出所有活跃 session
│   ├── show [id]                         查看 session 详情（含 daemon_version + version_skew 检查）
│   ├── current                           显示会被自动选中的 session
│   ├── cleanup                           删除已死亡 daemon 的 session 文件
│   └── history <id> [--skill] [--cmd] [--limit N]   查看 SKILL + CLI 历史
├── tunnel                            管理 SSH 隧道
│   ├── start [--timeout N] [--dry-run]    部署新 daemon + 隧道（破坏性）
│   ├── stop [--force] [--dry-run]         销毁 start 创建的内容
│   ├── restart [--timeout N]
│   ├── attach [--dry-run]                 连接已存在的 Virtuoso daemon（非破坏性）
│   ├── detach                             断开 attach 隧道；daemon 继续监听
│   ├── status
│   └── diagnose                          完整连接诊断
├── skill                             执行 SKILL 代码
│   ├── exec <code> [--timeout N]
│   ├── load <file>
│   ├── broadcast <code>              并发广播到所有活跃 session（需 VCLI_CAPABILITY=admin）
│   ├── find <query> [--mode M] [--include-desc]   搜索 Cadence .fnd（fuzzy/prefix/suffix/exact/regex）
│   └── info <name>                     查看单个 SKILL 函数详情
├── cell                              管理 cellview
│   ├── open / save / close / info
├── schematic                         原理图编辑与读取
│   ├── open / save / check / build --spec file.json
│   ├── place / wire / conn / label / pin
│   ├── list-instances / list-nets / list-pins
│   └── get-params --inst M1
├── maestro                           Maestro ADE Explorer（maestro view）仿真
│   ├── open / close / list-sessions / save
│   ├── set-var / get-analyses / add-output
│   ├── run / export
├── sim                               仿真
│   ├── setup / run / measure / sweep / corner
│   ├── run-async / job-status / job-list / job-cancel
│   └── results / netlist
├── design                            gm/Id 设计工具
├── process                           工艺表征
└── schema [--all] [noun] [verb]      输出命令 schema（供 Agent 发现）
```

### 配置说明

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `VB_SESSION` | - | 目标 session ID（多实例时使用） |
| `VB_PORT` | 按用户名 hash | 直连端口（无 session 文件时的回退值） |
| `VB_REMOTE_HOST` | - | SSH 远程主机名或别名 |
| `VB_REMOTE_USER` | 当前用户 | SSH 登录用户名 |
| `VB_JUMP_HOST` | - | 跳板机/堡垒机地址 |
| `VB_TIMEOUT` | `30` | 连接/执行超时（秒） |
| `VB_SSH_PORT` | `22` | 远端主机 SSH 端口（`vcli tunnel` 使用）；与 `VB_PORT`（daemon TCP 监听端口）不同 |
| `VB_SSH_KEY` | - | 密钥文件，以 `-i <path>` 传给 ssh |
| `VB_SSH_CONFIG` | - | 自定义 SSH config 文件，以 `-F <path>` 传给 ssh |
| `VB_SSH_BACKEND` | `openssh` | SSH 后端：`openssh`（默认）或 `native` |
| `VB_PROFILE` | - | 配置 profile（读取 `VB_*_<profile>` 变量） |
| `VB_CLIENT_ID` | `$VB_PROFILE` 或 `gethostname()` | 每客户端远端 scratch 隔离标识（如 `vcli-A`、`vcli-B`）；隔离 `/tmp/virtuoso_bridge/<client>/` 路径，避免多操作员并发冲突 |
| `VCLI_CAPABILITY` | `user` | 设为 `admin` 解锁 `vcli skill broadcast` 与原始 SKILL 执行权限 |
| `RB_DAEMON_PATH` | 自动检测 | 覆盖 daemon 二进制路径 |

### SSH 远程连接配置

执行 `vcli tunnel start` 之前，需要先打通到计算主机的 SSH 通路。

**1. 生成 SSH 密钥（可选）：**
```bash
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_vcli
ssh-copy-id -i ~/.ssh/id_ed25519_vcli user@remote-host
```

**2. 配置 SSH Config (`~/.ssh/config`)：**

```ssh-config
Host my-server
    HostName 192.168.1.100
    User username
    IdentityFile ~/.ssh/id_ed25519_vcli
    StrictHostKeyChecking accept-new
    ConnectTimeout 10
    # 重要：如果本地有代理软件（如 Clash / Surge），需要让 SSH 绕过代理
    ProxyCommand none
```

> **主机密钥校验**：`accept-new` 表示首次连接自动信任，但之后主机密钥一旦变更即拒绝连接，
> 因此仍保留 MITM 防护。只有在受信内网才可降级为 `StrictHostKeyChecking no`，
> 生产环境不要使用。

> **代理**：如果系统代理（macOS 网络偏好设置中的代理）拦截了 SSH 连接，
> 必须加 `ProxyCommand none` 来绕过它。

**3. 测试 SSH 连接：**
```bash
ssh my-server "uname -m"   # 应返回 x86_64 或 aarch64
```

**4. 使用 vcli 连接：**
```bash
# 方式一：使用 ~/.ssh/config 里的 Host 别名
VB_REMOTE_HOST=my-server vcli tunnel start

# 方式二：直接指定主机（用户名单独设置，不要写成 user@host）
VB_REMOTE_HOST=192.168.1.100 VB_REMOTE_USER=username vcli tunnel start

# 方式三：指定密钥或 SSH config 文件
VB_SSH_KEY=~/.ssh/id_ed25519_vcli VB_REMOTE_HOST=my-server vcli tunnel start
VB_SSH_CONFIG=~/.ssh/config_vcli VB_REMOTE_HOST=my-server vcli tunnel start
```

> `VB_REMOTE_HOST` 与 `VB_REMOTE_USER` 会被拼成 `user@host`，所以**不要**在
> `VB_REMOTE_HOST` 里写 `user@` 的同时又设置 `VB_REMOTE_USER` —— 那会拼出
> `user@user@host`。两种写法只用其一。

> `VB_REMOTE_HOST` 必须指向**运行 Virtuoso 的那台机器**（计算主机），不是跳板机；
> 跳板机填 `VB_JUMP_HOST`。`vcli tunnel status` 会报告配置主机名与实际主机名是否不一致。

**5. 备选方案：直接在远端主机上执行 vcli：**
```bash
# 如果 tunnel 有权限问题，可通过 SSH 直接调用远端的 vcli
ssh my-server 'vcli skill exec "version()"'
ssh my-server 'vcli session list'

# 设置别名方便使用
alias rvcli='ssh my-server vcli'
rvcli session list
```

**常见问题排查：**
```bash
# 检查代理设置（macOS）
networksetup -getwebproxy "Wi-Fi"

# 测试直连（绕过代理）
ssh -o "ProxyCommand=none" my-server "echo ok"

# 查看 vcli 详细日志
vcli tunnel start -v
```

### SSH 后端选择

`vcli` 在同一个 `RemoteTransport` 契约下提供两个 SSH 传输：

| 后端 | 选择方式 | 实际执行 |
|------|----------|----------|
| `openssh`（默认） | 不设置 / `VB_SSH_BACKEND=openssh` | 系统 `ssh` 客户端 — ControlMaster 多路复用，完整支持 `~/.ssh/config`（Host / ProxyJump / IdentityAgent / Match） |
| `native` | `VB_SSH_BACKEND=native` | 纯 Rust `russh` 客户端（由 `native-ssh` Cargo feature 门控） |

**没有自动迁移**。切换后端必须显式改 `VB_SSH_BACKEND`；已有 tunnel 会继续使用创建它们 `state.json` 时所用的后端，直到 `tunnel stop && tunnel start`。

**feature 门控的 build**。官方 `cargo install virtuoso-cli` 的二进制带 `--features native-ssh`；自定义构建如果去掉了该 feature，仍可选择 `openssh`；在该 build 上选择 `native` 会返回结构化 `UnsupportedBackend` 错误，**不会**静默回退到 OpenSSH。

`vcli tunnel status` JSON 会报告当前生效的选择：

```json
{
  "config": {
    "backend": {
      "selected": "openssh",
      "supported_in_build": true
    }
  },
  "tunnel": { "backend": "openssh" }
}
```

若 `config.backend.selected` 与 `tunnel.backend` 不一致，JSON 还会带一个 `config.backend_warning` 字段描述 drift —— 即 `state.json` 由另一个后端写入，而你当前运行的是另一个。

#### 能力矩阵（当前）

| 能力 | `openssh` | `native` |
|------|-----------|----------|
| 主机密钥校验（`known_hosts`） | 是（委托给 `ssh`） | 是（内置，支持传统 + `|1|salt|hash` 条目） |
| 公钥认证 | 是 | 是 |
| SSH agent forwarding | 是 | 否（选择此功能会返回 `UnsupportedOperation`） |
| 密码 / keyboard-interactive | 是（经 `ssh`） | 否（必须设置 `VB_SSH_KEY`） |
| `ProxyJump` / 多级跳板 | 是（委托给 `ssh`） | 否（仅单跳；返回 `UnsupportedOperation`） |
| 自定义 `~/.ssh/config`（`-F`） | 是（`VB_SSH_CONFIG`） | 否 — native 后端不读取 `VB_SSH_CONFIG`；设了它对 native 路径无效 |
| 单文件 SFTP streaming | 是（委托给 `sftp`/`scp`） | 是（russh-sftp 子系统，64 KiB chunk window） |
| 目录传输 | tar-over-exec | tar-over-exec（与 `openssh` 对齐） |
| 并发连接复用 | ControlMaster | 内置 endpoint pool + channel scheduler |
| 指数退避自动重连 | 由 `ssh` 负责 | 是（`VB_SSH_RECONNECT_MAX_ATTEMPTS` / `_MAX_DELAY`） |
| Keepalive 探测 | 委托给 `ssh`（`ServerAliveInterval`） | 是（`VB_SSH_KEEPALIVE_INTERVAL` / `_FAILURES`） |
| SOCKS5 转发 | 否（计划中） | 否（计划中） |
| RAMIC / X11 转发 | 是（委托给 `ssh`） | 否 |

如果你依赖右栏标记为 `否` 的任何能力，请暂时留在 `openssh`。如果是单跳、想要自包含的 Rust 二进制（无需宿主机 `/usr/bin/ssh`）、或看重内置连接池的延迟表现，再选择 `native`。

### GUI 调试

`vcli` 可以直接驱动 Virtuoso 的 X11 GUI 表单——无需屏幕抓取或 OCR。这使 AI Agent 能够在真实 Virtuoso session 上端到端测试 SKILL GUI 代码（表单、对话框、回调）。

[📊 交互式架构图：virtuoso-gui-debug × vcli 协同调试](https://htmlpreview.github.io/?https://github.com/deanyou/virtuoso-cli/blob/main/docs/virtuoso-gui-debug-architecture.html) · [📈 能力建设复盘](https://htmlpreview.github.io/?https://github.com/deanyou/virtuoso-cli/blob/main/docs/vcli-gui-debug-retrospective.html)

#### 窗口命令

```bash
# 发现指定 display 上的窗口（JSON：window_id、pid、title、几何信息）
vcli window list-windows-x11 --display :5.0 --format json

# 与指定窗口交互（服务端每次操作重新校验窗口身份）
vcli window action-x11 \
  --window-id 0x26037c7 --pid 114668 --display :5.0 \
  --operation click-rel --x 300 --y 167

# 支持的操作：click-rel、click-abs、type、key、drag-rel、activate、screenshot、scroll、minimize、maximize、double-click、close
vcli window action-x11 --window-id 0x26037c7 --pid 114668 --display :5.0 \
  --operation type --text "METAL1"
```

> **重要**：`vcli skill exec` 运行在 daemon 上下文中，**未加载 UI 库**（`hiCreateAppForm`、`hiDisplayForm` 等均为 nil）。因此 GUI 表单必须通过 **CIW** 创建/显示（用 xdotool 将 SKILL 代码输入 CIW 输入行），GUI 交互通过 `vcli window action-x11` 或 xdotool 完成。读取/设置表单字段值可通过 CIW（`form->field->value = "..."`）。

#### virtuoso-gui-debug 技能

仓库自带 `.claude/skills/virtuoso-gui-debug/`——一个确定性 GUI 调试技能，包含：

- **严格 JSON DSL** 用于可复现的测试场景（运行前先 validate）
- **三种执行器**：`fake`（离线回归）、`live`（真实 vcli）、`local`（直接 xdotool）
- **fail-closed 前置检查**：session/端口匹配、PID 正数、DISPLAY 一致、唯一窗口绑定、独占 X11 锁
- **多方法操作手册**——每个操作至少有 2 种独立验证的稳定方法

#### 多方法操作矩阵（IC25.1 真机验证）

| 操作 | 方法 1（推荐） | 方法 2 | 避免 |
|------|--------------|--------|------|
| 窗口发现 | `vcli window list-windows-x11 --format json` | `xdotool search --name "标题"` | — |
| 坐标获取 | SKILL 反推：`hiGetFieldInfo(form (quote field))` → `((x y) (w h))`，中心 = `(x+w/2, y+h/2)` | ImageMagick `import -window` + `convert -crop -resize` 像素级定位 | OCR 千分比框（漂移 ±40px） |
| 点击 | `xdotool mousemove --window <wid> <cx> <cy>; xdotool click 1`（窗口相对，无需 xwininfo） | `vcli window action-x11 --operation click-rel` | — |
| 文本输入 | `xdotool type --clearmodifiers --delay 50 "文本"`（聚焦字段后） | Tab 键导航到字段 + xdotool type；或 CIW 直接 `form->field->value = "文本"` | `vcli window action-x11 --operation type`（IC25.1 上注入乱码） |
| 提交/确认 | `xdotool key Return`（对话框中等效 Open/OK） | 点击按钮（click-rel 或 xdotool） | — |
| 关闭/取消 | `xdotool key Escape` | CIW `hiFormCancel(form)` | — |

#### 模态对话框处理

模态对话框（如 `hiDisplayFileDialog` 弹出的 "Choose a File"）**会拦截所有输入**——对父表单的点击会静默失败。任何 Browse/Open 操作后，先运行 `vcli window list-windows-x11` 检测是否有意外对话框，用 `Return`（提交）或 `Escape`（取消）关闭后**再**恢复父表单操作。

#### GUI 调试快速循环

```
1. validate SKILL → 2. scp 到远端 → 3. CIW: load("file.il")
4. CIW: showForm()  → 5. vcli list-windows-x11（获取 wid）
6. CIW: hiGetFieldInfo(form (quote field))  → 精确坐标
7. xdotool mousemove --window + click       → 交互
8. xdotool type / CIW 赋值                  → 输入
9. ImageMagick crop 截图                    → 视觉验证
10. CIW 截图 → 读取回调输出                 → 行为验证
11. 模态对话框？→ 优先处理
12. 发现 bug → 修 SKILL → 从第 2 步重复
```

### 工作原理

```
终端                          Virtuoso 进程
────                          ─────────────

vcli skill exec "1+2"
      │
      │ TCP: {"skill":"1+2"}
      ├──────────────────► virtuoso-daemon (port 42109)
      │                          │
      │                          │ evalstring("1+2") → "3"
      │                          │
      │ TCP: "3"
      ◄──────────────────────────┘
```

Session 注册流程：
```
vcli() in CIW
  → RBStart(): ipcBeginProcess(daemon, port=0)
  → OS 分配端口 N；daemon 打印 "PORT:N" 到 stderr
  → RBIpcErrHandler: RBPort=N，写入 session 文件
  → ~/.cache/virtuoso_bridge/sessions/<id>.json

vcli session list  # 读取 session 文件
vcli skill exec    # 连接到端口 N
```

---

## License / 许可证

MIT License — see [LICENSE](LICENSE)
