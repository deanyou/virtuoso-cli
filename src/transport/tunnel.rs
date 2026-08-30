#![allow(dead_code)]

use crate::config::Config;
use crate::error::{Result, VirtuosoError};
use crate::models::TunnelState;
use crate::transport::contract::RemoteTransport;
use crate::transport::daemon_lifecycle::{self, Verdict};
use crate::transport::openssh::OpenSshTransport;
use crate::transport::ssh::SSHRunner;
use include_dir::{include_dir, Dir};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

static RESOURCES: Dir = include_dir!("$CARGO_MANIFEST_DIR/resources");

// =============================================================================
// Profile-isolated setup dir helpers
//
// Multi-profile setups previously wrote every profile's CIW setup file
// (`ramic_bridge.il`) to the same remote path, so a second profile
// silently overwrote the first profile's setup file and the first
// profile's CIW `load()` would start the wrong daemon. The helpers
// below isolate per-profile scratch + env keys so concurrent
// profiles can coexist on the same remote host without colliding.
//
// Mirrors the upstream pattern (virtuoso-bridge PR #86) with a
// Rust-friendly surface and the same sanitization rules.
// =============================================================================

/// Remote bridge directory leaf for a given profile.
///
/// - `None` (no profile): unchanged `virtuoso_bridge`
/// - `Some(name)`: `virtuoso_bridge_<sanitized>`, length-capped at 64 chars
///
/// Sanitization: any char outside `[A-Za-z0-9._-]` is replaced with `_`.
/// An all-stripped result (e.g. profile=`"///"`) or an all-underscore
/// result (e.g. profile=`"___"`) falls back to `"profile"` to avoid
/// collisions with the no-profile leaf.
pub fn profiled_bridge_leaf(profile: Option<&str>) -> String {
    match profile {
        None => "virtuoso_bridge".to_string(),
        Some(p) => {
            let safe: String = p
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .take(64)
                .collect();
            // If sanitization left no meaningful content (empty, or
            // all underscores), fall back to a fixed name to avoid
            // collisions and a "virtuoso_bridge_" leaf that shadows
            // the no-profile case.
            let meaningful = safe.chars().any(|c| c != '_');
            if !meaningful {
                "virtuoso_bridge_profile".to_string()
            } else {
                format!("virtuoso_bridge_{safe}")
            }
        }
    }
}

/// Profile-suffixed env-var key. Mirrors the `VB_LOCAL_PORT_<profile>`
/// convention used by the upstream bridge to keep port-collision state
/// per-profile.
///
/// - `None` (no profile): returns `base` unchanged
/// - `Some(profile)`: returns `format!("{base}_{profile}")`
pub fn profiled_env_key(base: &str, profile: Option<&str>) -> String {
    match profile {
        None => base.to_string(),
        Some(p) => format!("{base}_{p}"),
    }
}

/// Absolute remote setup dir for a given profile. Always rooted at
/// `/tmp/` — the remote's `tmpfs`-backed location — so cleanup is cheap.
pub fn setup_dir_for_profile(profile: Option<&str>) -> String {
    format!("/tmp/{}", profiled_bridge_leaf(profile))
}

/// Whether a binary path is the ssh client (`/usr/bin/ssh`, `ssh.exe`, …).
///
/// Deliberately as loose as the Linux check below (`cmdline.contains("ssh")`):
/// the tunnel is spawned as `ssh -N -L …`, and being wrong here only chooses
/// between "kill it" and "warn and skip", both of which are recoverable.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn is_ssh_executable(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| name.contains("ssh"))
        .unwrap_or(false)
}

/// Linux: verify via `/proc/<pid>/cmdline`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn verify_ssh_pid(pid: u32) -> bool {
    let cmdline_path = format!("/proc/{pid}/cmdline");
    match std::fs::read_to_string(&cmdline_path) {
        Ok(cmdline) => cmdline.contains("ssh"),
        Err(_) => false,
    }
}

/// macOS has no `/proc`, so the executable is resolved through `sysctl`
/// (`KERN_PROCARGS2`) instead.
///
/// This branch is not cosmetic: with the `/proc`-only check, `verify_ssh_pid`
/// returned `false` for every pid on macOS, which made `tunnel stop` skip the
/// kill entirely and left the ssh process running.
#[cfg(target_os = "macos")]
fn verify_ssh_pid(pid: u32) -> bool {
    match crate::transport::identity::ProcessIdentity::of_pid(pid) {
        Ok(identity) => is_ssh_executable(&identity.executable_path),
        Err(_) => false,
    }
}

/// Windows: verify through the process's executable, resolved by
/// `identity::ProcessIdentity` (`OpenProcess` + `QueryFullProcessImageNameW`).
///
/// This closes the gap the design's "Stop and crash recovery" calls out: the
/// previous behaviour trusted the PID unconditionally, so `tunnel stop` would
/// `taskkill /F` whatever happened to be using it.
#[cfg(target_os = "windows")]
fn verify_ssh_pid(pid: u32) -> bool {
    match crate::transport::identity::ProcessIdentity::of_pid(pid) {
        Ok(identity) => is_ssh_executable(&identity.executable_path),
        Err(_) => false,
    }
}

/// Genuinely unknown platforms: no mechanism exists, so the PID is still
/// trusted. Every platform the design targets now has a real check; this arm
/// only covers targets outside that set.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "windows"
)))]
fn verify_ssh_pid(pid: u32) -> bool {
    let _ = pid;
    true
}

/// Non-destructive "is a process with this pid running" probe for Windows.
///
/// `tasklist /FI "PID eq <n>" /NH` prints the matching row, or an INFO line
/// when nothing matches. It must not terminate anything: the previous
/// implementation ran `taskkill /F` and inferred liveness from the exit
/// status, so asking whether the tunnel was alive killed it.
#[cfg(not(unix))]
fn pid_exists(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|out| {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .any(|line| line.split_whitespace().nth(1) == Some(&pid.to_string()))
        })
        .unwrap_or(false)
}

/// Whether it is safe to signal the recorded tunnel process.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StopDecision {
    /// The recorded process was verified (or proven alive) — signal it.
    Signal,
    /// Do not signal; the reason is reported to the operator.
    ///
    /// `clear_state` distinguishes *proven* staleness from a mere failure to
    /// verify: only the former justifies discarding the state file.
    Skip { reason: String, clear_state: bool },
}

/// Decide whether signalling the recorded tunnel process is authorized.
///
/// State that records an OS identity (the native daemon) goes through the
/// two-tier check in [`daemon_lifecycle`]: a process that answers the nonce
/// challenge, or that is unresponsive but still matches all three recorded
/// attributes, may be signalled. A stale or unverifiable one may not.
///
/// OpenSSH state records **no** identity, so it keeps the pre-existing
/// `verify_ssh_pid` behaviour byte-for-byte — including the non-Unix fallback
/// that trusts the PID. Closing that gap needs a Windows identity
/// implementation (see the design's "Stop and crash recovery") and is not
/// silently folded into this change.
fn stop_decision(state: Option<&TunnelState>, pid: u32) -> StopDecision {
    if let Some(s) = state {
        if daemon_lifecycle::recorded_identity(s).is_some() {
            // Tier 1 needs the IPC client that ships with the native daemon.
            // Until it lands, `challenge` answers false and every recorded
            // daemon falls through to the Tier 2 identity check — which is the
            // conservative direction: it can only ever deny, never widen.
            return verdict_to_decision(daemon_lifecycle::assess(s, |_nonce| false), pid);
        }
    }
    openssh_stop_decision(pid)
}

/// Translate a two-tier verdict into what `stop` should do.
///
/// Split out from [`stop_decision`] so the mapping is testable on every
/// platform: reaching `Unverifiable` through a real pid is not deterministic
/// (on macOS an absent pid is provably gone, i.e. `Stale`).
fn verdict_to_decision(verdict: Verdict, pid: u32) -> StopDecision {
    match verdict {
        Verdict::Alive | Verdict::UnresponsiveButIdentified => StopDecision::Signal,
        // Proven gone (or the pid now belongs to something else): the state
        // file is stale and may be discarded.
        Verdict::Stale => StopDecision::Skip {
            reason: format!("recorded daemon (pid {pid}) is no longer running"),
            clear_state: true,
        },
        // Not proof of absence — only of our inability to check. The design
        // requires evidence before discarding state, so the file is left for
        // the operator.
        Verdict::Unverifiable(reason) => StopDecision::Skip {
            reason: format!("cannot verify recorded daemon (pid {pid}): {reason}"),
            clear_state: false,
        },
    }
}

/// Every platform with a verification mechanism verifies.
///
/// This used to trust the PID unconditionally on non-Unix; Windows now has an
/// identity implementation, so it verifies like the rest.
#[cfg(any(unix, target_os = "windows"))]
fn openssh_stop_decision(pid: u32) -> StopDecision {
    if verify_ssh_pid(pid) {
        StopDecision::Signal
    } else {
        // A pid that is not ssh means the recorded tunnel is not there, so the
        // state file is stale and may still be cleared.
        StopDecision::Skip {
            reason: format!("PID {pid} is not an SSH process"),
            clear_state: true,
        }
    }
}

/// Targets with no identity mechanism at all: unchanged, the PID is trusted.
#[cfg(not(any(unix, target_os = "windows")))]
fn openssh_stop_decision(pid: u32) -> StopDecision {
    let _ = pid;
    StopDecision::Signal
}

pub struct SSHClient {
    /// The owned transport. `Arc` so that call sites which have migrated to
    /// the contract and call sites which still need runner-specific behaviour
    /// reach the *same* backend state rather than two divergent copies —
    /// `SSHRunner::clone` snapshots its ControlMaster flag instead of sharing
    /// it, so handing out a second runner would silently break the fallback.
    transport: Arc<OpenSshTransport>,
    pub port: u16,
    pub keep_remote_files: bool,
    pub profile: Option<String>,
    tunnel_pid: Option<u32>,
}

impl SSHClient {
    /// Borrow the underlying OpenSSH runner.
    ///
    /// Provided for behaviour the contract deliberately does not model:
    /// host-key hint strings, ControlMaster bookkeeping, and remote
    /// environment probing. New code should prefer [`Self::transport`].
    pub fn runner(&self) -> &SSHRunner {
        self.transport.runner()
    }

    /// The transport contract, for call sites that have migrated.
    pub fn transport(&self) -> Arc<dyn RemoteTransport> {
        self.transport.clone()
    }

    pub fn from_env(keep_remote_files: bool) -> Result<Self> {
        let cfg = Config::from_env()?;
        let mut runner = SSHRunner::new(cfg.remote_host.as_deref().unwrap_or(""));
        if let Some(ref user) = cfg.remote_user {
            runner = runner.with_user(user);
        }
        if let Some(ref jump) = cfg.jump_host {
            let mut r = runner.with_jump(jump);
            if let Some(ref user) = cfg.jump_user {
                r.jump_user = Some(user.clone());
            }
            runner = r;
        }
        runner.ssh_port = cfg.ssh_port;
        runner.ssh_key_path = cfg.ssh_key.clone();
        runner.ssh_config_path = cfg.ssh_config.clone();
        if cfg.disable_control_master {
            *runner.use_control_master.lock().unwrap() = false;
        }

        Ok(Self {
            transport: Arc::new(OpenSshTransport::new(runner)),
            port: cfg.port,
            keep_remote_files,
            profile: cfg.profile,
            tunnel_pid: None,
        })
    }

    pub fn warm(&mut self, _timeout: Option<u64>) -> Result<()> {
        self.ensure_remote_setup()?;
        self.ensure_tunnel()?;
        self.save_state()?;
        tracing::info!("tunnel established on port {}", self.port);
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        // Loaded once, profile-scoped, so the decision sees the same state the
        // rest of this client uses.
        let state = TunnelState::load_with_profile(self.profile.as_deref())
            .ok()
            .flatten();
        let mut may_clear_state = true;
        if let Some(pid) = self.tunnel_pid {
            match stop_decision(state.as_ref(), pid) {
                StopDecision::Signal => {
                    #[cfg(unix)]
                    {
                        let _ = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = Command::new("taskkill")
                            .args(["/PID", &pid.to_string(), "/F"])
                            .output();
                    }
                    tracing::info!("killed tunnel process {}", pid);
                }
                StopDecision::Skip {
                    reason,
                    clear_state,
                } => {
                    tracing::warn!("{reason}; skipping kill");
                    may_clear_state = clear_state;
                }
            }
        }

        if !may_clear_state {
            // Nothing was proven, so nothing is destroyed. Clearing the state
            // file here would discard the only record of a daemon we could not
            // identify, and wiping the remote scratch dir could break one that
            // is still running.
            tracing::warn!(
                "leaving tunnel state and remote files in place; \
                 remove the state file manually once the daemon is known to be gone"
            );
            return Ok(());
        }

        if !self.keep_remote_files {
            self.cleanup_remote()?;
        }

        // Same profile scope as the load above, so state written for a profile
        // is cleared for that profile rather than for whatever VB_PROFILE says.
        TunnelState::clear_with_profile(self.profile.as_deref()).ok();
        Ok(())
    }

    pub fn saved_port(&self) -> Option<u16> {
        TunnelState::load().ok().flatten().map(|s| s.port)
    }

    pub fn is_tunnel_alive(&self) -> bool {
        if let Some(pid) = self.tunnel_pid {
            #[cfg(unix)]
            {
                verify_ssh_pid(pid) && unsafe { libc::kill(pid as i32, 0) == 0 }
            }
            #[cfg(not(unix))]
            {
                // Was `taskkill /F`, which terminated the tunnel as a side
                // effect of asking whether it was alive, and then reported the
                // inverse of the truth. Probing must not kill.
                verify_ssh_pid(pid) && pid_exists(pid)
            }
        } else {
            false
        }
    }

    pub fn upload_file(&self, local: &str, remote: &str) -> Result<()> {
        self.runner().upload(local, remote)
    }

    pub fn download_file(&self, remote: &str, local: &str) -> Result<()> {
        self.runner().download(remote, local)
    }

    pub fn upload_text(&self, text: &str, remote: &str) -> Result<()> {
        self.runner().upload_text(text, remote)
    }

    pub fn run_command(&self, cmd: &str) -> Result<crate::models::RemoteTaskResult> {
        self.runner().run_command(cmd, None)
    }

    fn ensure_remote_setup(&self) -> Result<String> {
        let python = self.runner().detect_python()?;

        let setup_dir = setup_dir_for_profile(self.profile.as_deref());
        self.runner()
            .run_command(&format!("mkdir -p {setup_dir}"), None)?;

        let daemon_path = if let Some(ref py) = python {
            if py.contains("2.7") {
                self.deploy_daemon_27(&setup_dir)?
            } else {
                self.deploy_daemon_3(&setup_dir)?
            }
        } else {
            self.deploy_rust_daemon(&setup_dir)?
        };

        let il_path = self.deploy_il_script(&setup_dir, &daemon_path, python.as_deref())?;

        tracing::info!(
            "remote setup complete: profile={:?} daemon={} il={}",
            self.profile,
            daemon_path,
            il_path
        );
        Ok(il_path)
    }

    fn ensure_tunnel(&mut self) -> Result<()> {
        for port in self.port..(self.port + 10) {
            if self.try_ssh_tunnel(port).is_ok() {
                self.port = port;
                return Ok(());
            }
        }
        Err(VirtuosoError::Ssh(format!(
            "failed to establish tunnel on any port; verify SSH: `{}`",
            self.runner().verify_cmd_hint()
        )))
    }

    fn try_ssh_tunnel(&mut self, port: u16) -> Result<()> {
        let target = self.runner().remote_target();
        let mut cmd = Command::new("ssh");
        cmd.args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "ServerAliveCountMax=3",
            "-f",
            "-N",
            "-L",
            &format!("127.0.0.1:{port}:127.0.0.1:{port}"),
        ]);

        // Conditionally add ControlMaster options — disabled when CM has been
        // found to fail at runtime (WSL2/Windows named pipe issues).
        if *self.runner().use_control_master.lock().unwrap() {
            let control_dir = crate::runtime_paths::cache_subdir(&["ssh"]);
            let _ = std::fs::create_dir_all(&control_dir);
            let control_path = control_dir.join("%h-%p-%r");
            cmd.args([
                "-o",
                "ControlMaster=auto",
                "-o",
                &format!("ControlPath={}", control_path.display()),
                "-o",
                "ControlPersist=600",
            ]);
        }

        if let Some(p) = self.runner().ssh_port {
            cmd.arg("-p").arg(p.to_string());
        }
        if let Some(ref key) = self.runner().ssh_key_path {
            cmd.arg("-i").arg(key);
        }
        if let Some(ref config) = self.runner().ssh_config_path {
            cmd.arg("-F").arg(config);
        }
        if let Some(ref jump) = self.runner().jump_host {
            cmd.arg("-J").arg(jump);
        }
        cmd.arg(&target);

        let output = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| VirtuosoError::Ssh(format!("failed to start tunnel: {e}")))?;

        let pid = output.id();
        self.tunnel_pid = Some(pid);

        use std::net::TcpStream;
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
                return Ok(());
            }
        }
        Err(VirtuosoError::Ssh("tunnel port not reachable".into()))
    }

    fn save_state(&self) -> Result<()> {
        let state = TunnelState {
            version: crate::models::CURRENT_STATE_VERSION,
            port: self.port,
            pid: self.tunnel_pid.unwrap_or(0),
            remote_host: self.runner().host.clone(),
            setup_path: Some(setup_dir_for_profile(self.profile.as_deref())),
            // v2 fields: the OpenSSH backend records its backend + profile; the
            // remaining v2 fields (daemon nonce, IPC endpoint, executable path,
            // start identity, …) are populated by the native daemon when it ships.
            profile: self.profile.clone(),
            backend: Some("openssh".to_string()),
            daemon_nonce: None,
            executable_path: None,
            start_identity: None,
            ipc_endpoint: None,
            token_path: None,
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
        };
        state.save().map_err(|e| VirtuosoError::Ssh(e.to_string()))
    }

    fn deploy_daemon_3(&self, setup_dir: &str) -> Result<String> {
        let path = format!("{setup_dir}/ramic_bridge_daemon_3.py");
        let content = RESOURCES
            .get_file("daemons/ramic_bridge_daemon_3.py")
            .and_then(|f| f.contents_utf8())
            .ok_or_else(|| {
                VirtuosoError::Ssh("ramic_bridge_daemon_3.py not found in resources".into())
            })?;

        self.runner().upload_text(content, &path)?;
        Ok(path)
    }

    fn deploy_daemon_27(&self, setup_dir: &str) -> Result<String> {
        let path = format!("{setup_dir}/ramic_bridge_daemon_27.py");
        let content = RESOURCES
            .get_file("daemons/ramic_bridge_daemon_27.py")
            .and_then(|f| f.contents_utf8())
            .ok_or_else(|| {
                VirtuosoError::Ssh("ramic_bridge_daemon_27.py not found in resources".into())
            })?;

        self.runner().upload_text(content, &path)?;
        Ok(path)
    }

    fn deploy_rust_daemon(&self, setup_dir: &str) -> Result<String> {
        let arch = self.runner().detect_arch()?;
        let binary_name = match arch.as_str() {
            "x86_64" => "virtuoso-daemon-x86_64",
            "aarch64" => "virtuoso-daemon-aarch64",
            _ => {
                return Err(VirtuosoError::Ssh(format!(
                    "unsupported architecture: {arch}"
                )))
            }
        };

        let path = format!("{setup_dir}/{binary_name}");

        let embedded = RESOURCES
            .get_file(format!("daemons/{binary_name}"))
            .ok_or_else(|| {
                VirtuosoError::Ssh(format!("{binary_name} not found in resources, build with: cargo build --features daemon --release && cp target/release/virtuoso-daemon resources/daemons/{binary_name}"))
            })?;

        let content = embedded.contents();
        let tmp = tempfile::NamedTempFile::new()
            .map_err(|e| VirtuosoError::Ssh(format!("temp file failed: {e}")))?;
        tmp.as_file()
            .write_all(content)
            .map_err(|e| VirtuosoError::Ssh(format!("write temp failed: {e}")))?;

        self.runner().upload(tmp.path().to_str().unwrap(), &path)?;
        self.runner()
            .run_command(&format!("chmod +x {path}"), None)?;

        Ok(path)
    }

    fn deploy_il_script(
        &self,
        setup_dir: &str,
        daemon_path: &str,
        python: Option<&str>,
    ) -> Result<String> {
        let il_content = RESOURCES
            .get_file("ramic_bridge.il")
            .and_then(|f| f.contents_utf8())
            .ok_or_else(|| VirtuosoError::Ssh("ramic_bridge.il not found in resources".into()))?;

        let il_content = il_content
            .replace("__DAEMON_PATH__", daemon_path)
            .replace("__PYTHON_CMD__", python.unwrap_or(""));

        let path = format!("{setup_dir}/ramic_bridge.il");
        self.runner().upload_text(&il_content, &path)?;
        Ok(path)
    }

    fn cleanup_remote(&self) -> Result<()> {
        // Cleanup is scoped to the active profile's setup dir so that
        // stopping profile A does not wipe profile B's bridge files.
        // This was the bug upstream PR #86 fixed in the Python bridge
        // and that we mirror here.
        let setup_dir = setup_dir_for_profile(self.profile.as_deref());
        self.runner()
            .run_command(&format!("rm -rf {setup_dir}"), None)?;
        Ok(())
    }
}

pub fn file_md5(path: &str) -> Result<String> {
    let content =
        fs::read(path).map_err(|e| VirtuosoError::Config(format!("failed to read file: {e}")))?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    //! Unit tests for profile-isolated setup-dir helpers.
    //!
    //! These are the same invariants that upstream PR #86 enforces
    //! in the Python bridge; we mirror them in Rust so a future refactor
    //! can't silently regress the multi-profile safety property.

    #[cfg(target_os = "macos")]
    use super::is_ssh_executable;
    use super::{
        daemon_lifecycle, profiled_bridge_leaf, profiled_env_key, setup_dir_for_profile,
        stop_decision, verdict_to_decision, verify_ssh_pid, StopDecision, TunnelState, Verdict,
    };

    /// A state carrying no OS identity — what OpenSSH writes today, and what a
    /// v1 file deserializes to.
    fn openssh_like_state(pid: u32) -> TunnelState {
        TunnelState {
            version: 2,
            port: 40567,
            pid,
            remote_host: "compute-eda-42".into(),
            setup_path: None,
            profile: None,
            backend: Some("openssh".into()),
            daemon_nonce: None,
            executable_path: None,
            start_identity: None,
            ipc_endpoint: None,
            token_path: None,
            local_forward: None,
            start_time_unix_ms: None,
            health: None,
            config_digest: None,
        }
    }

    /// The safety property: without a recorded identity the old OpenSSH
    /// behaviour must be preserved exactly, so this change is a no-op for
    /// every tunnel that exists today.
    #[test]
    fn state_without_identity_does_not_take_the_native_path() {
        // A pid that is certainly not an ssh process: the decision must be the
        // OpenSSH verdict (Skip wherever a mechanism exists), never a native
        // one derived from identity.
        let state = openssh_like_state(999_999);
        let d = stop_decision(Some(&state), 999_999);
        // Platforms with a verification mechanism refuse; unverifiable targets
        // still trust the pid, which is the documented remaining gap.
        #[cfg(any(unix, target_os = "windows"))]
        assert_eq!(
            d,
            StopDecision::Skip {
                reason: "PID 999999 is not an SSH process".into(),
                clear_state: true,
            }
        );
        #[cfg(not(any(unix, target_os = "windows")))]
        assert_eq!(d, StopDecision::Signal);
    }

    /// No state at all falls back to the OpenSSH check rather than failing open.
    #[test]
    fn missing_state_falls_back_to_openssh_decision() {
        let d = stop_decision(None, 999_999);
        #[cfg(any(unix, target_os = "windows"))]
        assert_eq!(
            d,
            StopDecision::Skip {
                reason: "PID 999999 is not an SSH process".into(),
                clear_state: true,
            }
        );
        #[cfg(not(any(unix, target_os = "windows")))]
        assert_eq!(d, StopDecision::Signal);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn recorded_identity_matching_a_live_process_authorizes_the_signal() {
        let me = crate::transport::identity::ProcessIdentity::current().unwrap();
        let mut state = openssh_like_state(me.pid);
        daemon_lifecycle::record_identity(&mut state, &me);
        // Tier 1 is not wired yet (challenge answers false), so this exercises
        // Tier 2: unresponsive but identified → still safe to signal.
        assert_eq!(stop_decision(Some(&state), me.pid), StopDecision::Signal);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn recorded_identity_for_a_dead_process_is_skipped() {
        let me = crate::transport::identity::ProcessIdentity::current().unwrap();
        let mut state = openssh_like_state(me.pid);
        daemon_lifecycle::record_identity(&mut state, &me);
        state.pid = 999_999; // nothing is running here
        match stop_decision(Some(&state), 999_999) {
            StopDecision::Skip {
                reason,
                clear_state,
            } => {
                assert!(
                    reason.contains("no longer running"),
                    "unexpected reason: {reason}"
                );
                // Proven gone, so the stale state file may be discarded.
                assert!(clear_state);
            }
            other => panic!("must refuse to signal a dead pid, got {other:?}"),
        }
    }

    /// The "cleanup requires proof" half of the design. Exercised through
    /// `verdict_to_decision` because reaching `Unverifiable` via a real pid is
    /// not deterministic: on macOS an absent pid is provably gone (`Stale`).
    #[test]
    fn unverifiable_identity_keeps_the_state_file() {
        match verdict_to_decision(Verdict::Unverifiable("no mechanism".into()), 42) {
            StopDecision::Skip {
                clear_state: false,
                reason,
            } => assert!(reason.contains("no mechanism"), "reason: {reason}"),
            other => panic!("unverifiable must not clear state, got {other:?}"),
        }
    }

    /// Every verdict maps to exactly one decision, and only provable staleness
    /// permits discarding the state file.
    #[test]
    fn verdict_mapping_is_total_and_conservative() {
        assert_eq!(
            verdict_to_decision(Verdict::Alive, 42),
            StopDecision::Signal
        );
        assert_eq!(
            verdict_to_decision(Verdict::UnresponsiveButIdentified, 42),
            StopDecision::Signal
        );
        assert_eq!(
            verdict_to_decision(Verdict::Stale, 42),
            StopDecision::Skip {
                reason: "recorded daemon (pid 42) is no longer running".into(),
                clear_state: true,
            }
        );
        assert_eq!(
            verdict_to_decision(Verdict::Unverifiable("x".into()), 42),
            StopDecision::Skip {
                reason: "cannot verify recorded daemon (pid 42): x".into(),
                clear_state: false,
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ssh_executable_detection() {
        use std::path::Path;
        assert!(is_ssh_executable(Path::new("/usr/bin/ssh")));
        assert!(is_ssh_executable(Path::new("/usr/bin/sshd")));
        assert!(!is_ssh_executable(Path::new("/usr/bin/python3")));
        assert!(!is_ssh_executable(Path::new("/")));
    }

    /// The regression this branch exists for: on macOS there is no /proc, so a
    /// `/proc`-only check verified nothing and `tunnel stop` could never kill
    /// the tunnel. Spawn a real process and verify it.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_verifies_a_real_process_by_its_executable() {
        // A copy of `sleep` whose *name* contains "ssh" stands in for the ssh
        // client, so the check is exercised without opening a connection.
        let dir = tempfile::tempdir().unwrap();
        let ssh_like = dir.path().join("ssh-standin");
        std::fs::copy("/bin/sleep", &ssh_like).unwrap();

        let mut child = std::process::Command::new(&ssh_like)
            .arg("10")
            .spawn()
            .expect("spawn stand-in");
        let pid = child.id();
        assert!(
            verify_ssh_pid(pid),
            "a process whose binary is named ssh-* must verify on macOS"
        );
        reap(&mut child);

        // Control: the real /bin/sleep is not ssh.
        let mut other = std::process::Command::new("/bin/sleep")
            .arg("10")
            .spawn()
            .expect("spawn control");
        assert!(
            !verify_ssh_pid(other.id()),
            "/bin/sleep must not verify as ssh"
        );
        reap(&mut other);
    }

    /// Kill and reap a spawned stand-in so the test leaves no zombies.
    #[cfg(target_os = "macos")]
    fn reap(child: &mut std::process::Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A dead pid must verify as false rather than panicking or trusting the pid.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn dead_pid_does_not_verify_as_ssh() {
        assert!(!verify_ssh_pid(999_999));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn pid_reuse_is_skipped_even_though_the_pid_is_live() {
        // The case the whole two-tier design exists for: the pid is in use, but
        // by a different process than the one recorded.
        let me = crate::transport::identity::ProcessIdentity::current().unwrap();
        let mut state = openssh_like_state(me.pid);
        daemon_lifecycle::record_identity(&mut state, &me);
        state.start_identity = Some(me.start_identity.wrapping_add(1));
        match stop_decision(Some(&state), me.pid) {
            StopDecision::Skip { clear_state, .. } => {
                // The pid now belongs to a different process, so the recorded
                // daemon is proven gone and its state is stale.
                assert!(clear_state);
            }
            other => panic!("must refuse to signal a reused pid, got {other:?}"),
        }
    }

    #[test]
    fn bridge_leaf_no_profile() {
        assert_eq!(profiled_bridge_leaf(None), "virtuoso_bridge");
        assert_eq!(setup_dir_for_profile(None), "/tmp/virtuoso_bridge");
    }

    #[test]
    fn bridge_leaf_simple_profile() {
        assert_eq!(
            profiled_bridge_leaf(Some("analog")),
            "virtuoso_bridge_analog"
        );
        assert_eq!(
            setup_dir_for_profile(Some("analog")),
            "/tmp/virtuoso_bridge_analog"
        );
    }

    #[test]
    fn bridge_leaf_digits_and_punctuation() {
        // Digits, dots, underscores, hyphens pass through unchanged.
        assert_eq!(
            profiled_bridge_leaf(Some("t28_digital_v1.2")),
            "virtuoso_bridge_t28_digital_v1.2"
        );
    }

    #[test]
    fn bridge_leaf_sanitizes_special_chars() {
        // Slashes, spaces, exclamation marks, etc. become underscores.
        // The CRITICAL property: no path traversal can land us in a
        // parent of /tmp/ — the sanitization replaces `/` with `_`.
        assert_eq!(
            profiled_bridge_leaf(Some("../etc/passwd")),
            "virtuoso_bridge_.._etc_passwd"
        );
        assert_eq!(
            profiled_bridge_leaf(Some("weird/chars!@#")),
            "virtuoso_bridge_weird_chars___"
        );
    }

    #[test]
    fn bridge_leaf_length_capped() {
        // 64-char limit prevents runaway profile names from making
        // an arbitrarily long path that could exceed shell ARG_MAX.
        let long: String = "a".repeat(200);
        let leaf = profiled_bridge_leaf(Some(&long));
        assert!(leaf.len() <= 64 + "virtuoso_bridge_".len());
    }

    #[test]
    fn bridge_leaf_all_sanitized_falls_back() {
        // A profile name that sanitizes to empty must NOT produce
        // "virtuoso_bridge_" (which would shadow the no-profile
        // case). It falls back to "virtuoso_bridge_profile".
        let leaf = profiled_bridge_leaf(Some("///"));
        assert_eq!(leaf, "virtuoso_bridge_profile");
    }

    #[test]
    fn env_key_no_profile() {
        assert_eq!(profiled_env_key("VB_LOCAL_PORT", None), "VB_LOCAL_PORT");
    }

    #[test]
    fn env_key_with_profile() {
        assert_eq!(
            profiled_env_key("VB_LOCAL_PORT", Some("analog")),
            "VB_LOCAL_PORT_analog"
        );
    }

    #[test]
    fn env_key_preserves_base_name() {
        // The base key is passed through verbatim — we only append
        // a suffix, so callers can use any env-var name.
        assert_eq!(profiled_env_key("ANY_KEY", Some("p1")), "ANY_KEY_p1");
        assert_eq!(profiled_env_key("VB_PORT", Some("a.b.c")), "VB_PORT_a.b.c");
    }

    /// The two profiles produce **non-overlapping** setup dirs and
    /// non-overlapping env keys. This is the property that protects
    /// multi-profile users from cross-contamination.
    #[test]
    fn two_profiles_are_isolated() {
        let a = setup_dir_for_profile(Some("analog"));
        let b = setup_dir_for_profile(Some("digital"));
        assert_ne!(a, b, "profile dirs must differ");
        assert!(
            !a.contains("digital") && !b.contains("analog"),
            "no name leak between profiles"
        );

        let k_a = profiled_env_key("VB_LOCAL_PORT", Some("analog"));
        let k_b = profiled_env_key("VB_LOCAL_PORT", Some("digital"));
        assert_ne!(k_a, k_b);
    }
}
