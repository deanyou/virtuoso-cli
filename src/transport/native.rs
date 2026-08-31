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
//! - ✅ command exec + file/text/dir transfer over the exec channel
//!   (tar-over-exec for directories, matching the OpenSSH backend);
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

    let client_cfg = Arc::new(client::Config::default());
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

/// Open an exec channel, optionally feed `stdin`, and drain stdout/stderr/status.
async fn exec_command(
    cfg: &NativeTransportConfig,
    command: &str,
    stdin: Option<Vec<u8>>,
) -> Result<RawOutput, TransportError> {
    let session = establish(cfg).await?;
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

        Ok(NativeTransport {
            config: NativeTransportConfig {
                host,
                user: config.remote_user.clone(),
                ssh_port,
                jump_host: config.jump_host.clone(),
                key_path: Some(key_path),
                known_hosts: stores,
                connect_timeout: Duration::from_secs(config.timeout.max(5)),
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
            match establish(&cfg).await {
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
            let raw = exec_command(&cfg, &cmd, None).await?;
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
            let cmd = format!("cat > {}", shell_quote(&remote));
            let raw = exec_command(&cfg, &cmd, Some(bytes)).await?;
            if raw.exit_status != 0 {
                return Err(TransportError::TransferInterrupted {
                    request: req.id.clone(),
                    reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
                });
            }
            Ok(())
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
            let raw = exec_command(&cfg, &cmd, Some(bytes)).await?;
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
            let cmd = format!("cat {}", shell_quote(&remote));
            let raw = exec_command(&cfg, &cmd, None).await?;
            if raw.exit_status != 0 {
                return Err(TransportError::TransferInterrupted {
                    request: req.id.clone(),
                    reason: String::from_utf8_lossy(&raw.stderr).into_owned(),
                });
            }
            std::fs::write(&local, &raw.stdout)
                .map_err(|e| TransportError::LocalIo(format!("write {}: {e}", local.display())))?;
            Ok(())
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
            let raw = exec_command(&cfg, &cmd, None).await?;
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
}
