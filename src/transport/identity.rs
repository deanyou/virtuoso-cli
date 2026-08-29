//! OS-level process identity for daemon stop / crash recovery (Tier 2).
//!
//! A PID alone never identifies a process, because PIDs are reused. Per the
//! design's [Stop and crash recovery] contract, a Tier 2 termination (the daemon
//! is wedged and cannot answer a nonce challenge) is only permitted after an
//! operating-system identity match on **all three** recorded attributes:
//! executable path, PID, and start identity.
//!
//! If any attribute fails to match — or if the identity cannot be established on
//! this platform at all — the caller must refuse to signal and report the stale
//! state instead. A Tier 2 kill guarded by nothing but a PID is never performed
//! on any platform.
//!
//! [Stop and crash recovery]: ../../../docs/superpowers/specs/2026-08-29-native-remote-transport-design.md

// Consumed when the daemon lifecycle lands (step 6); mirrors `contract.rs`.
#![allow(dead_code)]

use std::path::PathBuf;

/// Why an identity could not be established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// The process is not running (or is not ours to inspect).
    NoSuchProcess(u32),
    /// This platform has no supported mechanism yet. Callers must refuse, never
    /// fall back to trusting the PID.
    UnsupportedPlatform(&'static str),
    /// The mechanism exists but the read failed.
    Unreadable(String),
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentityError::NoSuchProcess(pid) => write!(f, "no such process: {pid}"),
            IdentityError::UnsupportedPlatform(p) => {
                write!(f, "process identity unsupported on {p}")
            }
            IdentityError::Unreadable(m) => write!(f, "cannot read process identity: {m}"),
        }
    }
}

impl std::error::Error for IdentityError {}

/// The three attributes that together identify one process instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub executable_path: PathBuf,
    pub pid: u32,
    /// Platform-specific start marker: the moment the process was created.
    /// Detects PID reuse — a recycled PID has a different start identity.
    ///
    /// - Linux: `/proc/<pid>/stat` field 22 (`starttime`, clock ticks since boot)
    /// - macOS: `kinfo_proc.kp_proc.p_starttime.tv_sec` (Unix epoch seconds)
    /// - Windows: process creation time
    pub start_identity: u64,
}

impl ProcessIdentity {
    /// Identity of the current process.
    pub fn current() -> Result<Self, IdentityError> {
        Self::of_pid(std::process::id())
    }

    /// Identity of a live process by PID.
    pub fn of_pid(pid: u32) -> Result<Self, IdentityError> {
        imp::of_pid(pid)
    }

    /// Tier 2 match: all three attributes must agree.
    ///
    /// A zero start identity or an empty executable path never matches — an
    /// unreadable attribute must fail closed rather than compare as "equal to
    /// another empty value".
    pub fn matches(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.start_identity != 0
            && self.start_identity == other.start_identity
            && !self.executable_path.as_os_str().is_empty()
            && self.executable_path == other.executable_path
    }
}

/// Why a Tier 2 signal was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Identity could not be established — refuse rather than signal blindly.
    Unverifiable(IdentityError),
    /// The live process is not the recorded one (typically PID reuse).
    Mismatch {
        recorded: Box<ProcessIdentity>,
        live: Box<ProcessIdentity>,
    },
    /// Nothing is running under that PID; the state file is simply stale.
    ProcessGone(u32),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unverifiable(e) => write!(f, "cannot verify daemon identity: {e}"),
            Refusal::Mismatch { recorded, live } => write!(
                f,
                "recorded daemon does not match live process (recorded pid {} start {}, live pid {} start {})",
                recorded.pid, recorded.start_identity, live.pid, live.start_identity
            ),
            Refusal::ProcessGone(pid) => write!(f, "no process with pid {pid}; state is stale"),
        }
    }
}

/// Authorize a Tier 2 signal against the *live* process.
///
/// Returns `Ok(())` only when all three recorded attributes match a running
/// process. Every other outcome is a [`Refusal`] the caller must surface — this
/// is the guard that replaces "trust the PID".
pub fn authorize_signal(recorded: &ProcessIdentity) -> Result<(), Refusal> {
    match ProcessIdentity::of_pid(recorded.pid) {
        Ok(live) if live.matches(recorded) => Ok(()),
        Ok(live) => Err(Refusal::Mismatch {
            recorded: Box::new(recorded.clone()),
            live: Box::new(live),
        }),
        Err(IdentityError::NoSuchProcess(pid)) => Err(Refusal::ProcessGone(pid)),
        Err(e) => Err(Refusal::Unverifiable(e)),
    }
}

// ---------------------------------------------------------------------------
// Platform implementations
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    pub(super) fn of_pid(pid: u32) -> Result<ProcessIdentity, IdentityError> {
        let stat_path = format!("/proc/{pid}/stat");
        let stat = std::fs::read_to_string(&stat_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                IdentityError::NoSuchProcess(pid)
            } else {
                IdentityError::Unreadable(format!("{stat_path}: {e}"))
            }
        })?;
        // `comm` is parenthesised and may contain spaces and parens, so split
        // on the *last* ')' and index from there.
        let after_comm = stat.rsplit_once(')').map(|(_, rest)| rest).unwrap_or(&stat);
        let fields: Vec<&str> = after_comm.split_whitespace().collect();
        // Field 3 is `state`, so field 22 (`starttime`) is index 19.
        let start_identity = fields
            .get(19)
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| IdentityError::Unreadable(format!("{stat_path}: no starttime field")))?;
        let executable_path = std::fs::read_link(format!("/proc/{pid}/exe"))
            .map_err(|e| IdentityError::Unreadable(format!("/proc/{pid}/exe: {e}")))?;
        Ok(ProcessIdentity {
            executable_path,
            pid,
            start_identity,
        })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    const CTL_KERN: libc::c_int = 1;
    const KERN_PROC: libc::c_int = 14;
    const KERN_PROC_PID: libc::c_int = 1;
    const KERN_PROCARGS2: libc::c_int = 49;

    fn sysctl_bytes(mib: &mut [libc::c_int], buf: &mut [u8]) -> Result<usize, IdentityError> {
        let mut len = buf.len();
        // SAFETY: `buf` is a mutable byte slice we own; `mib` is a valid,
        // non-empty int array. `sysctl` writes at most `len` bytes.
        let ret = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as libc::c_uint,
                buf.as_mut_ptr() as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            return Err(IdentityError::Unreadable(format!("sysctl: {err}")));
        }
        Ok(len)
    }

    /// `kinfo_proc.kp_proc.p_starttime` — the first member of `extern_proc` is
    /// the `p_un` union whose `__p_starttime` is a `struct timeval`, so the
    /// start time sits at offset 0 of `kinfo_proc`. Reading the leading
    /// `timeval` avoids reproducing the whole (architecture-dependent) struct.
    fn start_time(pid: u32) -> Result<u64, IdentityError> {
        let mut mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid as libc::c_int];
        let mut buf = [0u8; 1024];
        let len = sysctl_bytes(&mut mib, &mut buf).map_err(|e| match e {
            IdentityError::Unreadable(ref m) if m.contains("ESRCH") => {
                IdentityError::NoSuchProcess(pid)
            }
            other => other,
        })?;
        if len < 16 {
            return Err(IdentityError::Unreadable(
                "sysctl KERN_PROC_PID returned a short buffer".into(),
            ));
        }
        let secs = i64::from_ne_bytes(buf[0..8].try_into().unwrap());
        Ok(secs as u64)
    }

    /// Executable path via `KERN_PROCARGS2`: an `int argc` followed by
    /// NUL-terminated strings, the first of which is the exec path.
    fn exe_path(pid: u32) -> Result<PathBuf, IdentityError> {
        // KERN_PROCARGS2 is a top-level KERN_* entry, not a KERN_PROC
        // subcommand: three elements, not four.
        let mut mib = [CTL_KERN, KERN_PROCARGS2, pid as libc::c_int];
        let mut buf = vec![0u8; 64 * 1024];
        let len = sysctl_bytes(&mut mib, &mut buf).map_err(|e| match e {
            IdentityError::Unreadable(ref m) if m.contains("ESRCH") => {
                IdentityError::NoSuchProcess(pid)
            }
            other => other,
        })?;
        if len <= 4 {
            return Err(IdentityError::Unreadable(
                "sysctl KERN_PROCARGS2 returned a short buffer".into(),
            ));
        }
        let strings = &buf[4..len];
        let end = strings
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(strings.len());
        let path = String::from_utf8_lossy(&strings[..end]).to_string();
        if path.is_empty() {
            return Err(IdentityError::Unreadable(
                "sysctl KERN_PROCARGS2 gave an empty exec path".into(),
            ));
        }
        Ok(PathBuf::from(path))
    }

    pub(super) fn of_pid(pid: u32) -> Result<ProcessIdentity, IdentityError> {
        // Both attributes are mandatory: a partially-read identity must fail
        // closed rather than compare as "empty equals empty".
        let start_identity = start_time(pid)?;
        let executable_path = exe_path(pid)?;
        Ok(ProcessIdentity {
            executable_path,
            pid,
            start_identity,
        })
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;

    pub(super) fn of_pid(pid: u32) -> Result<ProcessIdentity, IdentityError> {
        // Not implemented yet (`OpenProcess` / `GetModuleFileNameEx` /
        // `GetProcessTimes`). Refusing is deliberate: the current OpenSSH path
        // issues `taskkill /F` unconditionally on Windows, and the design calls
        // closing that gap part of this work. Failing closed is the safe state.
        Err(IdentityError::UnsupportedPlatform("windows"))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod imp {
    use super::*;

    pub(super) fn of_pid(_pid: u32) -> Result<ProcessIdentity, IdentityError> {
        Err(IdentityError::UnsupportedPlatform("this platform"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(pid: u32, path: &str, start: u64) -> ProcessIdentity {
        ProcessIdentity {
            executable_path: PathBuf::from(path),
            pid,
            start_identity: start,
        }
    }

    #[test]
    fn identical_identities_match() {
        let a = identity(42, "/usr/bin/vcli", 1000);
        let b = identity(42, "/usr/bin/vcli", 1000);
        assert!(a.matches(&b));
    }

    #[test]
    fn pid_reuse_is_detected_by_start_identity() {
        // Same PID, different process: the whole reason PID alone is rejected.
        let recorded = identity(42, "/usr/bin/vcli", 1000);
        let reused = identity(42, "/usr/bin/vcli", 9999);
        assert!(!recorded.matches(&reused));
    }

    #[test]
    fn different_executable_is_detected_even_with_same_pid_and_start() {
        let recorded = identity(42, "/usr/bin/vcli", 1000);
        let other = identity(42, "/tmp/evil", 1000);
        assert!(!recorded.matches(&other));
    }

    #[test]
    fn zero_start_identity_never_matches() {
        let a = identity(42, "/usr/bin/vcli", 0);
        let b = identity(42, "/usr/bin/vcli", 0);
        assert!(!a.matches(&b), "an unreadable start time must fail closed");
    }

    #[test]
    fn empty_path_never_matches() {
        let a = identity(42, "", 1000);
        let b = identity(42, "", 1000);
        assert!(!a.matches(&b), "an unreadable path must fail closed");
    }

    #[test]
    fn different_pid_never_matches() {
        let a = identity(42, "/usr/bin/vcli", 1000);
        let b = identity(43, "/usr/bin/vcli", 1000);
        assert!(!a.matches(&b));
    }

    #[test]
    fn unknown_platform_is_unverifiable_not_silent_success() {
        // The property that closes the Windows `taskkill /F` gap: where the
        // identity cannot be read, the answer is a refusal.
        let recorded = identity(999_999, "/usr/bin/vcli", 1);
        let result = authorize_signal(&recorded);
        assert!(matches!(
            result,
            Err(Refusal::Unverifiable(_)) | Err(Refusal::ProcessGone(_))
        ));
    }

    // --- Platform live checks (only where a mechanism exists) ---

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn current_process_identity_is_readable_and_matches_itself() {
        let me = ProcessIdentity::current().expect("identity of the running test process");
        assert_eq!(me.pid, std::process::id());
        assert_ne!(me.start_identity, 0, "start identity must be meaningful");
        assert!(
            !me.executable_path.as_os_str().is_empty(),
            "executable path must be resolved"
        );
        // The same PID looked up again yields the same identity.
        let again = ProcessIdentity::of_pid(me.pid).expect("re-read own identity");
        assert!(me.matches(&again));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn authorize_signal_succeeds_for_the_live_process() {
        let me = ProcessIdentity::current().unwrap();
        authorize_signal(&me).expect("our own live process must authorize");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_start_time_is_plausible_and_agrees_with_ps_when_available() {
        let me = ProcessIdentity::current().unwrap();

        // Always true: the parsed start time must be in the past and, for a
        // test process, within the last day. This alone catches an
        // endianness or offset mistake in the kinfo_proc read.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(me.start_identity <= now, "start time must be in the past");
        assert!(
            now.saturating_sub(me.start_identity) < 86_400,
            "start time {} should be recent (now {now})",
            me.start_identity
        );

        // Cross-check against `ps`, which reads the same value through
        // libproc. Sandboxed runners may forbid spawning it, in which case the
        // plausibility checks above are the assertion.
        let Ok(ps) = std::process::Command::new("ps")
            .args(["-o", "lstart=", "-p", &me.pid.to_string()])
            .output()
        else {
            return;
        };
        if !ps.status.success() {
            return;
        }
        let lstart = String::from_utf8_lossy(&ps.stdout).trim().to_string();
        if lstart.is_empty() {
            return;
        }
        // ps prints e.g. "Sun Aug 30 05:20:00 2026" — the trailing year.
        let ps_year = lstart.split_whitespace().last().unwrap_or("");
        let ours = chrono::DateTime::<chrono::Utc>::from_timestamp(me.start_identity as i64, 0)
            .expect("start time must be a valid timestamp")
            .format("%Y")
            .to_string();
        assert_eq!(
            ours, ps_year,
            "our start year {ours} vs ps lstart {lstart:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_exe_path_matches_current_exe() {
        let me = ProcessIdentity::current().unwrap();
        let real = std::env::current_exe().unwrap();
        // `KERN_PROCARGS2` may report the path the process was invoked with, so
        // compare the resolved file name rather than the full path.
        assert_eq!(
            me.executable_path.file_name(),
            real.file_name(),
            "procargs path {:?} vs current_exe {:?}",
            me.executable_path,
            real
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_exe_path_is_the_running_binary() {
        let me = ProcessIdentity::current().unwrap();
        assert_eq!(me.executable_path, std::env::current_exe().unwrap());
    }
}
