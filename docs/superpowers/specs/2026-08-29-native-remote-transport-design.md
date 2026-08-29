# Native Remote Transport Design

## Purpose and scope

`virtuoso-cli` will retain its existing OpenSSH transport and add a runtime-selectable, pure-Rust native SSH backend. The native backend must solve three requirements in its first stable release:

1. operate reliably on Windows without OpenSSH ControlMaster;
2. reuse authenticated SSH connections for concurrent Spectre, command, and file-transfer work;
3. route the complete remote path through an application-level SOCKS5 proxy when configured.

The native backend also owns the RAMIC local port forward. It does not invoke Python, Paramiko, `libssh2`, or another system SSH library. Official binaries include it; custom builds may disable it with a Cargo feature.

This design does not make the full application asynchronous. Existing business modules remain synchronous, while the native transport daemon contains the asynchronous runtime. OpenSSH remains the runtime default backend. The `native-ssh` Cargo feature is enabled in official release builds but may be disabled by custom builders. Backends are selected explicitly and never silently fall back to one another.

## Terminology

Two unrelated things in this project are called a daemon, and they run on
different machines. The distinction matters in `tunnel status` output, in the
state file, and in process-identity handling.

| Term | What it is | Where it runs |
|---|---|---|
| **Transport daemon** (this design) | Local background process owning native SSH connections for one profile. Hidden mode of the `vcli` binary: `vcli __transport-daemon` | Local machine |
| `virtuoso-daemon` (pre-existing) | The RAMIC/HBridge bridge that Virtuoso talks to. Deployed to the remote host by `tunnel start`, feature-gated binary | Remote compute host |
| RAMIC forward | Local TCP listener that reaches the remote `virtuoso-daemon` through an SSH `direct-tcpip` channel | Listener local, target remote |

There is no binary name collision — the new process is a hidden subcommand of
`vcli`, not an installed binary — but documentation and status output must say
which one they mean.

## Decisions

- `VB_SSH_BACKEND=openssh` remains the default; `native` selects the new backend.
- The native backend uses one hidden local transport-daemon process per profile.
- The daemon is another mode of the existing `vcli` binary, not a separate installed binary.
- Connections are deduplicated by resolved SSH endpoint inside one profile. Profiles never share connections or credentials.
- The native backend handles commands, files, jump routing, SOCKS5, and RAMIC `direct-tcpip` forwarding.
- Interactive credentials are cached only in daemon memory until that daemon exits.
- Unknown host keys may be trusted only through an explicit interactive confirmation. Changed host keys always fail.
- A connection may be rebuilt after failure, but an operation that may have reached the remote host is never replayed automatically.
- Local IPC uses Unix domain sockets on Unix/macOS and Named Pipes on Windows.

## Architecture

Business modules depend on a transport contract rather than the current concrete `SSHRunner`:

Session sync is in scope for the extraction: `SessionInfo::sync_from_remote`
currently takes a concrete `&SSHRunner` and is called from `tunnel start`
(`src/commands/tunnel.rs`) and from session listing. It needs only `run_command`
and directory download, so it moves behind `RemoteTransport` with the rest.

```text
X11 / Tunnel / Maestro / Spectre / Diagnostics / SessionSync
                    |
                    v
             RemoteTransport
                    |
          +---------+----------+
          |                    |
          v                    v
 OpenSshTransport      NativeTransportClient
 current subprocess             |
 implementation                 v
                         secured local IPC
                                |
                                v
                       TransportDaemon
                       + ConnectionPool
                       + ChannelScheduler
                       + CredentialVault
                       + HostKeyVerifier
                       + PortForwardManager
                       + HealthReporter
```

### RemoteTransport

The business-facing contract exposes only transport semantics:

```text
test_connection(deadline)
run_command(request)
upload_file(request)
upload_text(request)
download_file(request)
download_dir(request)
start_local_forward(request)
stop_local_forward(id)
health()
shutdown()
```

Every request has a request ID and absolute deadline. Results distinguish remote exit, queue timeout, execution timeout, cancellation, interrupted transfer, and unknown execution outcome. The contract does not expose russh, Tokio, SSH channels, or IPC types.

`OpenSshTransport` evolves from `SSHRunner` and preserves current behavior, including ControlMaster and its existing same-backend fallback. `NativeTransportClient` is a synchronous IPC client. Only the native transport daemon owns the async runtime.

#### Concurrency and sharing

The contract must be shareable across threads:

```rust
pub trait RemoteTransport: Send + Sync { /* … */ }
```

The bound is a hard requirement, not a convenience. Parallel Spectre work already
shares one transport handle by reference across scoped threads: `run_parallel`
creates a single `Simulator` clone outside `std::thread::scope` and every worker
borrows it (`src/spectre/runner.rs`). `SSHRunner` is `Send + Sync` today by virtue
of a `Mutex<bool>` and owned `String` fields; a bare `dyn RemoteTransport` is
neither, so the bound must be declared on the trait itself.

Business modules hold `Arc<dyn RemoteTransport>`. Implementations must not assume
one handle per thread. Because each worker drains its own chunk sequentially,
peak concurrent remote operations is bounded by the worker count rather than by
the number of commands a single job issues; capacity planning must follow the
invariant in [Channel scheduling](#channel-scheduling).

Because workers share one handle, `RemoteTransport` never replays an operation —
see [SKILL request retry policy](#skill-request-retry-policy).

### Transport daemon

`vcli tunnel start` launches:

```text
vcli __transport-daemon --profile <profile>
```

The daemon owns all native connections for that profile, keeps credentials in memory, schedules channels, maintains local forwarding, and answers local IPC requests. It contains no Maestro, Spectre, schematic, layout, or SKILL business logic.

Ordinary commands do not implicitly start a missing daemon. They return `DaemonUnavailable`. Only `tunnel start` and `tunnel restart` create it. A changed resolved configuration produces `RestartRequired`; a running daemon does not silently change identity, proxy, route, or host-key policy.

#### Commands affected by `DaemonUnavailable`

Not starting a daemon implicitly changes behaviour for commands that currently
assume a transport is always obtainable. The inventory below is part of the
design, not an implementation detail, because it determines which commands are
allowed to regress and which must not.

| Command family | Current transport use | Under the native backend |
|---|---|---|
| `tunnel start/stop/restart/status/diagnose` | `SSHClient::from_env` | Owns daemon lifecycle; unchanged semantics |
| `session list/show` | `SSHClient::from_env`, already wrapped in a best-effort `if let Ok(..)` | `DaemonUnavailable` is swallowed as today — no visible change |
| `window` (X11) | `x11::runner_for_config`, four call sites | Holds `Arc<dyn RemoteTransport>` |
| `sim` (Spectre) | `SSHRunner::from_config` | Holds `Arc<dyn RemoteTransport>` |
| `maestro` | `SSHRunner::from_config` | Holds `Arc<dyn RemoteTransport>` |
| `diag` | `SSHRunner::from_config` | Holds `Arc<dyn RemoteTransport>` |
| `skill exec`, `skill sync` | **Direct `Command::new("ssh")`, bypassing `SSHRunner`** | **Backend leak — see below** |

The last row is the one that would otherwise be missed. `src/commands/skill.rs`
and `src/skill_finder/mod.rs` spawn `ssh` directly rather than going through
`SSHRunner`. Extracting `RemoteTransport` therefore does not bring them along, and
under the native backend they would still shell out to OpenSSH — silently
ignoring `VB_SSH_BACKEND`, skipping the SOCKS5 route, and bypassing the daemon.

That is out of scope for the extraction increment, which must be behaviour
preserving, but it is in scope for this design: before the native backend is
marked stable these paths either move behind `RemoteTransport` or explicitly
document that they remain OpenSSH-only. Leaving them undocumented would make the
backend selection a partial setting.

### Connection identity and sharing

Connections use an immutable key composed of:

```text
host + port + user + complete jump route + SOCKS route
+ per-hop identities + per-hop HostKeyAlias values
+ security-relevant SSH options
```

GUI, deploy, daemon, and Spectre roles that resolve to the same key share one authenticated SSH Transport. Different endpoints use separate Transports. Profiles are always isolated, even if their endpoint keys are otherwise identical.

Endpoint aliases are not guessed to be equivalent. Two differently resolved keys create two connections unless configuration resolution produces the same canonical key.

## Lifecycle and data flow

### Start

`vcli tunnel start` resolves the profile, remote roles, backend, SSH config, proxy, identity, and host-key sources. For `openssh`, it runs the existing flow. For `native`, it creates the profile IPC endpoint and startup lock, launches the hidden daemon, and completes any `AuthRequired` or `HostTrustRequired` exchanges in the foreground.

Start succeeds only after all required endpoint connections are ready, the RAMIC local forward is established, and the local port passes a reachability check. The resulting state file records only non-sensitive metadata.

### Interactive authentication

The daemon never reads the terminal. It sends a nonce-bound challenge to the foreground `vcli`, which securely collects and returns the response over authenticated IPC. Supported mechanisms are:

- explicit OpenSSH private keys;
- standard Unix and Windows OpenSSH agents;
- encrypted-key passphrases;
- password authentication;
- multi-round keyboard-interactive authentication.

Credentials are not placed in arguments, environment variables, state files, logs, or crash reports. A challenge response is single-use. The daemon stores retained secrets in zeroizing containers and clears them on shutdown or drop; this is best-effort process-memory protection, not protection against a debugger or memory dump of the running user process. The daemon may retain credentials for reconnects until `tunnel stop` or daemon exit. If a non-interactive invocation encounters a challenge, it returns `InteractionRequired`.

### Commands

A synchronous business call becomes an authenticated IPC request. The daemon resolves the requested remote role into a connection key, waits for a scheduler permit within the original deadline, opens an SSH exec channel, and returns structured stdout, stderr, and exit status. Queue time and execution time share one deadline.

### File transfer

Single files use SFTP streaming. Directory transfer initially retains the proven tar-over-exec approach rather than recursive SFTP. Large file contents do not cross IPC; requests carry validated local and remote paths, and the daemon streams the local file itself.

Text upload retains temporary-file creation, byte-length and hash verification, mode preservation, and atomic publication. Downloads write to sibling staging and publish atomically only after successful completion and validation. Existing destinations are not overwritten by default. Interrupted transfers never publish partial files.

`upload_text` needs an explicit rule because its payload is not a file the caller
already has on disk: today the text is written straight into the SSH process
stdin, and generated SKILL payloads can be large. Under the native backend the
client writes the content to a protected local temporary file (owner-only mode,
created in the profile temp root) and the IPC request carries the path, the byte
length, and the SHA256 rather than the content. The daemon validates all three
before streaming, and removes the temporary file when the transfer settles
regardless of outcome. This keeps text upload inside the 8 MiB IPC frame limit
without special-casing it.

Directory transfer keeps the tar-over-exec approach on both backends, so the two
backends do not diverge in their remote toolchain requirements. "Remote host has
no `tar`" is therefore a pre-existing constraint, not a native-backend
regression, and is covered by the shared contract suite rather than by a
native-only test.

### RAMIC forwarding

The daemon listens on `127.0.0.1:<local-port>`. Each accepted local TCP connection opens a `direct-tcpip` channel through the daemon-host SSH Transport to the configured RAMIC endpoint. This path uses the same SOCKS5 and jump routing as the rest of the native connection.

After an SSH reconnect the listener remains available for new connections and reconstructs new forwarding channels. Existing TCP streams fail.

That changes a load-bearing assumption on the client side. Under OpenSSH `-L`
forwarding a new TCP connection to the local listener is cheap and close to
stateless: it reuses the one SSH transport and reaches the same remote endpoint.
Under native forwarding each accepted connection opens its own `direct-tcpip`
channel and therefore its own socket to the RAMIC endpoint. Any client logic that
treats "open a fresh TCP connection and resend" as a free retry — including the
bounded retry loop in `VirtuosoClient::execute_skill` — must be re-examined before
the native backend ships, because the remote side may attribute state to a
connection where the previous backend attributed none. See
[SKILL request retry policy](#skill-request-retry-policy).

### Stop and crash recovery

`tunnel stop` stops admission, grants running work a bounded grace period, cancels remaining channels, closes forwards and SSH Transports, clears credential memory, and removes the IPC endpoint, token, and state files.

PID alone is never sufficient to identify a process, because PIDs are reused. The
state file therefore records, for every daemon instance, the instance nonce, the
executable path, and a start identity. Termination requires two independent
checks, and which one applies depends on whether the daemon can still answer.

**Tier 1 — daemon responds.** The CLI issues an IPC nonce challenge. A correct
response proves the process on the other end is the recorded daemon. This is the
normal path and needs no platform-specific code.

**Tier 2 — daemon does not respond.** A hung, wedged, or otherwise unresponsive
daemon cannot complete a challenge, and that is precisely the case where an
operator most needs to force termination. Falling back to the PID here would
reintroduce the risk the nonce exists to remove, so Tier 2 requires an operating
system identity match on **all three** recorded attributes before any signal is
sent:

| Platform | Mechanism |
|---|---|
| Linux | `/proc/<pid>` for the executable path and start time; `pidfd` where available (5.3+) to pin the process identity across the check and the signal |
| macOS | `libproc::proc_pidinfo`, or `sysctl` `KERN_PROCARGS2` for the path plus the process start time. **macOS has neither `/proc` nor `pidfd`** — it is not a Linux variant here |
| Windows | `OpenProcess` with `GetModuleFileNameEx` for the path and `GetProcessTimes` for the creation time |

If any attribute fails to match, the CLI refuses to signal and reports the stale
state instead, leaving the operator to remove it explicitly. A Tier 2 kill guarded
by nothing but a PID is never performed on any platform.

Startup removes stale state only after proving that the recorded daemon is no
longer valid, using the same two tiers in the same order.

The current OpenSSH path trusts the PID on non-Unix platforms
(`verify_ssh_pid` returns `true` when `/proc` is absent) and `tunnel stop` on
Windows issues `taskkill /F` unconditionally. Closing that gap is part of this
work, not a downstream cleanup.

## Channel scheduling

Each connection key has independent limits:

```dotenv
VB_SSH_MAX_SESSIONS=10
VB_SSH_MAX_BULK_SESSIONS=2
```

One urgent exec slot is reserved for health checks, cancellation, and cleanup. Bulk file and directory transfers may consume at most the bulk limit. Remaining permits serve normal commands and Spectre work. Requests are FIFO within a priority class; urgent work may move ahead of queued normal work but does not interrupt running work.

The long-lived RAMIC `direct-tcpip` path is accounted separately from exec/SFTP session permits. A request whose deadline expires before acquiring a permit returns `QueueTimeout`, proving that its remote operation did not begin.

#### Capacity invariant

Session capacity and Spectre worker count are coupled and must satisfy:

```text
VB_SSH_MAX_SESSIONS >= VB_SPECTRE_MAX_WORKERS + control_reserve
```

where `control_reserve` covers the urgent slot plus ordinary foreground commands
issued while a sweep is running. The defaults satisfy this with room to spare
(`10 >= 8 + 2`), because each Spectre worker issues its commands sequentially and
therefore occupies at most one exec session at a time — a worker does not hold one
session per command.

Configuration validation rejects a combination that violates the invariant rather
than degrading later into spurious `QueueTimeout` errors. The preferred remedy for
saturation is reserving capacity for urgent and control work, not raising the
session total.

If the server rejects a channel because of its limit, the daemon lowers the effective limit and reports that condition. It does not create an additional authenticated connection to bypass server policy.

## Reconnection and retry safety

The daemon reconnects network failures with exponential backoff and jitter, using credentials held in memory. It recreates the local forward after reconnect. Requests still waiting for a channel may continue waiting within their original deadline.

An operation already sent to the server is never replayed automatically. A lost command returns `OutcomeUnknown`; a lost transfer returns `TransferInterrupted`. The caller may resubmit only when it knows that operation is idempotent.

Host-key changes, rejected authentication, unsupported security policy, and proxy policy failures are permanent until user action. Repeated transient failures eventually open a circuit breaker and set the endpoint to `Degraded`; `vcli tunnel reconnect` explicitly resets it.

## SKILL request retry policy

The invariant above governs the transport layer only. It does not currently hold
in the client above it, and the native backend makes that gap load-bearing.

`VirtuosoClient::execute_skill` wraps a request in a bounded retry loop that opens
a **fresh TCP connection on every iteration** and resends the identical request
when the response parses as a queued-ticket marker (`sync_N`). Today that is
harmless because a new connection over `-L` forwarding is cheap and stateless.
Under native forwarding each iteration is a new `direct-tcpip` channel and a new
socket at the RAMIC endpoint, so the remote side may attribute state to a
connection where the previous backend attributed none.

The producer of that ticket marker is not vendored in this repository — it lives
in the upstream CIW-side bridge — so this design does **not** assert that a
transport reconnect produces one. What it asserts is narrower and provable: the
client may resend a request up to ten times, and each resend is a new connection.
A retry policy that is safe only if the remote side never notices is not a policy.

Required before the native backend is marked stable:

- `RemoteTransport` never replays an operation. Reconnection re-establishes the
  path; it does not re-issue work.
- SKILL requests default to `RetryPolicy::Never`. A request is transmitted once.
- When the client observes a queued-ticket marker, a request that is not
  explicitly marked idempotent returns `OutcomeUnknown` instead of being resent.
- Only requests explicitly marked as idempotent probes may be re-sent, and only
  within the original deadline.
- Request IDs and idempotency keys are the durable fix, but they require a RAMIC
  protocol change and are therefore a follow-on upgrade, not a substitute for the
  policy above. Transport generation counters alone do not solve this: they prove
  the path changed, not whether the remote side executed the previous attempt.
- Under the native backend the retry loop's channel cost is bounded explicitly, so
  one logical request cannot open an unbounded number of forwarding channels.

Until this is enforced, non-idempotent SKILL — transaction commit above all — must
not run over the native backend.

## Host-key and SSH configuration policy

Known host keys are checked against the configured user and global `known_hosts` files. The implementation supports hashed entries, port-qualified entries, host aliases, and `HostKeyAlias`. Jump and target hosts are checked independently.

An unknown key in interactive `tunnel start` displays the algorithm and SHA256 fingerprint. Explicit acceptance writes the user `known_hosts` file atomically. Non-interactive unknown keys fail. Changed keys always fail and cannot be overwritten by a convenience prompt.

The native config resolver supports:

- `Host`, `HostName`, `User`, and `Port`;
- `Include`, with bounded recursion, cycle detection, and normal OpenSSH precedence;
- `IdentityFile` and `IdentitiesOnly`;
- one `ProxyJump` hop;
- `UserKnownHostsFile`, `GlobalKnownHostsFile`, and `HostKeyAlias`;
- `ConnectTimeout`, `ServerAliveInterval`, and `ServerAliveCountMax`.

`ProxyCommand`, chained `ProxyJump`, a custom unsupported `IdentityAgent`, or an unenforceable `RevokedHostKeys`/KRL policy causes native startup to fail with a specific diagnostic. Security-, route-, or authentication-affecting directives are never silently ignored. Irrelevant display or compression directives may be ignored but appear in diagnostic output.

## SOCKS5

`VB_SSH_PROXY=socks5://host:port` configures a first-hop SOCKS5 proxy. `VB_SSH_PROXY_USER` optionally supplies its user name. Passwords are collected interactively and retained only in daemon memory; passwords embedded in URLs or plaintext password environment variables are rejected.

The proxy resolves the proxied first-hop hostname to avoid local DNS leakage. With a jump host the route is SOCKS5 to jump SSH, then SSH `direct-tcpip` to the target SSH server. Without a jump host the proxy connects directly to the target SSH server.

Non-interactive automation may use an unauthenticated loopback SOCKS endpoint. Supporting a persistent external secret store is outside the first version.

## Local IPC protocol

Unix and macOS use a Unix domain socket with mode `0600`. Windows uses a Named Pipe whose ACL permits only the current user SID. Each profile has a separate endpoint. A random 256-bit session token provides an additional application-level check.

Unix domain socket paths are bounded by `sun_path` (103–104 usable bytes on
macOS and Linux). A profile-named socket under the default cache root can
approach that limit, so the endpoint path is length-checked at creation; when the
candidate exceeds the limit the daemon falls back to a short hashed socket name
under a fixed short directory, and records the chosen path in the state file rather
than recomputing it. Profile separation is preserved by the hash, not by the
profile name appearing in the path.

Messages use four-byte big-endian length-prefixed UTF-8 JSON, with an 8 MiB maximum frame. File contents are never carried in these frames. Every request includes:

```text
protocol_major
protocol_minor
profile
daemon_nonce
auth_token
request_id
deadline_unix_ms
operation
payload
```

Connection setup uses `Hello`/`HelloAck`. A major mismatch fails. Minor versions use capability negotiation. Unknown fields are ignored; unknown operations return `UnsupportedOperation`. Cancellation is an independent request keyed by request ID. A daemon restart changes its nonce and invalidates stale clients.

The token is stored separately with current-user-only permissions. The non-sensitive state file records the protocol version, profile, backend, PID, daemon nonce, executable path, start identity, IPC endpoint, token-file path, endpoint summaries, local forward, start time, health, and resolved-config digest.

#### State-file versioning

The state file replaces the existing `TunnelState` (currently
`{version, port, pid, remote_host, setup_path}`), which `tunnel stop`,
`tunnel status`, and `tunnel diagnose` already read. Migration rules:

- a v1 file lacking a `backend` field is read as `backend = openssh`, so existing
  OpenSSH tunnels keep working without user action;
- every write after this change is v2, including writes made by the OpenSSH
  backend, so the file does not oscillate between shapes;
- v2 fields absent from v1 (`daemon_nonce`, `executable_path`, `start_identity`,
  IPC endpoint, token path) are `Option`-typed so a v1 file parses without a
  custom deserializer.

Two-tier process identification needs the executable path and start identity, so
they are recorded for the OpenSSH backend too — `tunnel stop` cannot verify a
process it never recorded.

## Configuration

The new settings are:

```dotenv
VB_SSH_BACKEND=openssh
VB_SSH_MAX_SESSIONS=10
VB_SSH_MAX_BULK_SESSIONS=2
VB_SSH_PROXY=socks5://127.0.0.1:1080
VB_SSH_PROXY_USER=
VB_SSH_KEEPALIVE_INTERVAL=30
VB_SSH_KEEPALIVE_FAILURES=3
VB_SSH_RECONNECT_MAX_ATTEMPTS=8
VB_SSH_RECONNECT_MAX_DELAY=30
VB_TRANSPORT_SHUTDOWN_GRACE=10
```

Existing remote-host, role, jump-host, identity, SSH config, timeout, and profile variables remain valid. Resolution order is explicit CLI arguments, profile-specific environment variables, general environment variables, SSH config, then built-in defaults.

### Backend-specific settings

`VB_DISABLE_CONTROL_MASTER` is OpenSSH-only. Under the native backend it is
reported as `not_applicable` in `tunnel status` diagnostics and ignored. It is not
an error: it is a workaround for a multiplexing mechanism the native backend does
not use, not a security switch. Treating it as an error would break users who set
it once in a shared `.env`.

### Feature-gated builds

`native-ssh` is a Cargo feature. When a build is produced without it, selecting
`VB_SSH_BACKEND=native` returns structured `UnsupportedBackend` rather than an
unrecognised-value error or a silent fallback to OpenSSH. The hidden
`__transport-daemon` subcommand is absent from such builds. Build-time cost,
binary size, and dependency footprint are measured during the compatibility spike
in [Dependency boundary](#dependency-boundary) and recorded there; no targets are
preset in this design.

## Error model

The transport boundary reports structured codes:

```text
Configuration
DaemonUnavailable
ProtocolMismatch
AuthenticationFailed
InteractionRequired
HostKeyUnknown
HostKeyChanged
HostKeyPolicyUnsupported
ProxyFailed
JumpFailed
ConnectionFailed
QueueTimeout
ExecutionTimeout
RemoteExit
OutcomeUnknown
TransferInterrupted
IntegrityMismatch
LocalIo
RemoteIo
Cancelled
RestartRequired
UnsupportedOperation
UnsupportedBackend
```

`RemoteExit` proves the server command exited and includes its status. `QueueTimeout` proves no remote operation started. `ExecutionTimeout` records whether remote termination was proven; otherwise it carries unknown-outcome semantics. `OutcomeUnknown` means an operation may have executed. The public mapping into `VirtuosoError` preserves stable CLI exit codes and does not use message-string matching to decide retry behavior.

## Health and observability

An endpoint moves through `Starting`, `Authenticating`, `Ready`, `Reconnecting`, `Degraded`, `PermanentFailure`, `Stopping`, and `Stopped`. Profile status is `Ready` only when all required endpoints and the local RAMIC forward are usable. A host-key or authentication action produces `ActionRequired`.

Native `tunnel status` reports backend, daemon PID, IPC health, config-digest match, endpoint states, active and queued channel counts, bulk usage, reconnect attempts, last success, last structured error, local-forward state, credential source, and non-sensitive proxy mode.

Logs may record request IDs, operation classes, sanitized endpoints, timing, byte counts, channel utilization, and structured errors. They never record credentials, tokens, private keys, command bodies, SKILL, netlists, user file contents, or credential-bearing URLs. Command text is represented only by byte length and SHA256 unless a separate safe diagnostic explicitly supplies redacted content.

## Dependency boundary

A pure-Rust SSH implementation such as russh is the preferred candidate, but the crate is contained behind the native adapter. Before pinning it, a compatibility spike must verify public-key, password, keyboard-interactive, Windows agent, SFTP, `direct-tcpip`, jump routing, host-key algorithms, keepalive, and channel cancellation.

Changing the internal SSH crate must not change `RemoteTransport` or the IPC protocol. Tokio and SSH-crate types remain private to the native daemon.

## Testing

### Shared contract tests

The same behavioral suite runs against OpenSSH and native transports: command output and exit status, deadline, cancellation, text and file transfer, atomic publication, directory transfer, path safety, error classification, and unknown outcome after interruption.

### Native integration topology

CI creates an isolated SOCKS5 fixture, jump SSH server, target SSH server, SFTP subsystem, and TCP echo endpoint. Tests cover direct, SOCKS-direct, and SOCKS-jump-target routes; no-auth and username/password SOCKS; key, encrypted key, password, and keyboard-interactive authentication; known, unknown, changed, and hashed host keys; separate jump/target verification; proxy-side DNS; and local `direct-tcpip` forwarding.

### Concurrency and isolation

Tests prove that 100 concurrent short commands use one authentication, configured session and bulk limits are never exceeded, urgent work can pass queued normal work, equal-priority work is FIFO, queue deadlines are honored, roles with one key share a Transport, and profiles never share. Server channel rejection must reduce capacity rather than create another connection.

### Fault injection

Tests cut SSH, SOCKS, jump, IPC, transfers, and command responses at controlled points. They also crash the daemon, corrupt IPC frames and tokens, change host keys during reconnect, and invalidate credentials. Assertions prove no command replay, no partial publication, correct unknown-outcome reporting, forward reconstruction, permanent-error handling, log redaction, and stale-PID safety.

Linux runs the complete topology and stress suite. macOS covers IPC, known-hosts behavior, agent, permissions, and forwarding. Windows covers Named Pipe ACLs, Windows OpenSSH agent, interactive authentication, daemon lifecycle, paths, and native operation without ControlMaster, Unix sockets, or dynamic SSH libraries.

## Delivery sequence

The first public stable release includes Windows support, concurrent connection reuse, and SOCKS5. Development is still divided into reviewable, reversible increments:

0. **Contract test harness against the OpenSSH backend.** The shared behavioral
   suite from [Testing](#testing) runs against `OpenSshTransport` before any
   native code exists. This is a gate, not a task: it establishes that the
   contract is observable and that the extraction in step 1 is behaviour
   preserving. It also flushes out what the contract cannot express while both
   backends are still cheap to change.
1. extract `RemoteTransport` with zero OpenSSH behavior change, including
   `Send + Sync` and the `Arc<dyn RemoteTransport>` handoff;
2. add the versioned IPC protocol, state model, and fake daemon — and re-run the
   step 0 suite against the fake daemon;
3. **single-hop SSH server fixture** (one target, key authentication) plus native
   direct connection, host-key verification, and authentication. The fixture lands
   *before* the code it tests; later steps extend it rather than building it;
4. implement endpoint pooling, scheduling, SFTP, and transfer contracts;
5. implement SOCKS5 and one-hop jump routing, extending the step 3 fixture with a
   proxy and a jump hop;
6. implement native RAMIC forwarding, reconnect, and lifecycle — and enforce the
   [SKILL request retry policy](#skill-request-retry-policy), without which the
   native backend must not serve transaction commands;
7. complete Linux, macOS, and Windows integration matrices;
8. publish diagnostics, migration documentation, and stable status.

OpenSSH remains the default throughout this sequence. Native is marked stable only after the full matrix passes. No automatic backend migration occurs.

The original ordering placed the whole integration matrix second to last, which
would have landed direct connection, pooling, SOCKS5, and RAMIC forwarding with no
verification net. The full topology described in
[Testing](#testing) is the largest and least predictable piece of this project;
building it last is what makes it risky. Steps 0 and 3 exist to move that cost
forward, so that every later increment extends a fixture that already works
instead of debugging the fixture and the feature at the same time.

## Out of scope

- converting the full application to async;
- sharing connections or credentials across profiles;
- automatic backend fallback;
- automatic command replay;
- transfer resume;
- chained ProxyJump;
- arbitrary ProxyCommand execution in native mode;
- persistent password storage or external credential-vault integration;
- system-wide multi-user transport service;
- changing Maestro, Spectre, X11, or SKILL business semantics.
