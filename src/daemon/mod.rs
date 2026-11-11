//! Core of the `virtuoso-daemon` binary.
//!
//! The daemon owns **one evaluation channel** per Virtuoso session: it writes
//! SKILL text to its own stdout (the SKILL side reads that as the IPC
//! process' stdin) and waits for the answer to appear in a callback file pair
//! written by `RBSendCallback`.
//!
//! Two failure modes are handled here, and they are the reason this module
//! lives in the library instead of inside the binary — so both can be
//! unit-tested without a live Virtuoso:
//!
//! * **Timeout** — the evaluation did not come back in time. The channel is
//!   marked *hung*: the daemon stops delivering evaluations (the pipe to
//!   Virtuoso is never written again) and records the stuck call so the
//!   operator can see *what* hung it.
//! * **Hung channel** — every later request is answered immediately with a
//!   clear "restart the session" error instead of being queued behind the
//!   stuck evaluation.
//!
//! The evaluation itself cannot be cancelled: it runs inside the SKILL
//! interpreter's IPC callback, and nothing outside that interpreter can
//! interrupt it. "Cancel" therefore means "stop delivering", and the channel
//! is declared dead so the next request fails fast instead of blocking for
//! another full timeout.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::channel_state::{self, StuckCall};

/// NAK — transport-level failure marker (the answer never arrived).
pub const NAK: u8 = 0x15;

/// RS — trailing record separator written by `RBSendCallback`'s `sprintf %c`.
pub const RS: u8 = 0x1e;

/// Default evaluation deadline when a request does not carry one.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

static CALLS: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<Instant> = OnceLock::new();

/// A request as it arrives over the wire.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SkillRequest {
    /// The SKILL expression to evaluate.
    pub skill: String,
    /// Evaluation deadline in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// The evaluation channel: where the SKILL text goes, and where the answer
/// comes back.
///
/// The trait exists so the timeout/hang state machine can be exercised
/// offline: the production implementation writes to the daemon's stdout and
/// polls the callback files, the test implementation records what was
/// delivered and answers (or does not answer) on demand.
pub trait EvalChannel {
    /// Deliver `skill` to Virtuoso.
    fn submit(&mut self, skill: &str) -> io::Result<()>;

    /// Wait for the answer. `Ok(None)` means the deadline passed with no
    /// answer — the evaluation is stuck.
    fn wait_for_callback(&mut self, timeout_secs: u64) -> io::Result<Option<Vec<u8>>>;

    /// Best effort: did Virtuoso actually *enter* the evaluation?
    ///
    /// `None` means "unknown" — the running `ramic_bridge.il` predates the
    /// in-flight marker, so nothing can be said about it. The daemon only
    /// reports what it can prove.
    fn eval_entered(&self) -> Option<bool> {
        None
    }
}

/// The production channel: the daemon's stdout (Virtuoso's IPC stdin) and the
/// callback file pair written by `RBSendCallback`.
pub struct StdoutEvalChannel {
    cb_port: u16,
    dir: PathBuf,
}

impl StdoutEvalChannel {
    /// The channel for a daemon listening on `cb_port` (`RBPort + 1`).
    pub fn new(cb_port: u16) -> Self {
        Self {
            cb_port,
            dir: PathBuf::from("/tmp"),
        }
    }

    /// The channel with an explicit callback directory (used by tests).
    pub fn with_dir(cb_port: u16, dir: impl Into<PathBuf>) -> Self {
        Self {
            cb_port,
            dir: dir.into(),
        }
    }

    /// The file the SKILL side writes the answer into.
    pub fn data_file(&self) -> PathBuf {
        self.dir.join(format!(".ramic_cb_{}", self.cb_port))
    }

    /// The completion marker written after the answer.
    pub fn done_file(&self) -> PathBuf {
        self.dir.join(format!(".ramic_cb_{}.done", self.cb_port))
    }

    /// The in-flight marker: written by the SKILL side when it *enters* an
    /// evaluation. Its presence at timeout time proves that Virtuoso took
    /// the expression and never came back.
    pub fn started_file(&self) -> PathBuf {
        self.dir.join(format!(".ramic_cb_{}.started", self.cb_port))
    }
}

impl EvalChannel for StdoutEvalChannel {
    fn submit(&mut self, skill: &str) -> io::Result<()> {
        // Drop stale callback files first: a late answer from a previous
        // request must never be mistaken for this request's answer.
        let _ = std::fs::remove_file(self.data_file());
        let _ = std::fs::remove_file(self.done_file());
        let _ = std::fs::remove_file(self.started_file());

        let stdout = io::stdout();
        let mut out = stdout.lock();
        out.write_all(skill.as_bytes())?;
        out.flush()
    }

    fn wait_for_callback(&mut self, timeout_secs: u64) -> io::Result<Option<Vec<u8>>> {
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            if Instant::now() > deadline {
                return Ok(None);
            }

            if self.done_file().exists() {
                match std::fs::read(self.data_file()) {
                    Ok(mut data) => {
                        let _ = std::fs::remove_file(self.data_file());
                        let _ = std::fs::remove_file(self.done_file());
                        // Strip trailing RS marker written by RBSendCallback's sprintf %c
                        if data.last() == Some(&RS) {
                            data.pop();
                        }
                        return Ok(Some(data));
                    }
                    Err(_) => {
                        // done_file appeared but data_file not readable yet — retry
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn eval_entered(&self) -> Option<bool> {
        Some(self.started_file().exists())
    }
}

/// The daemon's view of the evaluation channel.
///
/// Once the channel is hung it stays hung: the stuck evaluation may still be
/// running inside the SKILL interpreter, and no amount of waiting makes it
/// safe to write to that pipe again. Recovery is a session restart.
#[derive(Debug, Default, Clone)]
pub struct ChannelState {
    stuck: Option<StuckCall>,
}

impl ChannelState {
    /// A healthy channel.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when the channel is dead.
    pub fn is_hung(&self) -> bool {
        self.stuck.is_some()
    }

    /// The stuck evaluation, if the channel is dead.
    pub fn stuck_call(&self) -> Option<&StuckCall> {
        self.stuck.as_ref()
    }

    /// Mark the channel dead, recording what hung it.
    pub fn mark_hung(&mut self, call: StuckCall) {
        self.stuck = Some(call);
    }
}

/// Handle one request and produce the response frame.
///
/// The frame is the raw byte string the daemon writes back: either the
/// answer the SKILL side produced (`STX` + result) or a `NAK` + message.
pub fn handle_request(
    channel: &mut dyn EvalChannel,
    state: &mut ChannelState,
    req: &SkillRequest,
    port: u16,
) -> Vec<u8> {
    let timeout = req.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS);

    // ── Hung channel ────────────────────────────────────────────────────
    // Answer immediately. Nothing is written to the Virtuoso pipe: the
    // evaluation that is stuck there is still holding it.
    if let Some(stuck) = state.stuck_call() {
        ERRORS.fetch_add(1, Ordering::Relaxed);
        let msg = channel_state::hung_message(stuck);
        eprintln!("[virtuoso-daemon] refusing request on a hung channel: {msg}");
        return nak_frame(&msg);
    }

    let started_at_unix = unix_now();
    let started_at = rfc3339_now();

    if let Err(e) = channel.submit(&req.skill) {
        ERRORS.fetch_add(1, Ordering::Relaxed);
        let msg = format!(
            "skill delivery failed: {e} — the channel to Virtuoso is not usable; \
             restart the Virtuoso session"
        );
        eprintln!("[virtuoso-daemon] {msg}");
        return nak_frame(&msg);
    }

    match channel.wait_for_callback(timeout) {
        Ok(Some(answer)) => answer,
        Ok(None) => {
            // The evaluation never came back. The channel is dead from here
            // on: the SKILL interpreter may still be inside that evalstring.
            ERRORS.fetch_add(1, Ordering::Relaxed);
            let stuck = StuckCall::new(&req.skill, started_at, started_at_unix, timeout);
            let entered = channel.eval_entered();
            let mut msg = channel_state::hung_message(&stuck);
            if let Some(true) = entered {
                msg.push_str(
                    "\nIn-flight: Virtuoso entered the evaluation (in-flight marker present) \
                     and never returned.",
                );
            }
            state.mark_hung(stuck.clone());
            write_hung_record(port, &stuck);
            eprintln!("[virtuoso-daemon] {msg}");
            nak_frame(&msg)
        }
        Err(e) => {
            ERRORS.fetch_add(1, Ordering::Relaxed);
            let msg = format!("callback read failed: {e}");
            eprintln!("[virtuoso-daemon] {msg}");
            nak_frame(&msg)
        }
    }
}

/// A `NAK` frame carrying `message`.
fn nak_frame(message: &str) -> Vec<u8> {
    let mut frame = Vec::with_capacity(message.len() + 1);
    frame.push(NAK);
    frame.extend_from_slice(message.as_bytes());
    frame
}

/// Serve one TCP connection: read the request, run it, answer it.
pub fn handle_connection(
    conn: &mut TcpStream,
    channel: &mut dyn EvalChannel,
    state: &mut ChannelState,
    port: u16,
) -> io::Result<()> {
    CALLS.fetch_add(1, Ordering::Relaxed);

    let mut req_bytes = Vec::new();
    conn.read_to_end(&mut req_bytes)?;

    let req: SkillRequest = serde_json::from_slice(&req_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {e}")))?;

    let response = handle_request(channel, state, &req, port);

    write_stats(port, state);
    conn.write_all(&response)?;
    let _ = conn.shutdown(Shutdown::Both);

    Ok(())
}

/// Path of the daemon's stats file.
pub fn stats_path(port: u16) -> String {
    format!("/tmp/.ramic_stats_{port}")
}

/// Path of the daemon's hung-channel record.
pub fn hung_record_path(port: u16) -> String {
    format!("/tmp/.ramic_hung_{port}.json")
}

/// Write the runtime metrics, including the hung-channel state when there is
/// one, so `session show` / the TUI can tell a dead channel from a busy one.
pub fn write_stats(port: u16, state: &ChannelState) {
    write_stats_file(Path::new(&stats_path(port)), state);
}

/// The stats writer, with the path separated out so it can be tested.
pub fn write_stats_file(path: &Path, state: &ChannelState) {
    let uptime = START.get().map(|t| t.elapsed().as_secs()).unwrap_or(0);
    let mut json = serde_json::json!({
        "calls": CALLS.load(Ordering::Relaxed),
        "errors": ERRORS.load(Ordering::Relaxed),
        "uptime_secs": uptime,
    });
    if let Some(stuck) = state.stuck_call() {
        json["channel_hung"] = serde_json::json!(true);
        json["stuck_skill"] = serde_json::json!(stuck.skill_summary);
        json["stuck_started_at"] = serde_json::json!(stuck.started_at);
        json["stuck_started_unix"] = serde_json::json!(stuck.started_unix);
        json["stuck_timeout_secs"] = serde_json::json!(stuck.timeout_secs);
    }
    let _ = std::fs::write(path, json.to_string());
}

/// Record the stuck call in its own file, for post-mortem inspection.
fn write_hung_record(port: u16, stuck: &StuckCall) {
    let json = serde_json::json!({
        "channel_hung": true,
        "stuck_skill": stuck.skill_summary,
        "stuck_started_at": stuck.started_at,
        "stuck_started_unix": stuck.started_unix,
        "stuck_timeout_secs": stuck.timeout_secs,
    });
    let _ = std::fs::write(hung_record_path(port), json.to_string());
}

/// Seconds since the Unix epoch.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The current instant as an RFC 3339 UTC timestamp.
fn rfc3339_now() -> String {
    chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).to_rfc3339()
}

/// Entry point of the `virtuoso-daemon` binary.
pub fn run_cli() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.len() < 3 {
        eprintln!("usage: virtuoso-daemon <host> <port>");
        process::exit(1);
    }

    let host = &args[1];
    let port: u16 = args[2].parse().unwrap_or_else(|_| {
        eprintln!("invalid port: {}", args[2]);
        process::exit(1);
    });

    let listener = std::net::TcpListener::bind(format!("{host}:{port}")).unwrap_or_else(|e| {
        eprintln!("failed to bind {host}:{port}: {e}");
        process::exit(1);
    });

    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    // Print version and port to stderr so ramic_bridge.il can read them.
    // VERSION must come before PORT so it is stored before the banner fires.
    eprintln!("VERSION:{}", env!("CARGO_PKG_VERSION"));
    eprintln!("PORT:{actual_port}");
    eprintln!("[virtuoso-daemon] listening on {host}:{actual_port}");

    // cb_port mirrors RBCallbackPort in ramic_bridge.il (RBPort + 1).
    // Must use actual_port, not the argv port (which may be 0 for OS-assigned).
    let cb_port = actual_port + 1;

    START.get_or_init(std::time::Instant::now);

    let mut channel = StdoutEvalChannel::new(cb_port);
    let mut state = ChannelState::new();

    for stream in listener.incoming() {
        match stream {
            Ok(mut conn) => {
                if let Err(e) = handle_connection(&mut conn, &mut channel, &mut state, actual_port)
                {
                    eprintln!("[virtuoso-daemon] error: {e}");
                }
            }
            Err(e) => {
                eprintln!("[virtuoso-daemon] accept error: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_state::{HUNG_TEXT_EN, HUNG_TEXT_ZH};
    use std::cell::RefCell;

    /// A channel that records every delivery and answers on demand.
    struct FakeChannel {
        delivered: RefCell<Vec<String>>,
        answer: Option<Vec<u8>>,
        entered: Option<bool>,
        submit_fails: bool,
    }

    impl FakeChannel {
        /// A channel whose evaluations never come back.
        fn hanging() -> Self {
            Self {
                delivered: RefCell::new(Vec::new()),
                answer: None,
                entered: Some(true),
                submit_fails: false,
            }
        }

        /// A channel that answers with `answer`.
        fn answering(answer: Vec<u8>) -> Self {
            Self {
                delivered: RefCell::new(Vec::new()),
                answer: Some(answer),
                entered: Some(true),
                submit_fails: false,
            }
        }

        fn delivered(&self) -> Vec<String> {
            self.delivered.borrow().clone()
        }
    }

    impl EvalChannel for FakeChannel {
        fn submit(&mut self, skill: &str) -> io::Result<()> {
            if self.submit_fails {
                return Err(io::Error::other("broken pipe"));
            }
            self.delivered.borrow_mut().push(skill.to_string());
            Ok(())
        }

        fn wait_for_callback(&mut self, _timeout_secs: u64) -> io::Result<Option<Vec<u8>>> {
            Ok(self.answer.clone())
        }

        fn eval_entered(&self) -> Option<bool> {
            self.entered
        }
    }

    fn request(skill: &str) -> SkillRequest {
        SkillRequest {
            skill: skill.to_string(),
            timeout: Some(1),
        }
    }

    fn payload(frame: &[u8]) -> String {
        String::from_utf8_lossy(&frame[1..]).into_owned()
    }

    #[test]
    fn daemon_timeout_does_not_enqueue_another_skill() {
        let mut channel = FakeChannel::hanging();
        let mut state = ChannelState::new();

        let first = handle_request(&mut channel, &mut state, &request("1+1"), 0);
        assert_eq!(first[0], NAK, "a timeout must answer NAK");
        assert_eq!(
            channel.delivered().len(),
            1,
            "the first request is delivered once"
        );

        // The second request must be refused *without* touching the pipe.
        let started = Instant::now();
        let second = handle_request(&mut channel, &mut state, &request("1+1"), 0);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a hung channel must answer immediately, took {:?}",
            started.elapsed()
        );
        assert_eq!(
            channel.delivered().len(),
            1,
            "the hung channel must not receive another SKILL"
        );
        assert_eq!(second[0], NAK);
    }

    #[test]
    fn daemon_timeout_error_mentions_outstanding_eval() {
        let mut channel = FakeChannel::hanging();
        let mut state = ChannelState::new();

        let frame = handle_request(
            &mut channel,
            &mut state,
            &request("maeOpenSetup(\"LIB\" \"CELL\" \"maestro\")"),
            0,
        );
        let text = payload(&frame);
        assert!(
            text.contains(HUNG_TEXT_ZH),
            "missing the Chinese text: {text}"
        );
        assert!(
            text.contains(HUNG_TEXT_EN),
            "missing the English text: {text}"
        );
        assert!(
            text.contains("maeOpenSetup"),
            "the stuck call must be named: {text}"
        );
        assert!(
            text.contains("timeout_secs=1"),
            "the timeout must be reported: {text}"
        );
        assert!(
            text.contains("started_at="),
            "the start time must be reported: {text}"
        );
        assert!(
            text.contains("In-flight:"),
            "the in-flight marker must be reported: {text}"
        );
    }

    #[test]
    fn daemon_marks_channel_hung_after_timeout() {
        let mut channel = FakeChannel::hanging();
        let mut state = ChannelState::new();
        assert!(!state.is_hung());

        handle_request(&mut channel, &mut state, &request("1+1"), 0);

        assert!(state.is_hung());
        let stuck = state.stuck_call().expect("the stuck call is recorded");
        assert_eq!(stuck.skill_summary, "1+1");
        assert_eq!(stuck.timeout_secs, 1);
        assert!(stuck.started_unix > 0);
    }

    #[test]
    fn a_successful_evaluation_leaves_the_channel_healthy() {
        let mut channel = FakeChannel::answering(vec![0x02, b'2']);
        let mut state = ChannelState::new();

        let frame = handle_request(&mut channel, &mut state, &request("1+1"), 0);

        assert_eq!(frame, vec![0x02, b'2'], "the answer is passed through");
        assert!(
            !state.is_hung(),
            "a healthy answer must not hang the channel"
        );
        assert_eq!(channel.delivered().len(), 1);
    }

    #[test]
    fn the_hung_stats_file_records_the_stuck_call() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.json");

        let mut channel = FakeChannel::hanging();
        let mut state = ChannelState::new();
        handle_request(&mut channel, &mut state, &request("1+1"), 0);
        write_stats_file(&path, &state);

        let raw = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(json["channel_hung"], serde_json::json!(true));
        assert_eq!(json["stuck_skill"], serde_json::json!("1+1"));
        assert_eq!(json["stuck_timeout_secs"], serde_json::json!(1));
        assert!(json["stuck_started_at"].as_str().unwrap().contains('T'));
    }

    #[test]
    fn a_healthy_stats_file_has_no_hung_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.json");
        let state = ChannelState::new();
        write_stats_file(&path, &state);
        let raw = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(json.get("channel_hung").is_none());
        assert!(json.get("calls").is_some());
        assert!(json.get("uptime_secs").is_some());
    }

    #[test]
    fn a_broken_pipe_is_reported_without_hanging_the_channel() {
        let mut channel = FakeChannel::hanging();
        channel.submit_fails = true;
        let mut state = ChannelState::new();

        let frame = handle_request(&mut channel, &mut state, &request("1+1"), 0);
        assert_eq!(frame[0], NAK);
        assert!(payload(&frame).contains("skill delivery failed"));
        assert!(
            !state.is_hung(),
            "a delivery failure is not a stuck evaluation"
        );
    }
}
