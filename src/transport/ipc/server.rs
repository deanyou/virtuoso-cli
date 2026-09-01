//! Server half of the transport IPC protocol.
//!
//! [`serve_one`] handles a single connection: it performs the `Hello` handshake
//! and then dispatches every subsequent request onto the supplied
//! [`RemoteTransport`] until the peer closes. [`run`] binds a Unix domain
//! socket at the given path, sets its mode to `0600`, and accepts connections
//! forever (or until the listener is dropped by `SIGTERM`/`SIGINT` handling
//! wired in the daemon subcommand).
//!
//! `Challenge` is the Tier-1 liveness probe described in the design's
//! "Stop and crash recovery" section: the parent CLI connects over IPC, asks
//! the daemon for its nonce, and compares the answer against the value it
//! recorded in the state file. A correct answer proves the process on the
//! other end is the recorded daemon — no PID or platform identity check
//! needed. This module is the one that knows the daemon's nonce (passed in
//! from the daemon's startup args), so it can answer the challenge.
//!
//! The server is Unix-only because the first transport is a Unix domain
//! socket. Windows named-pipe transport extends this module via the
//! transport-agnostic framing layer.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread;

// `run` is the only consumer of these, and it is feature-gated, so the imports
// must carry the same gate — ungated, a feature-off build reports them unused.
#[cfg(feature = "native-ssh")]
use std::os::unix::net::UnixListener;
#[cfg(feature = "native-ssh")]
use std::path::Path;

use serde_json::Value;

use crate::transport::contract::{
    CommandRequest, CommandResult, DownloadDirRequest, DownloadFileRequest, RemoteTransport,
    RequestId, UploadFileRequest, UploadTextRequest,
};
use crate::transport::ipc::framing::{
    FrameError, FrameReader, FrameWriter, PROTOCOL_MAJOR, PROTOCOL_MINOR,
};
use crate::transport::ipc::messages::{
    Hello, HelloAck, IpcError, Operation, RequestEnvelope, ResponseEnvelope, ResponseResult,
};

/// Wire payload for `Operation::RunCommand`. Mirrors `CommandRequest` but
/// projects the deadline as unix-ms (the envelope field) and exposes the
/// execution timeout as seconds for JSON cleanliness.
///
/// `pub(crate)` so the [`crate::transport::ipc::daemon`] client can encode
/// and decode without re-declaring the same shape.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct WireCommand {
    pub(crate) command: String,
    pub(crate) timeout: Option<u64>,
}

/// Wire mirror of `CommandResult`.
///
/// `CommandResult` carries a `std::time::Duration`, which this build's serde
/// configuration does not serialize, so the daemon and client translate through
/// this millisecond-based projection instead of deriving `Serialize` on the
/// contract type.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct WireCommandResult {
    pub(crate) exit_status: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) success: bool,
    pub(crate) duration_ms: u128,
}

impl From<&CommandResult> for WireCommandResult {
    fn from(r: &CommandResult) -> Self {
        Self {
            exit_status: r.exit_status,
            stdout: r.stdout.clone(),
            stderr: r.stderr.clone(),
            success: r.success,
            duration_ms: r.duration.as_millis(),
        }
    }
}

impl From<WireCommandResult> for CommandResult {
    fn from(w: WireCommandResult) -> Self {
        Self {
            exit_status: w.exit_status,
            stdout: w.stdout,
            stderr: w.stderr,
            success: w.success,
            duration: std::time::Duration::from_millis(w.duration_ms as u64),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct WireUploadFile {
    pub(crate) local: String,
    pub(crate) remote: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct WireUploadText {
    pub(crate) text: String,
    pub(crate) remote: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct WireDownloadFile {
    pub(crate) remote: String,
    pub(crate) local: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct WireDownloadDir {
    pub(crate) remote: String,
    pub(crate) local: String,
}

/// Payload returned by the daemon's Tier-1 challenge answer.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ChallengeAck {
    /// The daemon nonce the server generated at startup. The parent CLI
    /// compares it against the value it recorded in the state file when it
    /// launched the daemon; equality proves the process on the other end is
    /// the recorded daemon instance.
    pub daemon_nonce: String,
}

/// What `serve_one` does next when it has finished the handshake: either it
/// knows the peer or it must refuse.
enum HandshakeOutcome {
    /// Handshake succeeded; the caller should answer `Ok` and continue with
    /// the dispatch loop.
    Ok,
    /// Handshake failed; the caller should answer `Err` with the encoded
    /// [`IpcError`] and close the connection.
    Err(IpcError),
}

/// Validate an already-received `Hello` envelope and, if it passes, write the
/// `HelloAck` back on `writer`. Returns [`HandshakeOutcome::Ok`] on success or
/// the [`IpcError`] the caller should send back on failure.
///
/// The envelope is passed in — not read here — because [`serve_one`] must
/// inspect the first frame *before* deciding whether it is a handshake at all
/// (it needs the request id to echo, and it closes silently on anything that
/// is not `Hello`). Reading it again here would consume a second frame the
/// peer never sends, and both sides would block forever waiting on each other.
///
/// Splitting this out from [`serve_one`] keeps the dispatch loop below
/// readable and lets unit tests exercise the handshake independently.
fn do_handshake<W>(
    writer: &mut FrameWriter<W>,
    hello_env: &RequestEnvelope,
    expected_token: &str,
    server_nonce: &str,
) -> HandshakeOutcome
where
    W: std::io::Write,
{
    if hello_env.operation != Operation::Hello {
        return HandshakeOutcome::Err(IpcError::Configuration(format!(
            "expected Hello, got {:?}",
            hello_env.operation
        )));
    }
    if hello_env.protocol_major != PROTOCOL_MAJOR {
        return HandshakeOutcome::Err(IpcError::ProtocolMismatch {
            expected: PROTOCOL_MAJOR.to_string(),
            actual: hello_env.protocol_major.to_string(),
        });
    }
    if hello_env.auth_token != expected_token {
        return HandshakeOutcome::Err(IpcError::AuthenticationFailed(
            "auth token did not match".into(),
        ));
    }
    // Parse the hello body just to confirm the profile is a string — we don't
    // gate on profile name yet (the daemon is per-profile, so this is mostly a
    // sanity check).
    let _hello: Hello = match serde_json::from_value(hello_env.payload.clone()) {
        Ok(h) => h,
        Err(e) => {
            return HandshakeOutcome::Err(IpcError::Configuration(format!(
                "malformed hello payload: {e}"
            )))
        }
    };
    let ack = HelloAck {
        server_major: PROTOCOL_MAJOR,
        server_minor: PROTOCOL_MINOR,
        daemon_nonce: server_nonce.to_string(),
        capabilities: BTreeSet::new(),
    };
    let resp = ResponseEnvelope {
        request_id: hello_env.request_id.clone(),
        result: ResponseResult::Ok(serde_json::to_value(&ack).unwrap_or(Value::Null)),
    };
    let body = match serde_json::to_vec(&resp) {
        Ok(b) => b,
        Err(e) => return HandshakeOutcome::Err(IpcError::LocalIo(e.to_string())),
    };
    if let Err(e) = writer.write_frame(&body) {
        return HandshakeOutcome::Err(match e {
            FrameError::Io(_) => IpcError::DaemonUnavailable,
            other => IpcError::LocalIo(other.to_string()),
        });
    }
    HandshakeOutcome::Ok
}

/// Serve a single connection: perform the Hello handshake, then dispatch
/// every subsequent request onto `transport` until the peer closes.
///
/// `server_nonce` is what the daemon returned in the `HelloAck`; the
/// subsequent `Challenge` request echoes it back, and that echo is what the
/// parent CLI uses to prove the daemon is the recorded instance.
///
/// `auth_token` is the secret stored in the daemon's state file. Clients
/// that connect must present it in their `Hello` envelope; a mismatched
/// token is rejected with `AuthenticationFailed` and the socket is closed.
pub fn serve_one(
    stream: UnixStream,
    transport: Arc<dyn RemoteTransport>,
    auth_token: &str,
    server_nonce: &str,
) {
    let read_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = FrameReader::new(read_stream);
    let mut writer = FrameWriter::new(stream);

    // Peek the first frame so we know the request id to echo back.
    let first_frame = match reader.read_frame() {
        Ok(Some(b)) => b,
        _ => return, // EOF or framing error → nothing to do.
    };
    let first_env: RequestEnvelope = match serde_json::from_slice(&first_frame) {
        Ok(env) => env,
        Err(_) => return,
    };
    if first_env.operation != Operation::Hello {
        // A peer that doesn't even send Hello is not a client we should
        // answer — close silently. (An active attacker would learn nothing
        // here that isn't in the public protocol spec.)
        return;
    }
    let request_id = first_env.request_id.clone();
    let outcome = do_handshake(&mut writer, &first_env, auth_token, server_nonce);
    if let HandshakeOutcome::Err(e) = outcome {
        let resp = ResponseEnvelope {
            request_id,
            result: ResponseResult::Err(e),
        };
        let body = serde_json::to_vec(&resp).unwrap_or_default();
        let _ = writer.write_frame(&body);
        return;
    }

    // Dispatch loop.
    while let Ok(Some(frame)) = reader.read_frame() {
        let env: RequestEnvelope = match serde_json::from_slice(&frame) {
            Ok(env) => env,
            Err(e) => {
                let resp = ResponseEnvelope {
                    request_id: String::new(),
                    result: ResponseResult::Err(IpcError::LocalIo(e.to_string())),
                };
                let body = serde_json::to_vec(&resp).unwrap_or_default();
                let _ = writer.write_frame(&body);
                continue;
            }
        };
        let resp = ResponseEnvelope {
            request_id: env.request_id.clone(),
            result: dispatch(&*transport, &env, server_nonce),
        };
        let body = match serde_json::to_vec(&resp) {
            Ok(b) => b,
            Err(e) => {
                let resp = ResponseEnvelope {
                    request_id: env.request_id.clone(),
                    result: ResponseResult::Err(IpcError::LocalIo(e.to_string())),
                };
                serde_json::to_vec(&resp).unwrap_or_default()
            }
        };
        if writer.write_frame(&body).is_err() {
            // Peer closed — bail out of the loop. We don't try to recover
            // half-written state: every operation is idempotent or reports
            // `OutcomeUnknown`, and the next request will start fresh.
            break;
        }
    }
}

/// Dispatch a single request onto a transport. Pure function: no I/O,
/// no thread safety, no daemon state beyond the nonce passed in for the
/// Challenge answer.
pub fn dispatch(
    transport: &dyn RemoteTransport,
    env: &RequestEnvelope,
    server_nonce: &str,
) -> ResponseResult {
    use crate::transport::contract::Deadline;
    let deadline = Deadline::from_unix_ms(env.deadline_unix_ms);
    let request_id = env.request_id.clone();
    match &env.operation {
        Operation::Hello => ResponseResult::Err(IpcError::Configuration(
            "Hello must precede any other operation".into(),
        )),
        Operation::TestConnection => match transport.test_connection(deadline) {
            Ok(b) => match serde_json::to_value(b) {
                Ok(v) => ResponseResult::Ok(v),
                Err(e) => ResponseResult::Err(IpcError::LocalIo(e.to_string())),
            },
            Err(e) => ResponseResult::Err(IpcError::from(e)),
        },
        Operation::RunCommand => {
            let w: WireCommand = match serde_json::from_value(env.payload.clone()) {
                Ok(w) => w,
                Err(e) => return ResponseResult::Err(IpcError::Configuration(e.to_string())),
            };
            let req = CommandRequest {
                id: RequestId(request_id),
                deadline,
                command: w.command,
                timeout: w.timeout.map(std::time::Duration::from_secs),
            };
            match transport.run_command(&req) {
                Ok(r) => ResponseResult::Ok(
                    serde_json::to_value(WireCommandResult::from(&r)).unwrap_or(Value::Null),
                ),
                Err(e) => ResponseResult::Err(IpcError::from(e)),
            }
        }
        Operation::UploadFile => {
            let w: WireUploadFile = match serde_json::from_value(env.payload.clone()) {
                Ok(w) => w,
                Err(e) => return ResponseResult::Err(IpcError::Configuration(e.to_string())),
            };
            let req = UploadFileRequest {
                id: RequestId(request_id),
                deadline,
                local: std::path::PathBuf::from(w.local),
                remote: w.remote,
            };
            match transport.upload_file(&req) {
                Ok(()) => ResponseResult::Ok(Value::Null),
                Err(e) => ResponseResult::Err(IpcError::from(e)),
            }
        }
        Operation::UploadText => {
            let w: WireUploadText = match serde_json::from_value(env.payload.clone()) {
                Ok(w) => w,
                Err(e) => return ResponseResult::Err(IpcError::Configuration(e.to_string())),
            };
            let req = UploadTextRequest {
                id: RequestId(request_id),
                deadline,
                text: w.text,
                remote: w.remote,
            };
            match transport.upload_text(&req) {
                Ok(()) => ResponseResult::Ok(Value::Null),
                Err(e) => ResponseResult::Err(IpcError::from(e)),
            }
        }
        Operation::DownloadFile => {
            let w: WireDownloadFile = match serde_json::from_value(env.payload.clone()) {
                Ok(w) => w,
                Err(e) => return ResponseResult::Err(IpcError::Configuration(e.to_string())),
            };
            let req = DownloadFileRequest {
                id: RequestId(request_id),
                deadline,
                remote: w.remote,
                local: std::path::PathBuf::from(w.local),
            };
            match transport.download_file(&req) {
                Ok(()) => ResponseResult::Ok(Value::Null),
                Err(e) => ResponseResult::Err(IpcError::from(e)),
            }
        }
        Operation::DownloadDir => {
            let w: WireDownloadDir = match serde_json::from_value(env.payload.clone()) {
                Ok(w) => w,
                Err(e) => return ResponseResult::Err(IpcError::Configuration(e.to_string())),
            };
            let req = DownloadDirRequest {
                id: RequestId(request_id),
                deadline,
                remote: w.remote,
                local: std::path::PathBuf::from(w.local),
            };
            match transport.download_dir(&req) {
                Ok(()) => ResponseResult::Ok(Value::Null),
                Err(e) => ResponseResult::Err(IpcError::from(e)),
            }
        }
        Operation::Challenge => {
            // Tier-1 liveness proof: the daemon echoes its nonce. The parent
            // CLI compares the answer to its recorded `daemon_nonce` to prove
            // the process on the other end of the socket is the recorded
            // daemon instance. We deliberately do not include the auth_token
            // in the answer — anyone who could present the token has already
            // passed the handshake; this operation is the *separate* check
            // that runs before auth is granted.
            let ack = ChallengeAck {
                daemon_nonce: server_nonce.to_string(),
            };
            match serde_json::to_value(&ack) {
                Ok(v) => ResponseResult::Ok(v),
                Err(e) => ResponseResult::Err(IpcError::LocalIo(e.to_string())),
            }
        }
        Operation::StartLocalForward
        | Operation::StopLocalForward
        | Operation::Health
        | Operation::Shutdown => ResponseResult::Err(IpcError::UnsupportedOperation(format!(
            "{:?}",
            env.operation
        ))),
        Operation::Cancel => ResponseResult::Err(IpcError::UnsupportedOperation(
            "cancel: per-request cancellation is a step-6 increment".into(),
        )),
        Operation::Unknown(name) => {
            ResponseResult::Err(IpcError::UnsupportedOperation(name.clone()))
        }
    }
}

/// Bind a Unix domain socket at `socket_path`, set its mode to `0600`, and
/// spawn one OS thread per accepted connection. Each thread runs
/// [`serve_one`] until the peer closes.
///
/// Blocks for as long as the listener is alive. Step 6 owns the
/// graceful-shutdown contract (`SIGTERM`/`SIGINT`), so under normal use this
/// function never returns — it only returns `Err` if the socket cannot be
/// bound or the mode cannot be tightened to `0600`.
///
/// Gated to `native-ssh` because it is the production entry point: it is
/// only called by the daemon subcommand, which is itself feature-gated. The
/// other server entry points ([`serve_one`], [`dispatch`]) remain visible to
/// tests so the shared contract suite can exercise the IPC path without the
/// feature.
#[cfg(feature = "native-ssh")]
pub fn run(
    socket_path: &Path,
    transport: Arc<dyn RemoteTransport>,
    auth_token: &str,
    server_nonce: &str,
) -> Result<(), String> {
    if let Some(parent) = socket_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(format!(
                "ipc socket parent directory does not exist: {}",
                parent.display()
            ));
        }
    }
    // Best-effort cleanup of any stale socket left by a crashed previous run.
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)
        .map_err(|e| format!("failed to bind ipc socket {}: {e}", socket_path.display()))?;
    // Mode 0600: only the current user can connect. The owner is set by the
    // bind above; chmod enforces the bits regardless of umask.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(socket_path, perms) {
            return Err(format!(
                "failed to chmod 0600 ipc socket {}: {e}",
                socket_path.display()
            ));
        }
    }
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let transport = transport.clone();
                let token = auth_token.to_string();
                let nonce = server_nonce.to_string();
                thread::Builder::new()
                    .name("vcli-ipc".to_string())
                    .spawn(move || serve_one(s, transport, &token, &nonce))
                    .map_err(|e| format!("failed to spawn ipc worker: {e}"))?;
            }
            Err(e) => {
                // A spurious accept error is not fatal: the listener is still
                // alive and the next connection might succeed. We deliberately
                // do not return here — a transient resource exhaustion should
                // not tear down the daemon.
                eprintln!("vcli __transport-daemon: accept failed: {e}");
            }
        }
    }
    Ok(())
}

// ────────────────────────────────── tests ──────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::contract::test_support::FakeTransport;
    use crate::transport::contract::{CommandRequest, Deadline, UploadTextRequest};
    use crate::transport::ipc::daemon::NativeTransportClient;
    use crate::transport::ipc::framing::FrameReader;
    use crate::transport::ipc::messages::IpcError;
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    /// Build a `NativeTransportClient` connected to `socket`. The matching
    /// daemon is launched in a background thread before this returns so the
    /// Hello handshake always sees a ready server.
    fn start_daemon(
        transport: Arc<dyn RemoteTransport>,
        token: &str,
        nonce: &str,
    ) -> (std::path::PathBuf, UnixListener) {
        let socket = std::env::temp_dir().join(format!("vcli-test-{}.sock", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).expect("bind");
        let path = socket.clone();
        let token = token.to_string();
        let nonce = nonce.to_string();
        let listener_for_thread = listener.try_clone().expect("clone listener");
        thread::spawn(move || {
            // Accept exactly one connection, serve it until EOF, then exit.
            if let Ok((stream, _)) = listener_for_thread.accept() {
                serve_one(stream, transport, &token, &nonce);
            }
        });
        (path, listener)
    }

    /// The shared contract suite must pass against the real server, not just
    /// the in-test fake dispatcher. This is the step-2 gate.
    #[test]
    fn real_server_passes_shared_contract_suite() {
        let transport: Arc<dyn RemoteTransport> = Arc::new(FakeTransport::ok());
        let (socket, _listener) = start_daemon(transport, "secret-token", "test-nonce");

        let client = NativeTransportClient::connect(&socket, "test-profile", "secret-token")
            .expect("hello handshake");

        crate::transport::contract::test_support::shared_contract_suite(&client);

        // A real round-trip beyond the suite: a command, an upload, a probe.
        let ok = client
            .run_command(&CommandRequest::untimed("echo hi"))
            .unwrap();
        assert_eq!(ok.exit_status, 0);
        client
            .upload_text(&UploadTextRequest::untimed("payload", "/tmp/x"))
            .unwrap();
        assert!(client
            .test_connection(Deadline::from_now(Duration::from_secs(5)))
            .unwrap());

        drop(client);
        let _ = std::fs::remove_file(&socket);
    }

    /// A bad auth token must be rejected during Hello, not silently accepted.
    /// The connection closes after the error frame is sent.
    #[test]
    fn hello_rejects_bad_token() {
        let transport: Arc<dyn RemoteTransport> = Arc::new(FakeTransport::ok());
        let (socket, _listener) = start_daemon(transport, "secret-token", "n");

        // Use the raw framing layer to do a Hello with the wrong token and
        // inspect the server's reply.
        let stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
        let read = stream.try_clone().unwrap();
        let mut reader = FrameReader::new(read);
        let mut writer = FrameWriter::new(stream);

        let env = RequestEnvelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            profile: "p".into(),
            daemon_nonce: String::new(),
            auth_token: "wrong-token".into(),
            request_id: "r1".into(),
            deadline_unix_ms: 0,
            operation: Operation::Hello,
            payload: serde_json::to_value(Hello::new("p", "wrong-token")).unwrap(),
        };
        writer
            .write_frame(&serde_json::to_vec(&env).unwrap())
            .unwrap();
        let resp: ResponseEnvelope =
            serde_json::from_slice(&reader.read_frame().unwrap().unwrap()).unwrap();
        match resp.result {
            ResponseResult::Err(IpcError::AuthenticationFailed(_)) => {}
            other => panic!("expected AuthenticationFailed, got {other:?}"),
        }
        let _ = std::fs::remove_file(&socket);
    }

    /// Tier-1 challenge: the daemon echoes its nonce, the parent compares it
    /// to the recorded value. Equality proves the process on the other end
    /// is the recorded daemon — no PID, no platform-specific code.
    #[test]
    fn challenge_answers_with_the_server_nonce() {
        let transport: Arc<dyn RemoteTransport> = Arc::new(FakeTransport::ok());
        let (socket, _listener) = start_daemon(transport, "secret-token", "the-recorded-nonce");

        let stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
        let read = stream.try_clone().unwrap();
        let mut reader = FrameReader::new(read);
        let mut writer = FrameWriter::new(stream);

        // Handshake.
        let hello = RequestEnvelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            profile: "p".into(),
            daemon_nonce: String::new(),
            auth_token: "secret-token".into(),
            request_id: "r-hello".into(),
            deadline_unix_ms: 0,
            operation: Operation::Hello,
            payload: serde_json::to_value(Hello::new("p", "secret-token")).unwrap(),
        };
        writer
            .write_frame(&serde_json::to_vec(&hello).unwrap())
            .unwrap();
        let resp: ResponseEnvelope =
            serde_json::from_slice(&reader.read_frame().unwrap().unwrap()).unwrap();
        let ack: HelloAck = match resp.result {
            ResponseResult::Ok(v) => serde_json::from_value(v).unwrap(),
            ResponseResult::Err(e) => panic!("hello failed: {e:?}"),
        };
        assert_eq!(ack.daemon_nonce, "the-recorded-nonce");

        // Challenge.
        let challenge = RequestEnvelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            profile: "p".into(),
            daemon_nonce: ack.daemon_nonce.clone(),
            auth_token: "secret-token".into(),
            request_id: "r-challenge".into(),
            deadline_unix_ms: 0,
            operation: Operation::Challenge,
            payload: Value::Null,
        };
        writer
            .write_frame(&serde_json::to_vec(&challenge).unwrap())
            .unwrap();
        let resp: ResponseEnvelope =
            serde_json::from_slice(&reader.read_frame().unwrap().unwrap()).unwrap();
        let answer: ChallengeAck = match resp.result {
            ResponseResult::Ok(v) => serde_json::from_value(v).unwrap(),
            ResponseResult::Err(e) => panic!("challenge failed: {e:?}"),
        };
        assert_eq!(answer.daemon_nonce, "the-recorded-nonce");

        let _ = std::fs::remove_file(&socket);
    }
}
