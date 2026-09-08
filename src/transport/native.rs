//! Native (russh-based) SSH transport — step 3 of the native-transport plan.
//!
//! This is the *direct-connect* client: a single SSH connection per operation,
//! public-key auth, and `known_hosts` host-key verification. It implements the
//! same `RemoteTransport` contract as `OpenSshTransport`, so business modules
//! hold it behind `Arc<dyn RemoteTransport>` exactly as they do today.
//!
//! Scope (this increment) vs the full design:
//! - ✅ single-hop direct connection (`VB_REMOTE_HOST` only);
//! - ✅ host-key verification via the existing `host_keys` module (plaintext +
//!   hashed `known_hosts`, port-qualified entries);
//! - ✅ public-key auth (`VB_SSH_KEY`);
//! - ✅ command exec + file/text/dir transfer; single files stream over the
//!   SFTP subsystem (design step 4) with an exec `cat` fallback for remotes
//!   that do not advertise sftp, and directories keep tar-over-exec
//!   (matching the OpenSSH backend);
//! - ✅ connection reuse (P1-2): one live SSH connection per transport,
//!   established lazily and reused across operations; connection-level
//!   failures clear the slot so the next operation re-establishes. Verified
//!   by handshake count (not factory calls); the endpoint pool drives
//!   generation/reconnect policy;
//! - ❌ `ProxyJump` / jump-host routing, SOCKS5, RAMIC `direct-tcpip` forward,
//!   agent/password/keyboard-interactive auth, and the IPC transport-daemon —
//!   all later increments. Those paths return a clear `UnsupportedOperation`
//!   rather than silently misbehaving.
//!
//! The contract methods are synchronous; russh is async. The module owns one
//! multi-thread tokio runtime per transport (kept in `SessionState`) so the
//! russh session task — including its keepalive loop — keeps running even
//! between operations. Each operation drives its async work through
//! `block_on` against that shared runtime. The session handle is guarded only
//! while a channel is being opened; once the channel exists it is independent
//! of the handle, so concurrent commands run on separate channels without
//! serialising. Waiting for the handle, establishing the connection (connect +
//! auth + reconnect retries) and the operation itself all share the request's
//! deadline budget.

#![cfg(feature = "native-ssh")]

use std::path::{Path, PathBuf};
use std::process::Command as SyncCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;

use base64::Engine;
use russh::client::{self, Handler};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::ChannelMsg;
use sha2::{Digest, Sha256};
use shlex;

use crate::config::Config;
use crate::transport::contract::{
    CommandRequest, CommandResult, Deadline, DownloadDirRequest, DownloadFileRequest,
    RemoteTransport, RequestId, TransportError, UploadFileRequest, UploadTextRequest,
};
use crate::transport::host_keys::{KeyType, KnownHosts, Verification};
use crate::transport::lifecycle::{FailureClass, KeepalivePolicy, ReconnectPolicy};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Resolved, backend-local view of the SSH endpoint.
#[derive(Clone, Debug)]
pub struct NativeTransportConfig {
    pub host: String,
    pub user: Option<String>,
    pub ssh_port: u16,
    pub jump_host: Option<String>,
    /// Identity file (`VB_SSH_KEY`). Required for step 3 — agent/password/keys
    /// are later increments.
    pub key_path: Option<PathBuf>,
    /// `known_hosts` stores, most-specific first (user then global).
    pub known_hosts: Vec<KnownHosts>,
    /// TCP + SSH handshake budget.
    pub connect_timeout: Duration,
    /// Liveness probing for the established connection
    /// (`VB_SSH_KEEPALIVE_INTERVAL` / `_FAILURES`). A dead NAT or a silently
    /// dropped path becomes observable as a transient failure instead of a
    /// hang; russh then drops the session and the next operation reconnects.
    pub keepalive: KeepalivePolicy,
    /// Bounded backoff for re-establishing the *connection*
    /// (`VB_SSH_RECONNECT_MAX_ATTEMPTS` / `_MAX_DELAY`). Applies to the
    /// connect+auth phase only — never to an operation that may have reached
    /// the remote host (the design's no-replay invariant).
    pub reconnect: ReconnectPolicy,
}

/// Outcome of the host-key callback, captured so `establish` can surface a
/// structured `TransportError` even though russh only reports a generic failure.
struct HostKeyCheck {
    verification: Verification,
    fingerprint: Option<String>,
}

/// russh `Handler`: exists only to verify the server host key.
struct NativeClientHandler {
    host: String,
    port: u16,
    stores: Vec<KnownHosts>,
    verification: Arc<Mutex<Option<HostKeyCheck>>>,
}

impl Handler for NativeClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let (kt, b64, fingerprint) = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                let key_bytes = match key.to_bytes() {
                    Ok(b) => b,
                    // Unparseable key: treat as unverifiable and reject.
                    Err(_) => {
                        *self.verification.lock().unwrap() = Some(HostKeyCheck {
                            verification: Verification::Unknown,
                            fingerprint: None,
                        });
                        return Ok(false);
                    }
                };
                let b64 = base64::engine::general_purpose::STANDARD.encode(&key_bytes);
                let kt = KeyType::from_known_hosts_token(key.algorithm().as_str());
                let digest = Sha256::digest(&key_bytes);
                let fingerprint = format!(
                    "SHA256:{}",
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
                );
                (kt, b64, Some(fingerprint))
            }
            // Host certificates are out of scope for step 3; reject.
            PublicKeyOrCertificate::Certificate(_) => {
                *self.verification.lock().unwrap() = Some(HostKeyCheck {
                    verification: Verification::Unknown,
                    fingerprint: None,
                });
                return Ok(false);
            }
        };

        let verification =
            verify_against_stores(&self.stores, &self.host, Some(self.port), &kt, &b64);
        *self.verification.lock().unwrap() = Some(HostKeyCheck {
            verification: verification.clone(),
            fingerprint,
        });
        Ok(matches!(verification, Verification::Trusted))
    }
}

/// Load a `known_hosts` store, falling back to an empty in-memory store on any
/// read error. A missing file and a denied permission are both equivalent to
/// "no prior trust" — the caller discovers an unknown host on first connection.
#[allow(dead_code)] // wired on Unix by open_transport_for_daemon and the pooled daemon; non-Unix builds have neither
fn load_known_hosts_or_empty(path: &Path) -> KnownHosts {
    match KnownHosts::load(path) {
        Ok(kh) => kh,
        Err(_) => KnownHosts::memory(),
    }
}

/// Check a presented key against each store in order. A `Trusted` match wins
/// immediately; a `Changed`/`Revoked` mismatch rejects immediately (MITM);
/// a missing entry falls through to the next store and ends as `Unknown`.
fn verify_against_stores(
    stores: &[KnownHosts],
    host: &str,
    port: Option<u16>,
    kt: &KeyType,
    b64: &str,
) -> Verification {
    for store in stores {
        match store.check(host, port, kt, b64) {
            Verification::Trusted => return Verification::Trusted,
            other @ (Verification::Changed { .. } | Verification::Revoked) => return other,
            Verification::Unknown => {}
        }
    }
    Verification::Unknown
}

fn map_verification(host: &str, check: &HostKeyCheck) -> TransportError {
    match &check.verification {
        Verification::Trusted => TransportError::ConnectionFailed(format!(
            "internal error: host key for {host} unexpectedly trusted after rejection"
        )),
        Verification::Unknown => TransportError::HostKeyUnknown {
            host: host.to_string(),
            fingerprint: check.fingerprint.clone().unwrap_or_default(),
        },
        Verification::Changed { .. } => TransportError::HostKeyChanged {
            host: host.to_string(),
        },
        Verification::Revoked => {
            TransportError::HostKeyPolicyUnsupported(format!("host key for {host} is revoked"))
        }
    }
}

fn map_russh_error(e: russh::Error) -> TransportError {
    use russh::Error as E;
    match e {
        E::ConnectionTimeout | E::HUP | E::KeepaliveTimeout | E::InactivityTimeout => {
            TransportError::ConnectionFailed(e.to_string())
        }
        E::CouldNotReadKey | E::Keys(_) | E::SshKey(_) => {
            TransportError::LocalIo(format!("ssh key error: {e}"))
        }
        E::NoAuthMethod | E::PacketAuth | E::NotAuthenticated => {
            TransportError::AuthenticationFailed(e.to_string())
        }
        E::KeyChanged { .. } => TransportError::HostKeyChanged {
            host: "<unknown>".to_string(),
        },
        _ => TransportError::ConnectionFailed(e.to_string()),
    }
}

/// A fresh multi-thread runtime for the native transport. The worker threads
/// keep the russh session task — and its keepalive probe loop — running even
/// when no operation is in flight; a current-thread runtime would suspend the
/// whole connection between `block_on` calls, making keepalive and liveness
/// detection unreliable.
fn make_runtime() -> Result<tokio::runtime::Runtime, TransportError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| TransportError::LocalIo(format!("failed to start async runtime: {e}")))
}

/// Shell-quote a path for safe inclusion in a remote command. `shlex::try_quote`
/// rejects embedded NUL bytes; fall back to the raw string rather than failing
/// the whole operation (a NUL in a path is already invalid on POSIX).
fn shell_quote(s: &str) -> String {
    shlex::try_quote(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| s.to_string())
}

/// Raw bytes returned by an exec channel.
struct RawOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_status: i32,
}

/// Connect (with host-key verification + public-key auth) and return a handle.
async fn establish(
    cfg: &NativeTransportConfig,
) -> Result<client::Handle<NativeClientHandler>, TransportError> {
    if cfg.jump_host.is_some() {
        return Err(TransportError::UnsupportedOperation(
            "native backend: ProxyJump / jump-host routing is not implemented yet \
             (planned for a later step)"
                .into(),
        ));
    }
    let key_path = cfg.key_path.as_ref().ok_or_else(|| {
        TransportError::Configuration(
            "native backend requires VB_SSH_KEY: agent, password, and keyboard-interactive \
             auth are not yet supported"
                .into(),
        )
    })?;
    let key_pair = load_secret_key(key_path, None).map_err(|e| {
        TransportError::LocalIo(format!("could not load key {}: {e}", key_path.display()))
    })?;

    let verification = Arc::new(Mutex::new(None));
    let handler = NativeClientHandler {
        host: cfg.host.clone(),
        port: cfg.ssh_port,
        stores: cfg.known_hosts.clone(),
        verification: verification.clone(),
    };

    // Map the design's keepalive policy onto russh: probe every `interval`,
    // declare the connection dead after `max_failures` consecutive misses.
    // russh then surfaces it as `KeepaliveTimeout`, which classifies as a
    // transient failure and reconnects on the next operation.
    let client_cfg = Arc::new(client::Config {
        keepalive_interval: Some(cfg.keepalive.interval),
        keepalive_max: cfg.keepalive.max_failures as usize,
        ..Default::default()
    });
    let addrs = (cfg.host.clone(), cfg.ssh_port);
    let connect_fut = client::connect(client_cfg, addrs, handler);

    let mut session = match tokio::time::timeout(cfg.connect_timeout, connect_fut).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            if let Some(v) = verification.lock().unwrap().take() {
                if v.verification != Verification::Trusted {
                    return Err(map_verification(&cfg.host, &v));
                }
            }
            return Err(map_russh_error(e));
        }
        Err(_) => {
            return Err(TransportError::ConnectionFailed(format!(
                "connection to {}:{} timed out",
                cfg.host, cfg.ssh_port
            )))
        }
    };

    // Re-check the captured host-key outcome (defensive: connect must have
    // failed already if it was not Trusted, but surface a precise error anyway).
    if let Some(v) = verification.lock().unwrap().take() {
        if v.verification != Verification::Trusted {
            return Err(map_verification(&cfg.host, &v));
        }
    }

    let user = cfg.user.clone().unwrap_or_else(|| "root".to_string());
    let hash = session
        .best_supported_rsa_hash()
        .await
        .map_err(map_russh_error)?
        .flatten();
    let auth = session
        .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key_pair), hash))
        .await
        .map_err(map_russh_error)?;
    if !auth.success() {
        return Err(TransportError::AuthenticationFailed(format!(
            "public-key authentication to {} failed",
            cfg.host
        )));
    }
    Ok(session)
}

/// Establish a session with bounded reconnect backoff.
///
/// Only the *establishment* is retried — TCP connect, SSH handshake, and
/// public-key auth. No channel has been opened and no operation bytes have
/// been sent at that point, so a retry can never replay remote work: this is
/// the design's invariant that reconnection re-establishes the path without
/// re-issuing it. Errors that are permanent (host key, auth rejection,
/// configuration) or request-level are returned immediately.
async fn establish_with_retry(
    cfg: &NativeTransportConfig,
    deadline: Deadline,
) -> Result<client::Handle<NativeClientHandler>, TransportError> {
    let policy = &cfg.reconnect;
    let mut attempt: u32 = 1;
    loop {
        match establish(cfg).await {
            Ok(session) => return Ok(session),
            // Only a network-path failure is worth re-establishing for. A
            // request-level error cannot occur during establishment (nothing
            // was sent), but the match keeps the classification authoritative.
            Err(e) if FailureClass::of(&e) == FailureClass::Transient => {
                if !policy.may_retry(attempt) || deadline.is_expired() {
                    return Err(e);
                }
                // Spread retries with jitter, but never wait past the
                // caller's deadline: a bounded budget must not grow because
                // the network is flaky.
                let seed = establishment_seed();
                let wait = policy
                    .jittered_delay(attempt, seed)
                    .min(deadline.remaining());
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                if deadline.is_expired() {
                    return Err(e);
                }
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Jitter seed for reconnect backoff. Not cryptographic — it only needs to
/// decorrelate concurrent establishments; nanos plus the pid suffice.
fn establishment_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
}

/// Upload window for the streaming SFTP write/read paths.
const SFTP_CHUNK: usize = 64 * 1024;

/// Whether an SFTP attempt failed because the remote did not advertise the
/// sftp subsystem (vs a connection/auth failure that must not be retried
/// through the exec pipe). Callers use this to fall back to the exec `cat`
/// transfer that directories already rely on.
fn sftp_unavailable(e: &TransportError) -> bool {
    matches!(e, TransportError::UnsupportedOperation(_))
}

/// Stream `data` to an open SFTP `File` in [`SFTP_CHUNK`] windows and flush.
///
/// Extracted from `upload_via_sftp` so the in-process integration test can drive
/// the *exact* production write path (chunking + shutdown) against a real SFTP
/// server — `upload_via_sftp` only differs by how the `File` is opened.
async fn sftp_write_all(
    file: &mut russh_sftp::client::fs::File,
    data: &[u8],
) -> Result<(), TransportError> {
    for chunk in data.chunks(SFTP_CHUNK) {
        file.write_all(chunk)
            .await
            .map_err(|e| TransportError::TransferInterrupted {
                request: RequestId::new(),
                reason: format!("sftp write: {e}"),
            })?;
    }
    file.shutdown()
        .await
        .map_err(|e| TransportError::TransferInterrupted {
            request: RequestId::new(),
            reason: format!("sftp flush: {e}"),
        })?;
    Ok(())
}

/// Stream an open SFTP `File` into a local buffer in [`SFTP_CHUNK`] windows,
/// stopping at EOF. Mirrors the production read path; `download_via_sftp` is the
/// only caller besides the integration test.
async fn sftp_read_all(file: &mut russh_sftp::client::fs::File) -> Result<Vec<u8>, TransportError> {
    let mut buf = Vec::new();
    let mut window = vec![0u8; SFTP_CHUNK];
    loop {
        let n = file
            .read(&mut window)
            .await
            .map_err(|e| TransportError::TransferInterrupted {
                request: RequestId::new(),
                reason: format!("sftp read: {e}"),
            })?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&window[..n]);
    }
    Ok(buf)
}

/// Shared state for the native transport's single live SSH connection.
///
/// P1-2 keeps the connection alive across operations instead of the step-3
/// behaviour of establishing and disconnecting for every command. The russh
/// handle lives here in a `(generation, handle)` pair: opening a channel only
/// borrows the handle briefly (russh's `channel_open_session` takes `&self`),
/// so once a channel exists the lock is released and independent channels run
/// concurrently. `generation` is bumped on every (re)establishment so a late
/// connection-level failure from a stale request can never evict a
/// replacement built concurrently — eviction matches the generation it
/// started with. Connection-level failures still clear the slot so the next
/// operation re-establishes; the endpoint pool's generation guard decides
/// whether the whole endpoint is replaced.
struct SessionState {
    inner: AsyncMutex<Option<(u64, client::Handle<NativeClientHandler>)>>,
    runtime: tokio::runtime::Runtime,
    /// Monotonic connection generation: incremented on every establishment.
    /// Stale requests compare against it before clearing the slot.
    epoch: AtomicU64,
    /// Number of SSH connections ever established by this transport. Exposed
    /// for tests and acceptance: "handshakes", not factory calls.
    connections: AtomicU64,
}

impl std::fmt::Debug for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionState")
            .field("connections", &self.connections)
            .field("runtime", &self.runtime)
            .finish()
    }
}

impl SessionState {
    fn new() -> Result<Self, TransportError> {
        Ok(Self {
            inner: AsyncMutex::new(None),
            runtime: make_runtime()?,
            epoch: AtomicU64::new(0),
            connections: AtomicU64::new(0),
        })
    }
}

/// The native transport: a resolved endpoint plus a persistent SSH connection
/// (established lazily, reused across operations until a connection-level
/// failure clears it).
#[derive(Clone, Debug)]
pub struct NativeTransport {
    config: NativeTransportConfig,
    session: Arc<SessionState>,
}

impl NativeTransport {
    /// Build from the shared `Config`. Fails loudly (never falls back) when the
    /// native backend cannot satisfy the request: missing `VB_REMOTE_HOST`, or
    /// no `VB_SSH_KEY` (step 3 is public-key only).
    #[allow(dead_code)] // wired on Unix by open_transport_for_daemon and the pooled daemon; non-Unix builds have neither
    pub fn from_config(config: &Config) -> Result<Self, TransportError> {
        let host = config.remote_host.clone().ok_or_else(|| {
            TransportError::Configuration("native backend requires VB_REMOTE_HOST".into())
        })?;
        let ssh_port = config.ssh_port.unwrap_or(22);
        // Step 3 supports public-key auth only; a missing key is a configuration
        // error, never a silent fallback to OpenSSH.
        let key_path = config.ssh_key.as_ref().map(PathBuf::from).ok_or_else(|| {
            TransportError::Configuration(
                "native backend requires VB_SSH_KEY: agent, password, and keyboard-interactive \
                 auth are not yet supported"
                    .into(),
            )
        })?;

        // A missing or unreadable known_hosts is not fatal: the caller learns
        // about an unknown host on first connection (HostKeyUnknown), and a
        // permission error on the store must never block transport construction.
        let mut stores = Vec::new();
        if let Some(home) = dirs::home_dir() {
            stores.push(load_known_hosts_or_empty(
                &home.join(".ssh").join("known_hosts"),
            ));
        }
        stores.push(load_known_hosts_or_empty(Path::new(
            "/etc/ssh/ssh_known_hosts",
        )));

        // Lifecycle policies come from the same env surface; zero values are
        // rejected here so a bad env fails loudly at construction, not on the
        // first operation.
        let keepalive = KeepalivePolicy::from_config(config)?;
        let reconnect = ReconnectPolicy::from_config(config)?;

        let session = Arc::new(SessionState::new()?);
        Ok(NativeTransport {
            config: NativeTransportConfig {
                host,
                user: config.remote_user.clone(),
                ssh_port,
                jump_host: config.jump_host.clone(),
                key_path: Some(key_path),
                known_hosts: stores,
                connect_timeout: Duration::from_secs(config.timeout.max(5)),
                keepalive,
                reconnect,
            },
            session,
        })
    }

    /// Open a session channel on the live SSH connection and run `f` on it.
    ///
    /// Locking, establishment and channel-open all share the request's
    /// deadline: waiting for the handle (a concurrent command's short
    /// critical section), connecting + authenticating (with reconnect
    /// retries) and the server confirming the channel cannot overrun the
    /// budget. The handle lock is released as soon as the channel exists —
    /// russh channels are independent once opened — so concurrent commands
    /// run on separate channels instead of serialising on the handle.
    /// Connection-level failures from `f` clear the slot, but only when it
    /// still holds the same generation this call started with, so a late
    /// failure from a stale request never evicts a replacement connection.
    fn with_channel<T>(
        &self,
        deadline: Deadline,
        req_id: &RequestId,
        f: impl FnOnce(
            russh::Channel<russh::client::Msg>,
            &tokio::runtime::Runtime,
            Duration,
        ) -> Result<T, TransportError>,
    ) -> Result<T, TransportError> {
        if deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req_id.clone(),
                after_secs: 0,
            });
        }
        let rt = &self.session.runtime;
        let (epoch, channel) = {
            let remaining = deadline.remaining();
            rt.block_on(async move {
                tokio::time::timeout(remaining, async move {
                    // Lock wait, establishment and channel-open all consume
                    // the request's remaining budget (P1-2 / P1-3).
                    let mut guard = self.session.inner.lock().await;
                    if guard.as_ref().is_none_or(|(_, h)| h.is_closed()) {
                        let established = establish_with_retry(&self.config, deadline).await?;
                        self.session.connections.fetch_add(1, Ordering::Relaxed);
                        let epoch = self.session.epoch.fetch_add(1, Ordering::Relaxed) + 1;
                        *guard = Some((epoch, established));
                    }
                    let (epoch, handle) = guard.as_ref().expect("session");
                    let channel = handle
                        .channel_open_session()
                        .await
                        .map_err(map_russh_error)?;
                    Ok::<(u64, russh::Channel<russh::client::Msg>), TransportError>((
                        *epoch, channel,
                    ))
                })
                .await
                .map_err(|_| TransportError::ExecutionTimeout {
                    request: req_id.clone(),
                    after_secs: remaining.as_secs().max(1),
                    remote_terminated: false,
                })?
            })?
        };
        // The handle lock was released above: the channel is independent, so
        // another command can open its own channel and run concurrently.
        let result = f(channel, rt, deadline.remaining());
        if matches!(&result, Err(e) if FailureClass::of(e) == FailureClass::Transient) {
            // Generation-matched eviction: never drop a replacement built by a
            // concurrent request after our connection died. Bounded by the
            // request's remaining deadline — a concurrent re-establishment
            // holding the handle lock must never delay this request's error
            // return. If the lock is not reachable in time the stale slot is
            // left in place; the next operation re-detects it via is_closed()
            // / lazy establishment, so failure marking stays effective.
            let remaining = deadline.remaining();
            let _ = rt.block_on(async move {
                tokio::time::timeout(remaining, async move {
                    let mut guard = self.session.inner.lock().await;
                    if guard.as_ref().map(|(e, _)| *e) == Some(epoch) {
                        *guard = None;
                    }
                })
                .await
            });
        }
        result
    }

    /// Run `command` on the persistent SSH connection, reusing the live
    /// session when present. The channel is closed after the command; the
    /// connection itself is kept for the next operation.
    fn exec_command(
        &self,
        command: &str,
        stdin: Option<Vec<u8>>,
        deadline: Deadline,
        req_id: &RequestId,
    ) -> Result<RawOutput, TransportError> {
        let cmd = command.to_owned();
        self.with_channel(deadline, req_id, move |channel, rt, remaining| {
            rt.block_on(async move {
                tokio::time::timeout(remaining, async move {
                    let mut channel = channel;
                    channel.exec(false, cmd).await.map_err(map_russh_error)?;
                    if let Some(data) = stdin {
                        channel.data_bytes(data).await.map_err(map_russh_error)?;
                        channel.eof().await.map_err(map_russh_error)?;
                    }
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    let mut exit_status: i32 = -1;
                    while let Some(msg) = channel.wait().await {
                        match msg {
                            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                            ChannelMsg::ExtendedData { data, ext: 1 } => {
                                stderr.extend_from_slice(&data)
                            }
                            ChannelMsg::ExtendedData { .. } => {}
                            ChannelMsg::ExitStatus { exit_status: c } => exit_status = c as i32,
                            ChannelMsg::ExitSignal { .. } => exit_status = -1,
                            _ => {}
                        }
                    }
                    Ok(RawOutput {
                        stdout,
                        stderr,
                        exit_status,
                    })
                })
                .await
                .map_err(|_| TransportError::ExecutionTimeout {
                    request: req_id.clone(),
                    after_secs: remaining.as_secs().max(1),
                    remote_terminated: false,
                })?
            })
        })
    }

    /// Open the SFTP subsystem on the persistent connection. The SSH
    /// connection is reused; only a fresh SFTP channel is opened per call.
    fn open_sftp(
        &self,
        deadline: Deadline,
        req_id: &RequestId,
    ) -> Result<SftpSession, TransportError> {
        self.with_channel(deadline, req_id, move |channel, rt, remaining| {
            rt.block_on(async move {
                tokio::time::timeout(remaining, async move {
                    channel.request_subsystem(true, "sftp").await.map_err(|e| {
                        TransportError::UnsupportedOperation(format!(
                            "sftp subsystem unavailable: {e}"
                        ))
                    })?;
                    let stream = channel.into_stream();
                    let sftp = SftpSession::new(stream).await.map_err(|e| {
                        TransportError::UnsupportedOperation(format!(
                            "sftp session init failed: {e}"
                        ))
                    })?;
                    Ok(sftp)
                })
                .await
                .map_err(|_| TransportError::ExecutionTimeout {
                    request: req_id.clone(),
                    after_secs: remaining.as_secs().max(1),
                    remote_terminated: false,
                })?
            })
        })
    }

    /// Upload `data` to `remote` over the SFTP subsystem on the persistent
    /// connection, streaming in 64 KiB windows. A missing sftp subsystem
    /// surfaces as `UnsupportedOperation` so the caller can fall back to the
    /// exec `cat` pipe.
    fn upload_via_sftp(
        &self,
        remote: &Path,
        data: Vec<u8>,
        deadline: Deadline,
        req_id: &RequestId,
    ) -> Result<(), TransportError> {
        if deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req_id.clone(),
                after_secs: 0,
            });
        }
        let sftp = self.open_sftp(deadline, req_id)?;
        let remaining = deadline.remaining();
        let remote_str = remote.to_string_lossy().into_owned();
        // Atomic upload: write to a staging file first, then rename into place.
        // A failed upload leaves only the staging file (never a half-written
        // target). Per-request UUID so concurrent uploads to the same target
        // never share a staging file.
        let staging = format!("{remote_str}.tmp.{}", uuid::Uuid::new_v4());
        let staging_name = staging.clone();
        let rt = &self.session.runtime;
        let staged: Result<(), TransportError> = rt.block_on(async move {
            tokio::time::timeout(remaining, async move {
                let mut file = sftp.create(&staging_name).await.map_err(|e| {
                    TransportError::TransferInterrupted {
                        request: req_id.clone(),
                        reason: format!("sftp create {staging_name}: {e}"),
                    }
                })?;
                sftp_write_all(&mut file, &data).await?;
                drop(file);
                Ok(())
            })
            .await
            .map_err(|_| TransportError::ExecutionTimeout {
                request: req_id.clone(),
                after_secs: remaining.as_secs().max(1),
                remote_terminated: false,
            })?
        });
        staged?;
        // Atomic publish via exec `mv` — SFTP rename is not universally
        // supported, and `mv` is atomic on the same filesystem.
        let mv_cmd = format!(
            "mv -f {src} {dst} && rm -f {src}",
            src = shell_quote(&staging),
            dst = shell_quote(&remote_str)
        );
        let mv_result = self.exec_command(&mv_cmd, None, deadline, req_id);
        match mv_result {
            Ok(r) if r.exit_status == 0 => Ok(()),
            Ok(r) => {
                let _ = self.exec_command(
                    &format!("rm -f {}", shell_quote(&staging)),
                    None,
                    deadline,
                    req_id,
                );
                Err(TransportError::TransferInterrupted {
                    request: req_id.clone(),
                    reason: format!(
                        "atomic rename failed: {}",
                        String::from_utf8_lossy(&r.stderr)
                    ),
                })
            }
            Err(e) => {
                let _ = self.exec_command(
                    &format!("rm -f {}", shell_quote(&staging)),
                    None,
                    deadline,
                    req_id,
                );
                Err(e)
            }
        }
    }

    /// Download `remote` over the SFTP subsystem on the persistent connection,
    /// streaming in 64 KiB windows into a local buffer. A missing sftp
    /// subsystem surfaces as `UnsupportedOperation`.
    fn download_via_sftp(
        &self,
        remote: &Path,
        deadline: Deadline,
        req_id: &RequestId,
    ) -> Result<Vec<u8>, TransportError> {
        if deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req_id.clone(),
                after_secs: 0,
            });
        }
        let sftp = self.open_sftp(deadline, req_id)?;
        let remaining = deadline.remaining();
        let remote_str = remote.to_string_lossy().into_owned();
        let rt = &self.session.runtime;
        let fetched: Result<Vec<u8>, TransportError> = rt.block_on(async move {
            tokio::time::timeout(remaining, async move {
                let mut file = sftp.open(&remote_str).await.map_err(|e| {
                    TransportError::TransferInterrupted {
                        request: req_id.clone(),
                        reason: format!("sftp open {remote_str}: {e}"),
                    }
                })?;
                sftp_read_all(&mut file).await
            })
            .await
            .map_err(|_| TransportError::ExecutionTimeout {
                request: req_id.clone(),
                after_secs: remaining.as_secs().max(1),
                remote_terminated: false,
            })?
        });
        fetched
    }
}

impl Drop for NativeTransport {
    fn drop(&mut self) {
        // Only the last clone of a transport tears the shared connection down;
        // an earlier clone must be able to keep using it.
        if Arc::strong_count(&self.session) > 1 {
            return;
        }
        if let Some((_, session)) = self.session.inner.blocking_lock().take() {
            let rt = &self.session.runtime;
            rt.block_on(async move {
                let _ = session
                    .disconnect(russh::Disconnect::ByApplication, "", "English")
                    .await;
            });
        }
    }
}

impl RemoteTransport for NativeTransport {
    fn test_connection(&self, deadline: Deadline) -> Result<bool, TransportError> {
        if deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: RequestId::new(),
                after_secs: 0,
            });
        }
        let req_id = RequestId::new();
        let rt = &self.session.runtime;
        // test_connection is an explicit liveness probe. If a session already
        // exists it is probed in place (an SSH ping/pong) and, when the remote
        // has gone away, reported unhealthy — never silently re-established,
        // so callers can observe the disconnect. Only when no session exists
        // yet does the probe establish one, as a reachability check. Host-key
        // / auth failures are real errors, not "unreachable".
        let (epoch, probe): (u64, Result<(), TransportError>) = {
            let remaining = deadline.remaining();
            rt.block_on(async move {
                tokio::time::timeout(remaining, async move {
                    let mut guard = self.session.inner.lock().await;
                    match guard.as_ref() {
                        // No session yet: this is the reachability case, so
                        // establish (lazily) and ping the fresh handle.
                        None => {
                            let established = establish_with_retry(&self.config, deadline).await?;
                            self.session.connections.fetch_add(1, Ordering::Relaxed);
                            let epoch = self.session.epoch.fetch_add(1, Ordering::Relaxed) + 1;
                            *guard = Some((epoch, established));
                        }
                        // Session ended: evict it and report unavailable. Do
                        // not reconnect — the caller must observe the break.
                        Some((_, handle)) if handle.is_closed() => {
                            *guard = None;
                            return Ok::<(u64, Result<(), TransportError>), TransportError>((
                                0,
                                Err(TransportError::ConnectionFailed(
                                    "session closed since last use".into(),
                                )),
                            ));
                        }
                        Some(_) => {}
                    }
                    let (epoch, handle) = guard.as_ref().expect("session");
                    let ping = handle.send_ping().await.map_err(map_russh_error);
                    let probe = match ping {
                        Ok(()) => {
                            // russh's send_ping resolves its reply channel even
                            // when the session is tearing down; only a session
                            // that is still open proves the remote answered.
                            if handle.is_closed() {
                                Err(TransportError::ConnectionFailed(
                                    "session closed during liveness probe".into(),
                                ))
                            } else {
                                Ok(())
                            }
                        }
                        Err(e) => Err(e),
                    };
                    Ok::<(u64, Result<(), TransportError>), TransportError>((*epoch, probe))
                })
                .await
                .map_err(|_| TransportError::ExecutionTimeout {
                    request: req_id.clone(),
                    after_secs: remaining.as_secs().max(1),
                    remote_terminated: false,
                })?
            })?
        };
        match probe {
            Ok(()) => Ok(true),
            Err(e) => {
                if FailureClass::of(&e) == FailureClass::Transient {
                    // Generation-matched eviction: a stale request's failure
                    // must not drop a replacement connection. Bounded by the
                    // request's remaining deadline for the same reason as the
                    // with_channel cleanup: never block the probe's error
                    // return behind a concurrent re-establishment.
                    let remaining = deadline.remaining();
                    let _ = rt.block_on(async move {
                        tokio::time::timeout(remaining, async move {
                            let mut guard = self.session.inner.lock().await;
                            if guard.as_ref().map(|(g, _)| *g) == Some(epoch) {
                                *guard = None;
                            }
                        })
                        .await
                    });
                }
                match e {
                    TransportError::ConnectionFailed(_)
                    | TransportError::HostKeyUnknown { .. }
                    | TransportError::HostKeyChanged { .. }
                    | TransportError::HostKeyPolicyUnsupported(_)
                    | TransportError::AuthenticationFailed(_) => Err(e),
                    _ => Ok(false),
                }
            }
        }
    }

    fn run_command(&self, req: &CommandRequest) -> Result<CommandResult, TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        let raw = self.exec_command(&req.command, None, req.deadline, &req.id)?;
        Ok(CommandResult {
            exit_status: raw.exit_status,
            stdout: String::from_utf8_lossy(&raw.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&raw.stderr).into_owned(),
            success: raw.exit_status == 0,
            duration: Duration::ZERO,
        })
    }

    fn upload_file(&self, req: &UploadFileRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        let bytes = std::fs::read(&req.local)
            .map_err(|e| TransportError::LocalIo(format!("read {}: {e}", req.local.display())))?;
        // Single files stream over the SFTP subsystem (design step 4). Where
        // the remote does not advertise sftp, fall back to the exec `cat` pipe
        // that directories already rely on. Clone for the SFTP attempt; the
        // exec fallback still needs the original bytes.
        match self.upload_via_sftp(Path::new(&req.remote), bytes.clone(), req.deadline, &req.id) {
            Ok(()) => Ok(()),
            Err(e) if sftp_unavailable(&e) => {
                // Atomic upload via exec: write to staging file, then mv.
                let staging = format!("{}.tmp.{}", req.remote, uuid::Uuid::new_v4());
                let cmd = format!("cat > {}", shell_quote(&staging));
                let raw = self.exec_command(&cmd, Some(bytes), req.deadline, &req.id)?;
                if raw.exit_status != 0 {
                    let _ = self.exec_command(
                        &format!("rm -f {}", shell_quote(&staging)),
                        None,
                        req.deadline,
                        &req.id,
                    );
                    return Err(TransportError::TransferInterrupted {
                        request: req.id.clone(),
                        reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
                    });
                }
                // Atomic publish.
                let mv_cmd = format!(
                    "mv -f {src} {dst} && rm -f {src}",
                    src = shell_quote(&staging),
                    dst = shell_quote(&req.remote)
                );
                let mv_raw = self.exec_command(&mv_cmd, None, req.deadline, &req.id)?;
                if mv_raw.exit_status != 0 {
                    let _ = self.exec_command(
                        &format!("rm -f {}", shell_quote(&staging)),
                        None,
                        req.deadline,
                        &req.id,
                    );
                    return Err(TransportError::TransferInterrupted {
                        request: req.id.clone(),
                        reason: format!(
                            "atomic rename failed: {}",
                            String::from_utf8_lossy(&mv_raw.stderr)
                        ),
                    });
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn upload_text(&self, req: &UploadTextRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        let bytes = req.text.clone().into_bytes();
        // Atomic upload: write to staging, then mv into place.
        let staging = format!("{}.tmp.{}", req.remote, uuid::Uuid::new_v4());
        let cmd = format!("cat > {}", shell_quote(&staging));
        let raw = self.exec_command(&cmd, Some(bytes), req.deadline, &req.id)?;
        if raw.exit_status != 0 {
            let _ = self.exec_command(
                &format!("rm -f {}", shell_quote(&staging)),
                None,
                req.deadline,
                &req.id,
            );
            return Err(TransportError::TransferInterrupted {
                request: req.id.clone(),
                reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
            });
        }
        let mv_cmd = format!(
            "mv -f {src} {dst} && rm -f {src}",
            src = shell_quote(&staging),
            dst = shell_quote(&req.remote)
        );
        let mv_raw = self.exec_command(&mv_cmd, None, req.deadline, &req.id)?;
        if mv_raw.exit_status != 0 {
            let _ = self.exec_command(
                &format!("rm -f {}", shell_quote(&staging)),
                None,
                req.deadline,
                &req.id,
            );
            return Err(TransportError::TransferInterrupted {
                request: req.id.clone(),
                reason: format!(
                    "atomic rename failed: {}",
                    String::from_utf8_lossy(&mv_raw.stderr)
                ),
            });
        }
        Ok(())
    }

    fn download_file(&self, req: &DownloadFileRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        // Atomic download: write to a local staging file, then rename.
        // A failed download never leaves a half-written target file.
        let local_staging = format!("{}.tmp.{}", req.local.display(), uuid::Uuid::new_v4());
        let local_staging_path = std::path::PathBuf::from(&local_staging);

        let write_result =
            match self.download_via_sftp(Path::new(&req.remote), req.deadline, &req.id) {
                Ok(bytes) => {
                    std::fs::write(&local_staging_path, &bytes).map_err(|e| {
                        TransportError::LocalIo(format!("write {}: {e}", local_staging))
                    })?;
                    Ok(())
                }
                Err(e) if sftp_unavailable(&e) => {
                    let cmd = format!("cat {}", shell_quote(&req.remote));
                    let raw = self.exec_command(&cmd, None, req.deadline, &req.id)?;
                    if raw.exit_status != 0 {
                        let _ = std::fs::remove_file(&local_staging_path);
                        return Err(TransportError::TransferInterrupted {
                            request: req.id.clone(),
                            reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
                        });
                    }
                    std::fs::write(&local_staging_path, &raw.stdout).map_err(|e| {
                        TransportError::LocalIo(format!("write {}: {e}", local_staging))
                    })?;
                    Ok(())
                }
                Err(e) => Err(e),
            };

        match write_result {
            Ok(()) => {
                // Atomic publish: rename staging to target.
                std::fs::rename(&local_staging_path, &req.local).map_err(|e| {
                    let _ = std::fs::remove_file(&local_staging_path);
                    TransportError::LocalIo(format!(
                        "atomic rename {} -> {}: {e}",
                        local_staging,
                        req.local.display()
                    ))
                })?;
                Ok(())
            }
            Err(e) => {
                let _ = std::fs::remove_file(&local_staging_path);
                Err(e)
            }
        }
    }

    fn download_dir(&self, req: &DownloadDirRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        let cmd = format!("tar -cf - -C {} .", shell_quote(&req.remote));
        let raw = self.exec_command(&cmd, None, req.deadline, &req.id)?;
        if raw.exit_status != 0 {
            return Err(TransportError::TransferInterrupted {
                request: req.id.clone(),
                reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
            });
        }
        std::fs::create_dir_all(&req.local)
            .map_err(|e| TransportError::LocalIo(format!("create {}: {e}", req.local.display())))?;
        // Untar locally. The remote side already produced the tar stream, so
        // the local `tar` requirement mirrors the OpenSSH backend's remote
        // `tar` requirement (no extra remote toolchain divergence).
        let tmp = req
            .local
            .join(format!(".vcli-dl-{}.tar", std::process::id()));
        std::fs::write(&tmp, &raw.stdout)
            .map_err(|e| TransportError::LocalIo(format!("stage tar: {e}")))?;
        let status = SyncCommand::new("tar")
            .arg("-xf")
            .arg(&tmp)
            .arg("-C")
            .arg(&req.local)
            .status();
        let _ = std::fs::remove_file(&tmp);
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(TransportError::TransferInterrupted {
                request: req.id.clone(),
                reason: format!("local tar exited with {s}"),
            }),
            Err(e) => Err(TransportError::LocalIo(format!(
                "failed to run local tar (required for download_dir): {e}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::contract::test_support::shared_contract_suite;
    use russh::server::{self, Auth, Session};
    use std::io;
    use std::pin::Pin;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::oneshot;

    fn cfg_with(host: &str, key: Option<&str>, jump: Option<&str>) -> Config {
        Config {
            profile: None,
            remote_host: Some(host.into()),
            remote_user: None,
            port: 65432,
            port_explicit: true,
            jump_host: jump.map(String::from),
            jump_user: None,
            ssh_port: Some(22),
            ssh_key: key.map(String::from),
            ssh_config: None,
            ssh_backend: Some("native".into()),
            disable_control_master: false,
            timeout: 30,
            read_timeout: 120,
            keep_remote_files: false,
            spectre_cmd: "spectre".into(),
            spectre_args: vec![],
            spectre_max_workers: 8,
            ssh_max_sessions: 10,
            ssh_max_bulk_sessions: 2,
            ssh_reconnect_max_attempts: 8,
            ssh_reconnect_max_delay: 30,
            ssh_keepalive_interval: 30,
            ssh_keepalive_failures: 3,
            transport_shutdown_grace: 10,
            cadence_cshrc: None,
            spectre_bin: None,
            roles: Default::default(),
            transport_daemon_socket: None,
            transport_daemon_token: None,
        }
    }

    #[test]
    fn transport_reuses_the_session_slot_between_operations() {
        // P1-2 acceptance at the unit level: one transport owns one live
        // session slot plus a handshake counter, and every clone shares it
        // (the pool clones transports). Actual handshake reuse is verified
        // end-to-end by the native-pool probe against a real host — this
        // locks the structure that makes that reuse possible.
        let t = NativeTransport::from_config(&cfg_with("h", Some("/tmp/k"), None))
            .expect("from_config builds without a jump host");
        assert_eq!(
            t.session.connections.load(Ordering::Relaxed),
            0,
            "no SSH connection has been established yet"
        );
        let t2 = t.clone();
        assert!(
            Arc::ptr_eq(&t.session, &t2.session),
            "clones must share one SessionState (one live connection)"
        );
        assert_eq!(t2.session.connections.load(Ordering::Relaxed), 0);
        // The slot starts empty: the first operation establishes lazily.
        assert!(
            t.session.inner.blocking_lock().is_none(),
            "connection must be lazy (nothing established at construction)"
        );
    }

    #[test]
    fn from_config_requires_remote_host() {
        let mut c = cfg_with("h", Some("/tmp/k"), None);
        c.remote_host = None;
        assert!(matches!(
            NativeTransport::from_config(&c),
            Err(TransportError::Configuration(_))
        ));
    }

    #[test]
    fn from_config_requires_key_for_step3() {
        // Step 3 supports public-key auth only; no key is a configuration error,
        // not a silent fallback to OpenSSH.
        assert!(matches!(
            NativeTransport::from_config(&cfg_with("h", None, None)),
            Err(TransportError::Configuration(_))
        ));
    }

    #[test]
    fn sftp_unavailable_routes_only_to_the_exec_fallback() {
        // The exec fallback for single-file transfer must trigger only when the
        // remote lacks the sftp subsystem, never on a connection/auth failure.
        assert!(sftp_unavailable(&TransportError::UnsupportedOperation(
            "sftp subsystem unavailable".into()
        )));
        assert!(!sftp_unavailable(&TransportError::ConnectionFailed(
            "down".into()
        )));
        assert!(!sftp_unavailable(&TransportError::AuthenticationFailed(
            "no".into()
        )));
        assert!(!sftp_unavailable(&TransportError::TransferInterrupted {
            request: RequestId::new(),
            reason: "boom".into()
        }));
    }

    #[test]
    fn from_config_builds_and_passes_contract_suite() {
        // The shared suite only exercises an expired deadline + health(), so a
        // dummy (non-connectable) endpoint is sufficient — no network needed.
        let t = NativeTransport::from_config(&cfg_with(
            "compute-eda-42",
            Some("/tmp/id_ed25519"),
            None,
        ))
        .unwrap();
        shared_contract_suite(&t);
    }

    #[test]
    fn is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NativeTransport>();
    }

    #[test]
    fn from_config_wires_lifecycle_policies() {
        let mut c = cfg_with("h", Some("/tmp/k"), None);
        c.ssh_keepalive_interval = 15;
        c.ssh_keepalive_failures = 5;
        c.ssh_reconnect_max_attempts = 3;
        c.ssh_reconnect_max_delay = 9;
        let t = NativeTransport::from_config(&c).unwrap();
        assert_eq!(t.config.keepalive.interval, Duration::from_secs(15));
        assert_eq!(t.config.keepalive.max_failures, 5);
        assert_eq!(t.config.reconnect.max_attempts, 3);
        assert_eq!(t.config.reconnect.max_delay, Duration::from_secs(9));
    }

    #[test]
    fn from_config_rejects_a_zero_keepalive_interval() {
        let mut c = cfg_with("h", Some("/tmp/k"), None);
        c.ssh_keepalive_interval = 0;
        assert!(matches!(
            NativeTransport::from_config(&c),
            Err(TransportError::Configuration(_))
        ));
    }

    #[test]
    fn from_config_rejects_zero_reconnect_attempts() {
        let mut c = cfg_with("h", Some("/tmp/k"), None);
        c.ssh_reconnect_max_attempts = 0;
        assert!(matches!(
            NativeTransport::from_config(&c),
            Err(TransportError::Configuration(_))
        ));
    }

    // --- step 7b: real SFTP protocol roundtrip against an in-process server ---
    //
    // This is the end-to-end verification step 4's single-file SFTP was waiting
    // for. We do NOT need a real sshd: russh-sftp ships both client and server,
    // so we connect a `SftpSession` to a minimal in-memory `Handler` over a
    // `tokio::io::duplex` pair. `sftp_write_all` / `sftp_read_all` are the exact
    // helpers `upload_via_sftp` / `download_via_sftp` use, so this exercises the
    // production chunked streaming (and the >64 KiB multi-chunk path) against a
    // genuine SFTP implementation. Runs on every platform via the step 7 matrix.

    use russh_sftp::protocol::{
        Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
    };
    use russh_sftp::server::Handler;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Minimal in-memory SFTP server: file contents live in a `HashMap`, open
    /// handles reference a filename. Only the operations the russh-sftp client
    /// actually issues (init/open/write/read/close, plus fstat/stat/realpath for
    /// completeness) are implemented.
    #[derive(Default)]
    struct MemFs {
        files: Mutex<HashMap<String, Vec<u8>>>,
        handles: Mutex<HashMap<String, String>>,
        next_handle: Mutex<u32>,
    }

    impl Handler for MemFs {
        type Error = StatusCode;

        fn unimplemented(&self) -> Self::Error {
            StatusCode::OpUnsupported
        }

        async fn init(
            &mut self,
            _version: u32,
            _extensions: HashMap<String, String>,
        ) -> Result<Version, Self::Error> {
            Ok(Version::new())
        }

        async fn open(
            &mut self,
            id: u32,
            filename: String,
            pflags: OpenFlags,
            _attrs: FileAttributes,
        ) -> Result<Handle, Self::Error> {
            if pflags.contains(OpenFlags::WRITE) {
                self.files
                    .lock()
                    .unwrap()
                    .insert(filename.clone(), Vec::new());
            } else if !self.files.lock().unwrap().contains_key(&filename) {
                return Err(StatusCode::NoSuchFile);
            }
            let mut n = self.next_handle.lock().unwrap();
            *n += 1;
            let hid = format!("h{id}-{}", *n);
            self.handles.lock().unwrap().insert(hid.clone(), filename);
            Ok(Handle { id, handle: hid })
        }

        async fn write(
            &mut self,
            id: u32,
            handle: String,
            offset: u64,
            data: Vec<u8>,
        ) -> Result<Status, Self::Error> {
            let filename = self
                .handles
                .lock()
                .unwrap()
                .get(&handle)
                .ok_or(StatusCode::Failure)?
                .clone();
            let mut files = self.files.lock().unwrap();
            let contents = files.get_mut(&filename).ok_or(StatusCode::Failure)?;
            let off = offset as usize;
            if contents.len() < off + data.len() {
                contents.resize(off + data.len(), 0);
            }
            contents[off..off + data.len()].copy_from_slice(&data);
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: "Ok".into(),
                language_tag: "en-US".into(),
            })
        }

        async fn read(
            &mut self,
            id: u32,
            handle: String,
            offset: u64,
            len: u32,
        ) -> Result<Data, Self::Error> {
            let filename = self
                .handles
                .lock()
                .unwrap()
                .get(&handle)
                .ok_or(StatusCode::Failure)?
                .clone();
            let files = self.files.lock().unwrap();
            let contents = files.get(&filename).ok_or(StatusCode::Failure)?;
            let off = offset as usize;
            if off >= contents.len() {
                return Err(StatusCode::Eof);
            }
            let end = (off + len as usize).min(contents.len());
            Ok(Data {
                id,
                data: contents[off..end].to_vec(),
            })
        }

        async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
            self.handles.lock().unwrap().remove(&handle);
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: "Ok".into(),
                language_tag: "en-US".into(),
            })
        }

        async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
            let filename = self
                .handles
                .lock()
                .unwrap()
                .get(&handle)
                .ok_or(StatusCode::Failure)?
                .clone();
            let len = self
                .files
                .lock()
                .unwrap()
                .get(&filename)
                .map(|c| c.len() as u64)
                .unwrap_or(0);
            let attrs = FileAttributes {
                size: Some(len),
                ..Default::default()
            };
            Ok(Attrs { id, attrs })
        }

        async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
            let len = self
                .files
                .lock()
                .unwrap()
                .get(&path)
                .map(|c| c.len() as u64)
                .unwrap_or(0);
            let attrs = FileAttributes {
                size: Some(len),
                ..Default::default()
            };
            Ok(Attrs { id, attrs })
        }

        async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
            Ok(Name {
                id,
                files: vec![File::dummy(path)],
            })
        }
    }

    #[test]
    fn sftp_roundtrip_against_in_process_server() {
        // tokio's `macros` feature is not enabled, so drive the async test on the
        // module's own current-thread runtime (same one production uses).
        let rt = make_runtime().expect("runtime");
        rt.block_on(async {
            let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
            tokio::spawn(russh_sftp::server::run(server_stream, MemFs::default()));
            // Let the server task reach its read loop before the client speaks.
            tokio::task::yield_now().await;

            let sftp = SftpSession::new(client_stream)
                .await
                .expect("sftp session init");

            // Larger than SFTP_CHUNK (64 KiB) so the chunked streaming path is
            // actually exercised (multiple writes + multiple reads).
            let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
            let remote = "roundtrip.bin".to_string();

            // upload — mirrors `upload_via_sftp`'s path through the shared helper
            {
                let mut file = sftp.create(&remote).await.expect("sftp create");
                super::sftp_write_all(&mut file, &payload)
                    .await
                    .expect("sftp upload stream");
                // dropping `file` sends the CLOSE to the server
            }

            // download — mirrors `download_via_sftp`'s path
            let got = {
                let mut file = sftp.open(&remote).await.expect("sftp open");
                super::sftp_read_all(&mut file)
                    .await
                    .expect("sftp download stream")
                // drop sends CLOSE
            };

            assert_eq!(got, payload, "SFTP roundtrip must preserve every byte");
        });
    }

    #[test]
    fn establish_rejects_jump_host_with_unsupported_operation() {
        // Step 5 (ProxyJump / jump-host routing) is deferred. The native backend
        // must fail closed with a clear UnsupportedOperation at connect time
        // rather than silently attempting a single-hop connection. The guard
        // returns before any russh connect, so no network is touched.
        let rt = make_runtime().expect("runtime");
        // The transport owns a tokio runtime (SessionState); construct it
        // outside the test's block_on so dropping it happens on a plain
        // thread, not inside an async context (tokio rejects that).
        let t = NativeTransport::from_config(&cfg_with("h", Some("/tmp/k"), Some("jump")))
            .expect("from_config accepts a jump host (deferred, not rejected at construction)");
        rt.block_on(async {
            let err = establish(&t.config).await;
            assert!(
                matches!(err, Err(TransportError::UnsupportedOperation(_))),
                "jump host must surface as UnsupportedOperation"
            );
        });
    }

    #[test]
    fn native_transport_does_not_implement_local_forward() {
        // Step 6's RAMIC / X11 direct-tcpip forward is not implemented. The
        // RemoteTransport trait default reports the gap as UnsupportedOperation
        // so callers detect it structurally instead of panicking. This locks the
        // documented scope boundary (design doc Status: step 6 ⚠️ Partial).
        let t = NativeTransport::from_config(&cfg_with("h", Some("/tmp/k"), None))
            .expect("from_config builds without a jump host");
        let req = crate::transport::contract::ForwardRequest {
            id: crate::transport::contract::RequestId::new(),
            listen: "127.0.0.1:0".into(),
            remote_host: "remote".into(),
            remote_port: 80,
        };
        assert!(
            matches!(
                t.start_local_forward(&req),
                Err(TransportError::UnsupportedOperation(_))
            ),
            "start_local_forward must be UnsupportedOperation on the native backend"
        );
        assert!(
            matches!(
                t.stop_local_forward(&crate::transport::contract::ForwardId("x".into())),
                Err(TransportError::UnsupportedOperation(_))
            ),
            "stop_local_forward must be UnsupportedOperation on the native backend"
        );
    }

    // ===== In-process SSH server harness =====
    //
    // These tests drive the real native transport against an in-process russh
    // SSH server, so the behaviour assertions — connection reuse, concurrent
    // channels, deadline-bounded lock waits, establishment timeouts and
    // liveness probes — run against actual SSH wire traffic, not mocks.

    #[derive(Clone, Default)]
    struct ServerBehavior {
        /// Delay the exec reply when the command equals this (key, delay) pair.
        exec_delay_for: Option<(String, Duration)>,
        /// Never complete authentication: connection establishment hangs.
        hold_auth: bool,
        /// Reply to every exec with this payload and exit status 0.
        exec_reply: Option<Vec<u8>>,
        /// Disconnect the session when an exec whose command equals this
        /// arrives (simulates the remote going away mid-use).
        disconnect_on_exec: Option<String>,
        /// Sleep, then disconnect, when the exec command equals this (key,
        /// delay) pair — widens the failing exec's window so a test can take
        /// the handle lock while the stale request is still in flight.
        disconnect_delay_for: Option<(String, Duration)>,
        /// When the exec command equals this, signal `started` and then block
        /// on `release` before replying — a deterministic slow-command gate.
        gate_on_exec: Option<(String, Gate)>,
    }

    /// Deterministic slow-command gate. The server signals `started` the
    /// moment the gated command arrives, then waits on `release` before
    /// replying. The test waits on `started` (so it knows the slow command is
    /// actually in progress), runs a fast command, and only then releases the
    /// slow one — no sleep-based timing guesses.
    #[derive(Clone, Default)]
    struct Gate {
        started: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
        release: Arc<tokio::sync::Notify>,
    }

    impl Gate {
        fn new() -> Self {
            Self::default()
        }

        fn signal_started(&self) {
            let mut g = self.started.0.lock().unwrap();
            *g = true;
            self.started.1.notify_all();
        }

        fn wait_started(&self) {
            let mut g = self.started.0.lock().unwrap();
            while !*g {
                g = self.started.1.wait(g).unwrap();
            }
        }

        fn release(&self) {
            self.release.notify_waiters();
        }

        async fn wait_release(&self) {
            self.release.notified().await;
        }
    }

    /// Wraps the server-side TCP stream and counts bytes read from the
    /// client. SSH payloads are encrypted, so a count of *packets* is not
    /// visible here, but during a quiet idle gap the only traffic a client
    /// sends is keepalive global requests — a growing receive byte count is
    /// direct evidence the keepalive loop is running on the background
    /// runtime.
    struct CountingStream<S> {
        inner: S,
        received: Arc<AtomicU64>,
    }

    impl<S> CountingStream<S> {
        fn new(inner: S, received: Arc<AtomicU64>) -> Self {
            Self { inner, received }
        }
    }

    impl<S: AsyncRead + Unpin> AsyncRead for CountingStream<S> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let before = buf.filled().len();
            let result = Pin::new(&mut self.inner).poll_read(cx, buf);
            if result.is_ready() {
                let n = buf.filled().len().saturating_sub(before);
                self.received.fetch_add(n as u64, Ordering::Relaxed);
            }
            result
        }
    }

    impl<S: AsyncWrite + Unpin> AsyncWrite for CountingStream<S> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.inner).poll_write(cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[derive(Clone)]
    struct TestHandler {
        behavior: Arc<ServerBehavior>,
    }

    impl server::Handler for TestHandler {
        type Error = russh::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
            if self.behavior.hold_auth {
                return std::future::pending::<Result<Auth, Self::Error>>().await;
            }
            Ok(Auth::reject())
        }

        async fn auth_publickey(
            &mut self,
            _user: &str,
            _key: &russh::keys::PublicKey,
        ) -> Result<Auth, Self::Error> {
            if self.behavior.hold_auth {
                return std::future::pending::<Result<Auth, Self::Error>>().await;
            }
            Ok(Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            _channel: russh::Channel<russh::server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: russh::ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            let cmd = String::from_utf8_lossy(data).to_string();
            if self
                .behavior
                .disconnect_on_exec
                .as_ref()
                .is_some_and(|needle| needle == &cmd)
            {
                session.disconnect(russh::Disconnect::ByApplication, "test shutdown", "")?;
                return Ok(());
            }
            if let Some((needle, delay)) = &self.behavior.disconnect_delay_for {
                if cmd == *needle {
                    tokio::time::sleep(*delay).await;
                    session.disconnect(russh::Disconnect::ByApplication, "test shutdown", "")?;
                    return Ok(());
                }
            }
            if let Some((needle, gate)) = &self.behavior.gate_on_exec {
                if cmd == *needle {
                    gate.signal_started();
                    // Schedule the delayed reply on a separate task so the
                    // gated command does not block the connection's request
                    // handling (a real sshd services channels independently;
                    // the test server must behave the same or the client-side
                    // concurrency claim cannot be exercised).
                    let handle = session.handle();
                    let gate = gate.clone();
                    let payload = self.behavior.exec_reply.clone();
                    tokio::spawn(async move {
                        gate.wait_release().await;
                        if let Some(payload) = payload {
                            let _ = handle.data(channel, payload).await;
                            let _ = handle.exit_status_request(channel, 0).await;
                            let _ = handle.eof(channel).await;
                            let _ = handle.close(channel).await;
                        }
                    });
                    return Ok(());
                }
            }
            if let Some((needle, delay)) = &self.behavior.exec_delay_for {
                if cmd == *needle {
                    tokio::time::sleep(*delay).await;
                }
            }
            match &self.behavior.exec_reply {
                Some(payload) => {
                    session.data(channel, payload.clone())?;
                    session.exit_status_request(channel, 0)?;
                    session.eof(channel)?;
                    session.close(channel)?;
                    Ok(())
                }
                None => std::future::pending::<Result<(), Self::Error>>().await,
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    struct TestServer {
        port: u16,
        pub_key: russh::keys::PublicKey,
        shutdown_tx: Option<oneshot::Sender<()>>,
        bytes_received: Arc<AtomicU64>,
        _runtime: tokio::runtime::Runtime,
    }

    impl TestServer {
        fn start(behavior: ServerBehavior) -> Self {
            let server_key =
                russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                    .expect("generate server ed25519 key");
            let pub_key = server_key.public_key().clone();
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            let (port_tx, port_rx) = oneshot::channel();
            let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
            let behavior = Arc::new(behavior);
            let bytes_received = Arc::new(AtomicU64::new(0));
            let br_total = Arc::clone(&bytes_received);
            runtime.spawn(async move {
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                    .await
                    .expect("bind test server");
                let port = listener.local_addr().unwrap().port();
                let _ = port_tx.send(port);
                let config = Arc::new(server::Config {
                    keys: vec![server_key],
                    ..Default::default()
                });
                let mut sessions = Vec::new();
                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => break,
                        accepted = listener.accept() => {
                            let (stream, _) = accepted.expect("accept client");
                            let cfg = Arc::clone(&config);
                            let bh = Arc::clone(&behavior);
                            let br = Arc::clone(&br_total);
                            sessions.push(tokio::spawn(async move {
                                let stream = CountingStream::new(stream, br);
                                let _ = server::run_stream(cfg, stream, TestHandler { behavior: bh })
                                    .await;
                            }));
                        }
                    }
                }
                drop(listener);
                for s in sessions {
                    s.abort();
                }
            });
            let port = port_rx.blocking_recv().expect("test server port");
            TestServer {
                port,
                pub_key: pub_key.clone(),
                shutdown_tx: Some(shutdown_tx),
                bytes_received,
                _runtime: runtime,
            }
        }

        fn stop(&mut self) {
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }
        }
    }

    fn dl(secs: u64) -> Deadline {
        Deadline::from_now(Duration::from_secs(secs))
    }

    /// Build a `NativeTransport` pointed at the in-process server, with a
    /// freshly generated client identity and a known_hosts entry trusting the
    /// server key. Returns the transport plus the temp dir that keeps the
    /// client key file alive.
    fn test_transport(
        server: &TestServer,
        keepalive: Duration,
        keepalive_failures: u32,
    ) -> (NativeTransport, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let key_path = dir.path().join("client_ed25519");
        let client_key =
            russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .expect("generate client ed25519 key");
        let openssh = client_key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .expect("encode client key");
        std::fs::write(&key_path, openssh.as_bytes()).expect("write client key");

        let mut kh = KnownHosts::memory();
        let b64 = base64::engine::general_purpose::STANDARD
            .encode(server.pub_key.to_bytes().expect("encode server host key"));
        kh.trust("127.0.0.1", Some(server.port), &KeyType::SshEd25519, &b64)
            .expect("trust server host key");

        let session = Arc::new(SessionState::new().expect("session state"));
        let t = NativeTransport {
            config: NativeTransportConfig {
                host: "127.0.0.1".into(),
                user: Some("test".into()),
                ssh_port: server.port,
                jump_host: None,
                key_path: Some(key_path),
                known_hosts: vec![kh],
                connect_timeout: Duration::from_secs(10),
                keepalive: KeepalivePolicy {
                    interval: keepalive,
                    max_failures: keepalive_failures,
                },
                reconnect: ReconnectPolicy {
                    max_attempts: 2,
                    max_delay: Duration::from_secs(1),
                    base: Duration::from_millis(50),
                },
            },
            session,
        };
        (t, dir)
    }

    /// Real handshake reuse across sequential operations on one transport:
    /// three operations, exactly one SSH connection.
    #[test]
    fn native_reuses_one_live_connection_across_operations() {
        let server = TestServer::start(ServerBehavior {
            exec_reply: Some(b"ok".to_vec()),
            ..Default::default()
        });
        let (t, _dir) = test_transport(&server, Duration::from_secs(30), 3);
        let rid = RequestId::new();
        let r1 = t
            .exec_command("echo", None, dl(10), &rid)
            .expect("first exec");
        assert_eq!(r1.stdout, b"ok");
        let r2 = t
            .exec_command("echo", None, dl(10), &rid)
            .expect("second exec");
        assert_eq!(r2.stdout, b"ok");
        assert!(
            t.test_connection(dl(10)).expect("liveness probe"),
            "probe must succeed while the server is up"
        );
        assert_eq!(
            t.session.connections.load(Ordering::Relaxed),
            1,
            "three operations must share exactly one real SSH connection"
        );
    }

    /// P1-2 concurrency: a slow command on one channel must not block a fast
    /// command — each operation opens its own channel and the handle lock is
    /// released as soon as the channel exists. The slow command is gated with
    /// a synchronisation signal rather than a sleep: the test waits until the
    /// server confirms the slow command is in flight, runs the fast command,
    /// asserts it completes, and only then releases the slow command. A
    /// serialising implementation would hold the handle lock until the slow
    /// command finished, so the fast command could not complete before the
    /// release.
    #[test]
    fn concurrent_commands_run_on_independent_channels() {
        let gate = Arc::new(Gate::new());
        let server = TestServer::start(ServerBehavior {
            gate_on_exec: Some(("slow".to_string(), (*gate).clone())),
            exec_reply: Some(b"ok".to_vec()),
            ..Default::default()
        });
        let (t, _dir) = test_transport(&server, Duration::from_secs(30), 3);
        let rid = RequestId::new();
        t.exec_command("warm", None, dl(10), &rid).expect("warmup");
        let slow_rid = rid.clone();
        let slow = {
            let t = t.clone();
            std::thread::spawn(move || t.exec_command("slow", None, dl(10), &slow_rid))
        };
        // Deterministic: wait until the slow command has actually started on
        // the server (its channel is open and the exec is being processed)
        // before touching the fast command.
        gate.wait_started();
        let started = std::time::Instant::now();
        let fast = t
            .exec_command("fast", None, dl(10), &rid)
            .expect("fast exec");
        let fast_elapsed = started.elapsed();
        assert_eq!(fast.stdout, b"ok");
        assert!(
            fast_elapsed < Duration::from_millis(500),
            "fast command must not wait for the slow command's channel (got {fast_elapsed:?})"
        );
        // The slow command is still blocked: release it and only then may it
        // complete — proving the fast command finished first.
        gate.release();
        let slow_out = slow.join().expect("slow thread").expect("slow exec");
        assert_eq!(slow_out.stdout, b"ok");
        assert_eq!(
            t.session.connections.load(Ordering::Relaxed),
            1,
            "both channels must share one connection"
        );
    }

    /// P1-2 lock wait + P1-3 establishment budget: while one request is stuck
    /// establishing (holding the handle lock), a second request with a short
    /// deadline must return within its own budget instead of waiting for the
    /// first to finish.
    #[test]
    fn lock_wait_and_establishment_respect_the_request_deadline() {
        let server = TestServer::start(ServerBehavior {
            hold_auth: true,
            ..Default::default()
        });
        let (t, _dir) = test_transport(&server, Duration::from_secs(30), 3);
        let rid = RequestId::new();
        let stuck_rid = rid.clone();
        let stuck = {
            let t = t.clone();
            std::thread::spawn(move || t.exec_command("a", None, dl(10), &stuck_rid))
        };
        std::thread::sleep(Duration::from_millis(300));
        let started = std::time::Instant::now();
        let short = t.exec_command("b", None, dl(1), &rid);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "waiting for the handle must be bounded by the request deadline (got {elapsed:?})"
        );
        assert!(
            short.is_err(),
            "establishment hangs under hold_auth, so the short request must time out"
        );
        let _ = stuck.join().expect("stuck thread");
    }

    /// P1 failure-cleanup deadline: after a stale request's connection-level
    /// failure, the generation-matched eviction must be bounded by the
    /// request's remaining deadline — it must never block behind a concurrent
    /// slow re-establishment holding the handle lock. The concurrent
    /// re-establishment is simulated by holding the lock (the deterministic
    /// equivalent of a connection stuck in authentication, as hold_auth would
    /// produce), and the stale request's operation returns a transient
    /// connection failure mid-flight. The stale request must return on its own
    /// deadline — before the lock is released — and the closed handle must
    /// still be replaced by the next operation even when the eviction was
    /// skipped.
    #[test]
    fn stale_failure_cleanup_is_bounded_by_the_request_deadline() {
        let server = TestServer::start(ServerBehavior {
            exec_reply: Some(b"ok".to_vec()),
            // "die" sleeps 800 ms, then disconnects — a real connection death.
            disconnect_delay_for: Some(("die".to_string(), Duration::from_millis(800))),
            ..Default::default()
        });
        let (t, _dir) = test_transport(&server, Duration::from_secs(30), 3);
        let rid = RequestId::new();
        t.exec_command("warm", None, dl(10), &rid).expect("warmup");
        // A: drive with_channel directly; its operation kills the connection
        // mid-flight (server disconnect) and then fails with a transient
        // connection error — the path whose cleanup must respect the
        // deadline. B takes the handle lock while A's operation is still in
        // flight.
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let a_rid = rid.clone();
        let a = {
            let t = t.clone();
            let done_tx = done_tx.clone();
            std::thread::spawn(move || {
                let result: Result<(), TransportError> =
                    t.with_channel(dl(2), &a_rid, |channel, rt, _remaining| {
                        // Real mid-use death: the server disconnects ~800 ms
                        // after receiving "die".
                        rt.block_on(async move {
                            let mut ch = channel;
                            let _ = ch.exec(false, "die".to_string()).await;
                            let _ = tokio::time::timeout(Duration::from_millis(1500), async {
                                while ch.wait().await.is_some() {}
                            })
                            .await;
                        });
                        Err(TransportError::ConnectionFailed(
                            "simulated mid-use connection failure".into(),
                        ))
                    });
                let _ = done_tx.send(());
                result
            })
        };
        // Take the handle lock while A's operation is in flight, simulating a
        // concurrent re-establishment stuck in auth. Hold it well past A's
        // deadline.
        std::thread::sleep(Duration::from_millis(150));
        let guard = t.session.inner.blocking_lock();
        std::thread::sleep(Duration::from_secs(3));
        let returned_early = done_rx.recv_timeout(Duration::from_millis(300)).is_ok();
        drop(guard);
        assert!(
            returned_early,
            "stale request's cleanup must be bounded by its remaining deadline and must not wait \
             for the concurrent re-establishment to release the handle lock"
        );
        let a_res = a.join().expect("stale thread");
        assert!(
            a_res.is_err(),
            "the stale request must surface its connection failure"
        );
        // Eviction may have been skipped (the lock was unreachable before the
        // deadline); the dead handle must still be re-detected and replaced by
        // the next operation — failure marking stays effective.
        let final_res = t
            .exec_command("final", None, dl(5), &rid)
            .expect("next operation rebuilds");
        assert_eq!(final_res.stdout, b"ok");
        assert_eq!(
            t.session.connections.load(Ordering::Relaxed),
            2,
            "the dead session must be replaced by a fresh connection"
        );
    }

    /// P1-1 liveness: after the remote goes away, test_connection must stop
    /// reporting healthy — the probe is a real ping/pong, not a cached
    /// success based on the handle existing.
    #[test]
    fn test_connection_reports_unhealthy_after_remote_disconnect() {
        let server = TestServer::start(ServerBehavior {
            exec_reply: Some(b"ok".to_vec()),
            disconnect_on_exec: Some("die".into()),
            ..Default::default()
        });
        let (t, _dir) = test_transport(&server, Duration::from_secs(30), 3);
        let rid = RequestId::new();
        t.exec_command("warm", None, dl(10), &rid).expect("warmup");
        assert!(
            t.test_connection(dl(10)).expect("healthy probe"),
            "probe is healthy while the server is up"
        );
        // The remote tears the session down mid-use; the exec fails, and the
        // liveness probe afterwards must not report healthy.
        let _ = t.exec_command("die", None, dl(5), &rid);
        std::thread::sleep(Duration::from_millis(800));
        let probe = t.test_connection(dl(3));
        assert!(
            probe != Ok(true),
            "probe after remote disconnect must not report healthy: {probe:?}"
        );
    }

    /// P1-1 background liveness: with a short keepalive interval, the shared
    /// multi-thread runtime keeps sending keepalives while the transport is
    /// idle — the server must observe incoming traffic during a quiet gap
    /// longer than several keepalive periods, with no client-side operation
    /// and no reconnect.
    #[test]
    fn idle_connection_sends_keepalives_on_the_background_runtime() {
        let server = TestServer::start(ServerBehavior {
            exec_reply: Some(b"ok".to_vec()),
            ..Default::default()
        });
        // 500 ms keepalive interval; idle across >3 periods.
        let (t, _dir) = test_transport(&server, Duration::from_millis(500), 3);
        let rid = RequestId::new();
        t.exec_command("warm", None, dl(10), &rid).expect("warmup");
        // Let the warmup's channel teardown traffic (eof/close) drain before
        // the baseline, so the observation cannot be attributed to it.
        std::thread::sleep(Duration::from_millis(600));
        let before = server.bytes_received.load(Ordering::Relaxed);
        // No client operation: only the background keepalive loop may produce
        // traffic. Sample across two consecutive keepalive periods and require
        // sustained growth in each — a one-shot teardown burst can never
        // satisfy both mid > before and after > mid.
        std::thread::sleep(Duration::from_millis(600));
        let mid = server.bytes_received.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(600));
        let after = server.bytes_received.load(Ordering::Relaxed);
        assert!(
            mid > before,
            "the server must receive keepalive traffic in the first idle period \
             (before={before} mid={mid})"
        );
        assert!(
            after > mid,
            "keepalive traffic must continue across the second idle period \
             (mid={mid} after={after})"
        );
        assert!(
            t.test_connection(dl(5)).expect("probe after idle"),
            "connection must stay usable after an idle gap"
        );
        assert_eq!(
            t.session.connections.load(Ordering::Relaxed),
            1,
            "no reconnect: the idle connection was kept alive"
        );
    }
}
