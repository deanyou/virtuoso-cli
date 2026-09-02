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
//! - ❌ connection pooling / channel scheduling — each operation reconnects
//!   (the design's reuse requirement is step 4's daemon);
//! - ❌ `ProxyJump` / jump-host routing, SOCKS5, RAMIC `direct-tcpip` forward,
//!   agent/password/keyboard-interactive auth, and the IPC transport-daemon —
//!   all later increments. Those paths return a clear `UnsupportedOperation`
//!   rather than silently misbehaving.
//!
//! The contract methods are synchronous; russh is async. Each call spins up a
//! fresh current-thread tokio runtime and `block_on`s the async work. That is
//! deliberate: it keeps the async runtime entirely inside this module (never
//! shared with the synchronous business layer) and avoids holding a `!Sync` russh
//! session across a `&mut self` boundary.

#![cfg(feature = "native-ssh")]

use std::path::{Path, PathBuf};
use std::process::Command as SyncCommand;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use russh::client::{self, Handler};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::ChannelMsg;
use russh::Disconnect;
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

/// A fresh current-thread runtime for one synchronous call into russh.
fn make_runtime() -> Result<tokio::runtime::Runtime, TransportError> {
    tokio::runtime::Builder::new_current_thread()
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

/// Open an exec channel, optionally feed `stdin`, and drain stdout/stderr/status.
async fn exec_command(
    cfg: &NativeTransportConfig,
    command: &str,
    stdin: Option<Vec<u8>>,
    deadline: Deadline,
) -> Result<RawOutput, TransportError> {
    let session = establish_with_retry(cfg, deadline).await?;
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(map_russh_error)?;
    channel
        .exec(false, command.to_owned())
        .await
        .map_err(map_russh_error)?;
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
            ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
            ChannelMsg::ExtendedData { .. } => {}
            ChannelMsg::ExitStatus { exit_status: c } => exit_status = c as i32,
            ChannelMsg::ExitSignal { .. } => exit_status = -1,
            _ => {}
        }
    }
    session
        .disconnect(Disconnect::ByApplication, "", "English")
        .await
        .ok();
    Ok(RawOutput {
        stdout,
        stderr,
        exit_status,
    })
}

/// 64 KiB SFTP write/read windows — small enough to stay beneath the SSH
/// channel's max packet size while streaming large files without buffering
/// them entirely in memory (design: "the daemon streams the local file
/// itself", never the whole buffer across one frame).
const SFTP_CHUNK: usize = 64 * 1024;

/// Whether an SFTP attempt failed because the remote did not advertise the
/// sftp subsystem (vs a connection/auth failure that must not be retried
/// through the exec pipe). Callers use this to fall back to the exec `cat`
/// transfer that directories already rely on.
fn sftp_unavailable(e: &TransportError) -> bool {
    matches!(e, TransportError::UnsupportedOperation(_))
}

/// Establish a session and open the SFTP subsystem channel.
///
/// Returns the russh handle (kept alive for the duration of the file op) and a
/// high-level [`SftpSession`]. A server that does not advertise sftp surfaces
/// as [`TransportError::UnsupportedOperation`].
async fn open_sftp(
    cfg: &NativeTransportConfig,
    deadline: Deadline,
) -> Result<(client::Handle<NativeClientHandler>, SftpSession), TransportError> {
    let session = establish_with_retry(cfg, deadline).await?;
    let channel = session
        .channel_open_session()
        .await
        .map_err(map_russh_error)?;
    // A server that rejects the subsystem returns an error here; map it to
    // `UnsupportedOperation` so the caller can fall back to exec transfer.
    channel.request_subsystem(true, "sftp").await.map_err(|e| {
        TransportError::UnsupportedOperation(format!(
            "sftp subsystem unavailable on {}: {e}",
            cfg.host
        ))
    })?;
    let stream = channel.into_stream();
    let sftp = SftpSession::new(stream).await.map_err(|e| {
        TransportError::UnsupportedOperation(format!(
            "sftp session init failed on {}: {e}",
            cfg.host
        ))
    })?;
    Ok((session, sftp))
}

/// Upload `data` to `remote` over the SFTP subsystem, streaming in 64 KiB
/// windows. A missing sftp subsystem surfaces as `UnsupportedOperation`.
async fn upload_via_sftp(
    cfg: &NativeTransportConfig,
    remote: &Path,
    data: Vec<u8>,
    deadline: Deadline,
) -> Result<(), TransportError> {
    let (_session, sftp) = open_sftp(cfg, deadline).await?;
    let remote_str = remote.to_string_lossy().into_owned();
    let mut file =
        sftp.create(&remote_str)
            .await
            .map_err(|e| TransportError::TransferInterrupted {
                request: RequestId::new(),
                reason: format!("sftp create {remote_str}: {e}"),
            })?;
    sftp_write_all(&mut file, &data).await?;
    Ok(())
}

/// Download `remote` over the SFTP subsystem, streaming in 64 KiB windows into
/// a local buffer. A missing sftp subsystem surfaces as `UnsupportedOperation`.
async fn download_via_sftp(
    cfg: &NativeTransportConfig,
    remote: &Path,
    deadline: Deadline,
) -> Result<Vec<u8>, TransportError> {
    let (_session, sftp) = open_sftp(cfg, deadline).await?;
    let remote_str = remote.to_string_lossy().into_owned();
    let mut file =
        sftp.open(&remote_str)
            .await
            .map_err(|e| TransportError::TransferInterrupted {
                request: RequestId::new(),
                reason: format!("sftp open {remote_str}: {e}"),
            })?;
    let buf = sftp_read_all(&mut file).await?;
    Ok(buf)
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

/// Run `fut` on a fresh runtime, bounding it by `deadline`. A timeout surfaces as
/// `ExecutionTimeout` carrying `req_id` (termination unproven — conservative).
fn block_with_deadline<F, Fut, T>(
    deadline: Deadline,
    req_id: RequestId,
    fut: F,
) -> Result<T, TransportError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, TransportError>>,
{
    let rt = make_runtime()?;
    let span = deadline.remaining();
    rt.block_on(async move {
        match tokio::time::timeout(span, fut()).await {
            Ok(r) => r,
            Err(_) => Err(TransportError::ExecutionTimeout {
                request: req_id,
                after_secs: span.as_secs().max(1),
                remote_terminated: false,
            }),
        }
    })
}

/// The native transport: a resolved endpoint plus the sync bridge.
#[derive(Clone, Debug)]
pub struct NativeTransport {
    config: NativeTransportConfig,
}

impl NativeTransport {
    /// Build from the shared `Config`. Fails loudly (never falls back) when the
    /// native backend cannot satisfy the request: missing `VB_REMOTE_HOST`, or
    /// no `VB_SSH_KEY` (step 3 is public-key only).
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
        })
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
        let cfg = self.config.clone();
        block_with_deadline(deadline, RequestId::new(), move || async move {
            // test_connection is an explicit idempotent probe, so re-establishing
            // the path within the deadline is allowed here.
            match establish_with_retry(&cfg, deadline).await {
                Ok(_) => Ok(true),
                // A connection-level failure means the host is not reachable;
                // host-key / auth failures are real errors, not "unreachable".
                Err(e) => match e {
                    TransportError::ConnectionFailed(_)
                    | TransportError::HostKeyUnknown { .. }
                    | TransportError::HostKeyChanged { .. }
                    | TransportError::HostKeyPolicyUnsupported(_)
                    | TransportError::AuthenticationFailed(_) => Err(e),
                    _ => Ok(false),
                },
            }
        })
    }

    fn run_command(&self, req: &CommandRequest) -> Result<CommandResult, TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        let cfg = self.config.clone();
        let cmd = req.command.clone();
        let req_id = req.id.clone();
        block_with_deadline(req.deadline, req_id, move || async move {
            let raw = exec_command(&cfg, &cmd, None, req.deadline).await?;
            Ok(CommandResult {
                exit_status: raw.exit_status,
                stdout: String::from_utf8_lossy(&raw.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&raw.stderr).into_owned(),
                success: raw.exit_status == 0,
                duration: Duration::ZERO,
            })
        })
    }

    fn upload_file(&self, req: &UploadFileRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        let cfg = self.config.clone();
        let local = req.local.clone();
        let remote = req.remote.clone();
        let req_id = req.id.clone();
        block_with_deadline(req.deadline, req_id, move || async move {
            let bytes = std::fs::read(&local)
                .map_err(|e| TransportError::LocalIo(format!("read {}: {e}", local.display())))?;
            // Single files stream over the SFTP subsystem (design step 4). Where
            // the remote does not advertise sftp, fall back to the exec `cat`
            // pipe that directories already rely on.
            // Clone for the SFTP attempt; the exec fallback still needs the
            // original bytes if the remote lacks the sftp subsystem.
            match upload_via_sftp(&cfg, Path::new(&remote), bytes.clone(), req.deadline).await {
                Ok(()) => Ok(()),
                Err(e) if sftp_unavailable(&e) => {
                    let cmd = format!("cat > {}", shell_quote(&remote));
                    let raw = exec_command(&cfg, &cmd, Some(bytes), req.deadline).await?;
                    if raw.exit_status != 0 {
                        return Err(TransportError::TransferInterrupted {
                            request: req.id.clone(),
                            reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
                        });
                    }
                    Ok(())
                }
                Err(e) => Err(e),
            }
        })
    }

    fn upload_text(&self, req: &UploadTextRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        let cfg = self.config.clone();
        let bytes = req.text.clone().into_bytes();
        let remote = req.remote.clone();
        let req_id = req.id.clone();
        block_with_deadline(req.deadline, req_id, move || async move {
            let cmd = format!("cat > {}", shell_quote(&remote));
            let raw = exec_command(&cfg, &cmd, Some(bytes), req.deadline).await?;
            if raw.exit_status != 0 {
                return Err(TransportError::TransferInterrupted {
                    request: req.id.clone(),
                    reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
                });
            }
            Ok(())
        })
    }

    fn download_file(&self, req: &DownloadFileRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        let cfg = self.config.clone();
        let remote = req.remote.clone();
        let local = req.local.clone();
        let req_id = req.id.clone();
        block_with_deadline(req.deadline, req_id, move || async move {
            // Single files stream over the SFTP subsystem (design step 4); a
            // remote without sftp falls back to the exec `cat` pipe.
            match download_via_sftp(&cfg, Path::new(&remote), req.deadline).await {
                Ok(bytes) => {
                    std::fs::write(&local, &bytes).map_err(|e| {
                        TransportError::LocalIo(format!("write {}: {e}", local.display()))
                    })?;
                    Ok(())
                }
                Err(e) if sftp_unavailable(&e) => {
                    let cmd = format!("cat {}", shell_quote(&remote));
                    let raw = exec_command(&cfg, &cmd, None, req.deadline).await?;
                    if raw.exit_status != 0 {
                        return Err(TransportError::TransferInterrupted {
                            request: req.id.clone(),
                            reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
                        });
                    }
                    std::fs::write(&local, &raw.stdout).map_err(|e| {
                        TransportError::LocalIo(format!("write {}: {e}", local.display()))
                    })?;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        })
    }

    fn download_dir(&self, req: &DownloadDirRequest) -> Result<(), TransportError> {
        if req.deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: req.id.clone(),
                after_secs: 0,
            });
        }
        let cfg = self.config.clone();
        let remote = req.remote.clone();
        let local = req.local.clone();
        let req_id = req.id.clone();
        block_with_deadline(req.deadline, req_id, move || async move {
            let cmd = format!("tar -cf - -C {} .", shell_quote(&remote));
            let raw = exec_command(&cfg, &cmd, None, req.deadline).await?;
            if raw.exit_status != 0 {
                return Err(TransportError::TransferInterrupted {
                    request: req.id.clone(),
                    reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
                });
            }
            std::fs::create_dir_all(&local)
                .map_err(|e| TransportError::LocalIo(format!("create {}: {e}", local.display())))?;
            // Untar locally. The remote side already produced the tar stream, so
            // the local `tar` requirement mirrors the OpenSSH backend's remote
            // `tar` requirement (no extra remote toolchain divergence).
            let tmp = local.join(format!(".vcli-dl-{}.tar", std::process::id()));
            std::fs::write(&tmp, &raw.stdout)
                .map_err(|e| TransportError::LocalIo(format!("stage tar: {e}")))?;
            let status = SyncCommand::new("tar")
                .arg("-xf")
                .arg(&tmp)
                .arg("-C")
                .arg(&local)
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::contract::test_support::shared_contract_suite;

    fn cfg_with(host: &str, key: Option<&str>, jump: Option<&str>) -> Config {
        Config {
            profile: None,
            remote_host: Some(host.into()),
            remote_user: None,
            port: 65432,
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
        }
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
        rt.block_on(async {
            let t = NativeTransport::from_config(&cfg_with("h", Some("/tmp/k"), Some("jump")))
                .expect("from_config accepts a jump host (deferred, not rejected at construction)");
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
}
