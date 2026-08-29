//! Test fixture: a single-hop SSH server (real `sshd`) for exercising the
//! native transport end-to-end.
//!
//! Per the design doc, "the fixture lands *before* the code it tests; later
//! steps extend it rather than building it." This module is `#[cfg(test)]` only
//! and is hermetic: it spins up a throwaway `sshd` on `127.0.0.1` with a
//! generated host key, a single client key, and key-only authentication.
//!
//! On platforms where `sshd` cannot be spawned for the test (e.g. macOS, whose
//! system `/usr/sbin/sshd` calls a Seatbelt `sandbox_init` that fails unless
//! launched by launchd), [`SshServerFixture::start`] returns `None` and tests
//! `return` early — so the suite stays green and the fixture activates the
//! moment it runs somewhere sshd works (Linux CI, or a machine with a
//! non-sandboxed openssh).

#![cfg(test)]
#![allow(dead_code)]

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use crate::transport::contract::RemoteTransport;
use crate::transport::ssh::SSHRunner;

const SSHD_BIN: &str = "/usr/sbin/sshd";
const SSH_KEYGEN_BIN: &str = "/usr/bin/ssh-keygen";

/// Render an `sshd_config` for the fixture. Pure: unit-tested without spawning.
fn sshd_config_text(port: u16, host_key: &str, authorized_keys: &str, pid_file: &str) -> String {
    format!(
        "Port {port}\n\
         ListenAddress 127.0.0.1\n\
         HostKey {host_key}\n\
         PidFile {pid_file}\n\
         AuthorizedKeysFile {authorized_keys}\n\
         PubkeyAuthentication yes\n\
         PasswordAuthentication no\n\
         UsePAM no\n\
         StrictModes no\n\
         AllowTcpForwarding yes\n\
         PermitOpen any\n\
         Subsystem sftp internal-sftp\n"
    )
}

/// A running throwaway sshd plus the client material needed to reach it.
pub struct SshServerFixture {
    _temp: tempfile::TempDir,
    port: u16,
    user: String,
    client_key_path: PathBuf,
    child: Child,
}

impl SshServerFixture {
    /// Whether the binaries this fixture needs exist on this machine.
    pub fn available() -> bool {
        std::path::Path::new(SSHD_BIN).exists() && std::path::Path::new(SSH_KEYGEN_BIN).exists()
    }

    /// Start sshd. Returns `None` if sshd is unavailable or a real SSH
    /// connection cannot be established (callers must skip in that case).
    pub fn start() -> Option<Self> {
        if !Self::available() {
            return None;
        }
        let temp = tempfile::tempdir().ok()?;
        let dir = temp.path().to_path_buf();
        let host_key = dir.join("host_key");
        let client_key = dir.join("client_key");
        let authorized = dir.join("authorized_keys");
        let pid_file = dir.join("sshd.pid");

        let gen = |path: &std::path::Path| {
            Command::new(SSH_KEYGEN_BIN)
                .args(["-t", "ed25519", "-f"])
                .arg(path)
                .args(["-N", ""])
                .output()
                .is_ok()
        };
        if !gen(&host_key) || !gen(&client_key) {
            return None;
        }
        // Copy the public key into authorized_keys and lock it down.
        let pub_key = std::fs::read_to_string(client_key.with_extension("pub")).ok()?;
        std::fs::write(&authorized, pub_key).ok()?;
        let _ = Command::new("chmod")
            .args([
                "600",
                &authorized.to_string_lossy(),
                &host_key.to_string_lossy(),
                &client_key.to_string_lossy(),
            ])
            .output();

        let port = free_port()?;
        let config = sshd_config_text(
            port,
            &host_key.to_string_lossy(),
            &authorized.to_string_lossy(),
            &pid_file.to_string_lossy(),
        );
        let config_path = dir.join("sshd_config");
        {
            let mut f = std::fs::File::create(&config_path).ok()?;
            let _ = f.write_all(config.as_bytes());
        }

        let child = Command::new(SSHD_BIN)
            .args(["-f", &config_path.to_string_lossy(), "-D", "-e"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;

        let fixture = SshServerFixture {
            _temp: temp,
            port,
            user: whoami(),
            client_key_path: client_key,
            child,
        };

        // Readiness: a real ssh round-trip, not just a TCP connect, because the
        // server may listen yet reject connections (e.g. sandbox failures).
        if fixture.wait_until_reachable(Duration::from_secs(8)) {
            Some(fixture)
        } else {
            None
        }
    }

    /// Build an `SSHRunner` pointed at this fixture — used to exercise
    /// `OpenSshTransport` (and later the native client) against a live server.
    pub fn runner(&self) -> SSHRunner {
        let mut runner = SSHRunner::new("127.0.0.1");
        runner.user = Some(self.user.clone());
        runner.ssh_port = Some(self.port);
        runner.ssh_key_path = Some(self.client_key_path.to_string_lossy().into_owned());
        *runner.use_control_master.lock().unwrap() = false;
        runner
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    fn wait_until_reachable(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                // TCP is up; confirm a full handshake via the ssh client.
                let status = Command::new("/usr/bin/ssh")
                    .args([
                        "-i",
                        &self.client_key_path.to_string_lossy(),
                        "-p",
                        &self.port.to_string(),
                        "-o",
                        "StrictHostKeyChecking=no",
                        "-o",
                        "UserKnownHostsFile=/dev/null",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ConnectTimeout=3",
                        &format!("{}@127.0.0.1", self.user),
                        "true",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                if matches!(status, Ok(s) if s.success()) {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        false
    }
}

impl Drop for SshServerFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    listener.local_addr().ok().map(|a| a.port())
}

fn whoami() -> String {
    std::env::var("USER")
        .ok()
        .or_else(|| {
            Command::new("/usr/bin/whoami")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "user".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_text_is_well_formed() {
        let text = sshd_config_text(1234, "/k/host", "/k/auth", "/k/pid");
        assert!(text.contains("Port 1234"));
        assert!(text.contains("HostKey /k/host"));
        assert!(text.contains("AuthorizedKeysFile /k/auth"));
        assert!(text.contains("PubkeyAuthentication yes"));
        assert!(text.contains("PasswordAuthentication no"));
        // No `UseSandbox` — it is unsupported on this OpenSSH build and would
        // abort config parsing.
        assert!(!text.contains("UseSandbox"));
    }

    // This test is meaningful where sshd can actually serve (Linux CI). On
    // macOS it returns None and the test is a no-op.
    #[test]
    fn fixture_allows_open_ssh_transport_roundtrip() {
        let Some(fixture) = SshServerFixture::start() else {
            eprintln!("sshd unavailable on this platform; skipping");
            return;
        };
        let transport = crate::transport::openssh::OpenSshTransport::new(fixture.runner());
        let req = crate::transport::contract::CommandRequest::new(
            "echo fixture-ok",
            crate::transport::contract::Deadline::from_now(std::time::Duration::from_secs(30)),
        );
        let result = transport.run_command(&req).expect("command should run");
        assert!(
            result.stdout.contains("fixture-ok"),
            "got: {:?}",
            result.stdout
        );
    }
}
