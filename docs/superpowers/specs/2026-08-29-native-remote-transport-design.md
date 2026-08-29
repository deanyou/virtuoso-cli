# Native Remote Transport Design

## Purpose and scope

`virtuoso-cli` will retain its existing OpenSSH transport and add a runtime-selectable, pure-Rust native SSH backend. The native backend must solve three requirements in its first stable release:

1. operate reliably on Windows without OpenSSH ControlMaster;
2. reuse authenticated SSH connections for concurrent Spectre, command, and file-transfer work;
3. route the complete remote path through an application-level SOCKS5 proxy when configured.

The native backend also owns the RAMIC local port forward. It does not invoke Python, Paramiko, `libssh2`, or another system SSH library. Official binaries include it; custom builds may disable it with a Cargo feature.

This design does not make the full application asynchronous. Existing business modules remain synchronous, while the native transport daemon contains the asynchronous runtime. OpenSSH remains the runtime default backend. The `native-ssh` Cargo feature is enabled in official release builds but may be disabled by custom builders. Backends are selected explicitly and never silently fall back to one another.

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

```text
X11 / Tunnel / Maestro / Spectre / Diagnostics
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

### Transport daemon

`vcli tunnel start` launches:

```text
vcli __transport-daemon --profile <profile>
```

The daemon owns all native connections for that profile, keeps credentials in memory, schedules channels, maintains local forwarding, and answers local IPC requests. It contains no Maestro, Spectre, schematic, layout, or SKILL business logic.

Ordinary commands do not implicitly start a missing daemon. They return `DaemonUnavailable`. Only `tunnel start` and `tunnel restart` create it. A changed resolved configuration produces `RestartRequired`; a running daemon does not silently change identity, proxy, route, or host-key policy.

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

### RAMIC forwarding

The daemon listens on `127.0.0.1:<local-port>`. Each accepted local TCP connection opens a `direct-tcpip` channel through the daemon-host SSH Transport to the configured RAMIC endpoint. This path uses the same SOCKS5 and jump routing as the rest of the native connection.

After an SSH reconnect the listener remains available for new connections and reconstructs new forwarding channels. Existing TCP streams fail; `VirtuosoClient` reconnects according to its existing connection semantics.

### Stop and crash recovery

`tunnel stop` stops admission, grants running work a bounded grace period, cancels remaining channels, closes forwards and SSH Transports, clears credential memory, and removes the IPC endpoint, token, and state files.

If the daemon does not respond, the CLI verifies the executable path, profile, instance nonce, and process identity before terminating it. PID alone is never enough because PIDs may be reused. Startup removes stale state only after proving that the recorded daemon is no longer valid.

## Channel scheduling

Each connection key has independent limits:

```dotenv
VB_SSH_MAX_SESSIONS=10
VB_SSH_MAX_BULK_SESSIONS=2
```

One urgent exec slot is reserved for health checks, cancellation, and cleanup. Bulk file and directory transfers may consume at most the bulk limit. Remaining permits serve normal commands and Spectre work. Requests are FIFO within a priority class; urgent work may move ahead of queued normal work but does not interrupt running work.

The long-lived RAMIC `direct-tcpip` path is accounted separately from exec/SFTP session permits. A request whose deadline expires before acquiring a permit returns `QueueTimeout`, proving that its remote operation did not begin.

If the server rejects a channel because of its limit, the daemon lowers the effective limit and reports that condition. It does not create an additional authenticated connection to bypass server policy.

## Reconnection and retry safety

The daemon reconnects network failures with exponential backoff and jitter, using credentials held in memory. It recreates the local forward after reconnect. Requests still waiting for a channel may continue waiting within their original deadline.

An operation already sent to the server is never replayed automatically. A lost command returns `OutcomeUnknown`; a lost transfer returns `TransferInterrupted`. The caller may resubmit only when it knows that operation is idempotent.

Host-key changes, rejected authentication, unsupported security policy, and proxy policy failures are permanent until user action. Repeated transient failures eventually open a circuit breaker and set the endpoint to `Degraded`; `vcli tunnel reconnect` explicitly resets it.

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

The token is stored separately with current-user-only permissions. The non-sensitive state file records the protocol version, profile, backend, PID, daemon nonce, IPC endpoint, token-file path, endpoint summaries, local forward, start time, health, and resolved-config digest.

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

1. extract `RemoteTransport` with zero OpenSSH behavior change;
2. add the versioned IPC protocol, state model, and fake daemon;
3. implement native direct connection, host-key verification, and authentication;
4. implement endpoint pooling, scheduling, SFTP, and transfer contracts;
5. implement SOCKS5 and one-hop jump routing;
6. implement native RAMIC forwarding, reconnect, and lifecycle;
7. complete Linux, macOS, and Windows integration matrices;
8. publish diagnostics, migration documentation, and stable status.

OpenSSH remains the default throughout this sequence. Native is marked stable only after the full matrix passes. No automatic backend migration occurs.

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
