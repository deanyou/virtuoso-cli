//! The synchronous `NativeTransportClient` and the integration test that
//! proves the IPC wiring end-to-end.
//!
//! The client is Unix-only because the first transport is a Unix domain
//! socket. Named-pipe (Windows) and TCP variants share this code through
//! the transport-agnostic [`framing`] layer.
//!
//! Wire payload shapes and the request dispatcher live in
//! [`crate::transport::ipc::server`], which is the production daemon. This
//! module owns the client half plus a test that round-trips through the
//! real server.

#![allow(dead_code)]

use crate::transport::contract::{
    CommandRequest, CommandResult, Deadline, DownloadDirRequest, DownloadFileRequest,
    RemoteTransport, RequestId, TransportError, UploadFileRequest, UploadTextRequest,
};
use crate::transport::ipc::framing::{
    FrameError, FrameReader, FrameWriter, PROTOCOL_MAJOR, PROTOCOL_MINOR,
};
use crate::transport::ipc::messages::{
    Hello, HelloAck, Operation, RequestEnvelope, ResponseEnvelope, ResponseResult,
};
// The wire payload types live in `ipc::server`, which is gated on
// `(unix, any(test, feature = "native-ssh"))`. Mirror that gate here so the
// import disappears in builds where the server is absent.
#[cfg(all(unix, any(test, feature = "native-ssh")))]
pub(crate) use crate::transport::ipc::server::{
    ChallengeAck, WireCommand, WireCommandResult, WireDownloadDir, WireDownloadFile,
    WireUploadFile, WireUploadText,
};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

// ───────────────────────────── client (production-shaped) ─────────────────────────────

#[cfg(all(unix, any(test, feature = "native-ssh")))]
use std::os::unix::net::UnixStream;

#[cfg(all(unix, any(test, feature = "native-ssh")))]
pub struct NativeTransportClient {
    conn: Mutex<UnixStream>,
    profile: String,
    auth_token: String,
    /// Nonce issued by the daemon during `Hello`. Every request must echo it so
    /// a daemon restart (new nonce) invalidates this client.
    daemon_nonce: String,
}

#[cfg(all(unix, any(test, feature = "native-ssh")))]
impl NativeTransportClient {
    /// Connect to a daemon's Unix socket and complete the `Hello` handshake.
    pub fn connect(
        socket_path: &Path,
        profile: &str,
        auth_token: &str,
    ) -> Result<Self, TransportError> {
        let stream =
            UnixStream::connect(socket_path).map_err(|_| TransportError::DaemonUnavailable)?;
        // Set a generous default socket timeout (5 minutes). Individual
        // `exchange()` calls tighten this to the request's remaining deadline
        // before reading the response, so long-running commands (large file
        // transfers, slow SKILL evals) are not cut off at 5 seconds.
        apply_socket_timeouts(&stream, Duration::from_secs(300));
        let conn = Mutex::new(stream);
        let nonce = do_handshake(&conn, profile, auth_token)?;
        Ok(Self {
            conn,
            profile: profile.to_string(),
            auth_token: auth_token.to_string(),
            daemon_nonce: nonce,
        })
    }

    /// Tier-1 liveness probe: ask the daemon to echo its nonce.
    ///
    /// Per the design's [Stop and crash recovery] contract, the parent CLI
    /// uses this to prove that the process on the other end of the IPC
    /// socket is the recorded daemon — a correct answer proves the daemon
    /// is reachable *and* still knows the nonce it was issued at startup.
    /// This is the normal path; Tier 2 (OS identity) is only consulted when
    /// this call returns `false`.
    ///
    /// [Stop and crash recovery]: ../../../docs/superpowers/specs/2026-08-29-native-remote-transport-design.md
    pub fn challenge(&self) -> Result<ChallengeAck, TransportError> {
        // Tier-1 is a one-shot probe, not a long-running operation. A short
        // deadline keeps `tunnel stop` responsive even when the daemon has
        // hung the socket open without serving.
        let resp = self.exchange(
            Operation::Challenge,
            Value::Null,
            Deadline::from_now(Duration::from_secs(2)),
        )?;
        match resp.result {
            ResponseResult::Ok(v) => serde_json::from_value(v)
                .map_err(|e| TransportError::LocalIo(format!("ipc decode: {e}"))),
            ResponseResult::Err(e) => Err(TransportError::from(e)),
        }
    }

    /// Ask the daemon to shut down cooperatively.
    ///
    /// The daemon acks, fires its internal cancellation token, and closes this
    /// connection; its accept loop stops admitting and the shutdown
    /// coordinator runs the design's three phases (stop admission → grace for
    /// in-flight work → exit). Returns once the ack is received — daemon exit
    /// itself is asynchronous and bounded by `VB_TRANSPORT_SHUTDOWN_GRACE`.
    pub fn request_shutdown(&self) -> Result<(), TransportError> {
        let resp = self.exchange(
            Operation::Shutdown,
            Value::Null,
            Deadline::from_now(Duration::from_secs(5)),
        )?;
        match resp.result {
            ResponseResult::Ok(_) => Ok(()),
            ResponseResult::Err(e) => Err(TransportError::from(e)),
        }
    }

    fn exchange(
        &self,
        operation: Operation,
        payload: Value,
        deadline: Deadline,
    ) -> Result<ResponseEnvelope, TransportError> {
        let env = RequestEnvelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            profile: self.profile.clone(),
            daemon_nonce: self.daemon_nonce.clone(),
            auth_token: self.auth_token.clone(),
            request_id: RequestId::new().0,
            deadline_unix_ms: deadline.to_unix_ms(),
            operation,
            payload,
        };
        let bytes = serde_json::to_vec(&env)
            .map_err(|e| TransportError::LocalIo(format!("ipc encode: {e}")))?;

        let mut guard = self.conn.lock().unwrap();
        // Tighten the socket read timeout to the request's remaining deadline
        // before reading the response. This ensures long-running operations
        // (large transfers, slow evals) are not cut off by the 5-minute
        // default, while genuinely hung daemons still time out predictably.
        let remaining = deadline.remaining();
        apply_socket_timeouts(&*guard, remaining.max(Duration::from_secs(1)));
        FrameWriter::new(&mut *guard)
            .write_frame(&bytes)
            .map_err(frame_err_to_transport)?;
        let resp_bytes = match FrameReader::new(&mut *guard).read_frame() {
            Ok(Some(b)) => b,
            Ok(None) => return Err(TransportError::DaemonUnavailable),
            Err(e) => return Err(frame_err_to_transport(e)),
        };
        drop(guard);

        serde_json::from_slice(&resp_bytes)
            .map_err(|e| TransportError::LocalIo(format!("ipc decode: {e}")))
    }
}

#[cfg(all(unix, any(test, feature = "native-ssh")))]
fn do_handshake(
    conn: &Mutex<UnixStream>,
    profile: &str,
    auth_token: &str,
) -> Result<String, TransportError> {
    let hello = Hello::new(profile, auth_token);
    let env = RequestEnvelope {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        profile: profile.to_string(),
        daemon_nonce: String::new(),
        auth_token: auth_token.to_string(),
        request_id: RequestId::new().0,
        deadline_unix_ms: Deadline::from_now(Duration::from_secs(10)).to_unix_ms(),
        operation: Operation::Hello,
        payload: serde_json::to_value(&hello)
            .map_err(|e| TransportError::LocalIo(format!("ipc encode: {e}")))?,
    };
    let bytes = serde_json::to_vec(&env)
        .map_err(|e| TransportError::LocalIo(format!("ipc encode: {e}")))?;

    let mut guard = conn.lock().unwrap();
    // Handshake has a 10-second deadline; tighten the socket timeout so a
    // stuck peer doesn't block connect() for the full 5-minute default.
    apply_socket_timeouts(&*guard, Duration::from_secs(10));
    FrameWriter::new(&mut *guard)
        .write_frame(&bytes)
        .map_err(frame_err_to_transport)?;
    let resp_bytes = match FrameReader::new(&mut *guard).read_frame() {
        Ok(Some(b)) => b,
        Ok(None) => return Err(TransportError::DaemonUnavailable),
        Err(e) => return Err(frame_err_to_transport(e)),
    };
    drop(guard);

    let resp: ResponseEnvelope = serde_json::from_slice(&resp_bytes)
        .map_err(|e| TransportError::LocalIo(format!("ipc decode: {e}")))?;
    match resp.result {
        ResponseResult::Ok(v) => {
            let ack: HelloAck = serde_json::from_value(v)
                .map_err(|e| TransportError::LocalIo(format!("ipc decode: {e}")))?;
            Ok(ack.daemon_nonce)
        }
        ResponseResult::Err(e) => Err(TransportError::from(e)),
    }
}

#[cfg(all(unix, any(test, feature = "native-ssh")))]
fn frame_err_to_transport(e: FrameError) -> TransportError {
    match e {
        FrameError::Io(_) => TransportError::DaemonUnavailable,
        FrameError::Truncated | FrameError::TooLarge(_) => {
            TransportError::LocalIo(format!("ipc framing: {e}"))
        }
    }
}

/// Apply `SO_RCVTIMEO` and `SO_SNDTIMEO` on `stream` so every read/write
/// becomes a timed call. The kernel returns `EAGAIN`/`EWOULDBLOCK` once the
/// timer elapses, which surfaces as `FrameError::Io` and is mapped to
/// [`TransportError::DaemonUnavailable`] — the same code as a peer that
/// hung up. Without this a wedged daemon would block the parent CLI
/// forever on a single I/O call.
#[cfg(all(unix, any(test, feature = "native-ssh")))]
fn apply_socket_timeouts(stream: &UnixStream, timeout: Duration) {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    let secs = timeout.as_secs() as libc::time_t;
    let extra_nanos = timeout.subsec_nanos() as libc::c_long;
    let tv = libc::timeval {
        tv_sec: secs,
        tv_usec: (extra_nanos / 1000) as libc::suseconds_t,
    };
    // SAFETY: `fd` is a valid open socket; `tv` is a fully-initialised
    // `timeval`. Both `setsockopt` calls write the kernel's timeout into
    // the socket's receive/send side; failures are logged but ignored —
    // a missing timeout just leaves the prior (blocking) behaviour, which
    // is what the caller would have seen before this code existed.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_SNDTIMEO,
            &tv as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
}

#[cfg(all(unix, any(test, feature = "native-ssh")))]
impl RemoteTransport for NativeTransportClient {
    fn test_connection(&self, deadline: Deadline) -> Result<bool, TransportError> {
        let resp = self.exchange(Operation::TestConnection, Value::Null, deadline)?;
        match resp.result {
            ResponseResult::Ok(v) => serde_json::from_value(v)
                .map_err(|e| TransportError::LocalIo(format!("ipc decode: {e}"))),
            ResponseResult::Err(e) => Err(TransportError::from(e)),
        }
    }

    fn run_command(&self, req: &CommandRequest) -> Result<CommandResult, TransportError> {
        let payload = serde_json::to_value(WireCommand {
            command: req.command.clone(),
            timeout: req.timeout.map(|d| d.as_secs()),
        })
        .map_err(|e| TransportError::LocalIo(format!("ipc encode: {e}")))?;
        let resp = self.exchange(Operation::RunCommand, payload, req.deadline)?;
        match resp.result {
            ResponseResult::Ok(v) => {
                let w: WireCommandResult = serde_json::from_value(v)
                    .map_err(|e| TransportError::LocalIo(format!("ipc decode: {e}")))?;
                Ok(CommandResult::from(w))
            }
            ResponseResult::Err(e) => Err(TransportError::from(e)),
        }
    }

    fn upload_file(&self, req: &UploadFileRequest) -> Result<(), TransportError> {
        let payload = serde_json::to_value(WireUploadFile {
            local: req.local.to_string_lossy().into_owned(),
            remote: req.remote.clone(),
        })
        .map_err(|e| TransportError::LocalIo(format!("ipc encode: {e}")))?;
        let resp = self.exchange(Operation::UploadFile, payload, req.deadline)?;
        match resp.result {
            ResponseResult::Ok(_) => Ok(()),
            ResponseResult::Err(e) => Err(TransportError::from(e)),
        }
    }

    fn upload_text(&self, req: &UploadTextRequest) -> Result<(), TransportError> {
        let payload = serde_json::to_value(WireUploadText {
            text: req.text.clone(),
            remote: req.remote.clone(),
        })
        .map_err(|e| TransportError::LocalIo(format!("ipc encode: {e}")))?;
        let resp = self.exchange(Operation::UploadText, payload, req.deadline)?;
        match resp.result {
            ResponseResult::Ok(_) => Ok(()),
            ResponseResult::Err(e) => Err(TransportError::from(e)),
        }
    }

    fn download_file(&self, req: &DownloadFileRequest) -> Result<(), TransportError> {
        let payload = serde_json::to_value(WireDownloadFile {
            remote: req.remote.clone(),
            local: req.local.to_string_lossy().into_owned(),
        })
        .map_err(|e| TransportError::LocalIo(format!("ipc encode: {e}")))?;
        let resp = self.exchange(Operation::DownloadFile, payload, req.deadline)?;
        match resp.result {
            ResponseResult::Ok(_) => Ok(()),
            ResponseResult::Err(e) => Err(TransportError::from(e)),
        }
    }

    fn download_dir(&self, req: &DownloadDirRequest) -> Result<(), TransportError> {
        let payload = serde_json::to_value(WireDownloadDir {
            remote: req.remote.clone(),
            local: req.local.to_string_lossy().into_owned(),
        })
        .map_err(|e| TransportError::LocalIo(format!("ipc encode: {e}")))?;
        let resp = self.exchange(Operation::DownloadDir, payload, req.deadline)?;
        match resp.result {
            ResponseResult::Ok(_) => Ok(()),
            ResponseResult::Err(e) => Err(TransportError::from(e)),
        }
    }
}

// ───────────────────────────── end-to-end test (over IPC, against the real server) ─────────────────────────────

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::transport::contract::test_support::{shared_contract_suite, FakeTransport};
    use crate::transport::ipc::server as real_server;
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    /// Serve one connection: Hello handshake, then dispatch every request onto
    /// `transport` until the client closes.
    fn serve_one_via_real_server(
        listener: std::os::unix::net::UnixListener,
        transport: Arc<dyn RemoteTransport>,
    ) {
        // The real `server` module is the source of truth — this test exists
        // only to verify the client (NativeTransportClient) still round-trips
        // through a server that uses it.
        let (stream, _) = listener.accept().unwrap();
        real_server::serve_one(stream, transport, "", "fake-daemon-nonce");
    }

    #[test]
    fn ipc_transport_passes_shared_contract_suite() {
        // The bound the design calls a hard requirement.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NativeTransportClient>();

        let socket = std::env::temp_dir().join(format!("vcli-ipc-{}.sock", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();

        let handle = thread::spawn(move || {
            serve_one_via_real_server(listener, Arc::new(FakeTransport::ok()));
        });

        let client = NativeTransportClient::connect(&socket, "test-profile", "").unwrap();

        // The step-0 shared suite must pass over the IPC path exactly as it does
        // for the OpenSSH backend.
        shared_contract_suite(&client);

        // Extra: a successful command and a transfer round-trip through the daemon.
        let ok = client
            .run_command(&CommandRequest::untimed("echo hi"))
            .unwrap();
        assert_eq!(ok.exit_status, 0);
        assert_eq!(ok.stdout, "ok");
        client
            .upload_text(&UploadTextRequest::untimed("payload", "/tmp/x"))
            .unwrap();
        assert!(client
            .test_connection(Deadline::from_now(Duration::from_secs(5)))
            .unwrap());

        drop(client);
        handle.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }
}
