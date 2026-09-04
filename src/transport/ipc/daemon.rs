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

        // Acquire the shared connection mutex with a deadline. A long-running
        // request holding the lock must not cause a short-deadline request to
        // wait indefinitely — try_lock polls and bails when the deadline passes.
        let mut guard = lock_with_deadline(&self.conn, &deadline)?;
        // Re-check after acquiring the lock: the deadline may have passed
        // while we were waiting. Don't send a request whose budget is gone.
        if deadline.is_expired() {
            return Err(TransportError::QueueTimeout {
                request: RequestId::new(),
                after_secs: 0,
            });
        }
        let stream = &mut *guard;
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&*stream);
        // Write under the absolute deadline: every partial write re-applies
        // the remaining budget as the socket send timeout, so back-pressure
        // cannot accumulate beyond the total deadline.
        FrameWriter::new(&mut *stream)
            .write_frame_with_deadline(&bytes, Some(deadline.0), move |d| {
                apply_socket_timeouts_fd(fd, d)
            })
            .map_err(frame_err_to_transport)?;
        // Read the full frame under the same absolute deadline — every
        // partial read re-applies the remaining budget, so a multi-segment
        // response cannot reset the timer on every underlying read().
        let resp_bytes = match FrameReader::new(&mut *stream)
            .read_frame_until_with(Some(deadline.0), move |d| {
                apply_socket_timeouts_fd(fd, d)
            }) {
            Ok(Some(b)) => b,
            Ok(None) => return Err(TransportError::DaemonUnavailable),
            Err(e) => return Err(frame_err_to_transport(e)),
        };
        drop(guard);

        serde_json::from_slice(&resp_bytes)
            .map_err(|e| TransportError::LocalIo(format!("ipc decode: {e}")))
    }
}

/// Acquire a mutex, polling with `try_lock` so the wait is bounded by `deadline`.
///
/// `std::sync::Mutex::lock()` blocks indefinitely; a request whose deadline
/// passes while another request holds the lock must fail with `QueueTimeout`
/// rather than wait forever. Polls every 1ms — cheap enough for IPC latency,
/// responsive enough for short deadlines.
#[cfg(all(unix, any(test, feature = "native-ssh")))]
fn lock_with_deadline<'a, T>(
    mutex: &'a Mutex<T>,
    deadline: &Deadline,
) -> Result<std::sync::MutexGuard<'a, T>, TransportError> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::WouldBlock) => {
                if deadline.is_expired() {
                    return Err(TransportError::QueueTimeout {
                        request: RequestId::new(),
                        after_secs: 0,
                    });
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(TransportError::DaemonUnavailable);
            }
        }
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
    // Handshake should complete in well under a second on a healthy daemon.
    // Use a 5s absolute deadline with dynamic socket timeouts.
    let handshake_deadline = std::time::Instant::now() + Duration::from_secs(5);
    let stream = &mut *guard;
    let fd = std::os::unix::io::AsRawFd::as_raw_fd(&*stream);
    FrameWriter::new(&mut *stream)
        .write_frame_with_deadline(&bytes, Some(handshake_deadline), move |d| {
            apply_socket_timeouts_fd(fd, d)
        })
        .map_err(frame_err_to_transport)?;
    let resp_bytes = match FrameReader::new(&mut *stream)
        .read_frame_until_with(Some(handshake_deadline), move |d| {
            apply_socket_timeouts_fd(fd, d)
        }) {
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

/// Set both SO_RCVTIMEO and SO_SNDTIMEO on a raw socket fd. Used inside
/// read/write loops where the `&UnixStream` is already mutably borrowed by
/// the frame reader/writer — capturing just the `RawFd` (Copy) avoids a
/// double-borrow.
#[cfg(all(unix, any(test, feature = "native-ssh")))]
fn apply_socket_timeouts_fd(fd: std::os::unix::io::RawFd, timeout: Duration) {
    let secs = timeout.as_secs() as libc::time_t;
    let extra_nanos = timeout.subsec_nanos() as libc::c_long;
    let tv = libc::timeval {
        tv_sec: secs,
        tv_usec: (extra_nanos / 1000) as libc::suseconds_t,
    };
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

    /// Regression: a short-deadline request must not wait indefinitely for a
    /// long request holding the shared connection mutex. The lock acquisition
    /// itself is bounded by the deadline.
    #[test]
    fn lock_with_deadline_bails_when_deadline_passes() {
        let mutex = Arc::new(Mutex::new(0i32));
        let mutex_clone = mutex.clone();
        // Hold the lock in another thread for 2 seconds.
        let handle = thread::spawn(move || {
            let guard = mutex_clone.lock().unwrap();
            std::thread::sleep(Duration::from_secs(2));
            drop(guard);
        });
        // Give the thread time to acquire the lock.
        std::thread::sleep(Duration::from_millis(100));
        // Try to acquire with a 500ms deadline — must fail in <2s.
        let deadline = Deadline::from_now(Duration::from_millis(500));
        let start = std::time::Instant::now();
        let result = lock_with_deadline(&mutex, &deadline);
        let elapsed = start.elapsed();
        assert!(result.is_err(), "lock should fail while held");
        assert!(
            elapsed < Duration::from_secs(2),
            "lock waited too long: {:?}",
            elapsed
        );
        handle.join().unwrap();
    }

    /// Regression: a short-deadline request must not wait indefinitely for a
    /// long request holding the shared connection mutex. End-to-end test over
    /// a real IPC connection with a slow server-side transport.
    #[test]
    fn short_deadline_request_times_out_while_lock_held() {
        use crate::transport::contract::test_support::FakeTransport;
        use crate::transport::contract::{CommandRequest, CommandResult, DownloadDirRequest, DownloadFileRequest, RemoteTransport, UploadFileRequest, UploadTextRequest};

        /// Wraps FakeTransport but sleeps 2s on test_connection to hold the lock.
        struct SlowTransport(FakeTransport);
        impl RemoteTransport for SlowTransport {
            fn test_connection(&self, deadline: Deadline) -> Result<bool, TransportError> {
                std::thread::sleep(Duration::from_secs(2));
                self.0.test_connection(deadline)
            }
            fn run_command(&self, req: &CommandRequest) -> Result<CommandResult, TransportError> {
                self.0.run_command(req)
            }
            fn upload_file(&self, req: &UploadFileRequest) -> Result<(), TransportError> {
                self.0.upload_file(req)
            }
            fn upload_text(&self, req: &UploadTextRequest) -> Result<(), TransportError> {
                self.0.upload_text(req)
            }
            fn download_file(&self, req: &DownloadFileRequest) -> Result<(), TransportError> {
                self.0.download_file(req)
            }
            fn download_dir(&self, req: &DownloadDirRequest) -> Result<(), TransportError> {
                self.0.download_dir(req)
            }
        }

        let socket = std::env::temp_dir().join(format!("vcli-ipc-lock-{}.sock", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();

        let server_handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            real_server::serve_one(stream, Arc::new(SlowTransport(FakeTransport::ok())), "", "n");
        });

        let client = Arc::new(NativeTransportClient::connect(&socket, "test-profile", "").unwrap());

        // Thread A: long request (will hold the mutex for ~2s on server side).
        let client_a = client.clone();
        let handle_a = thread::spawn(move || {
            let _ = client_a.test_connection(Deadline::from_now(Duration::from_secs(10)));
        });
        // Give thread A time to acquire the mutex and send its request.
        std::thread::sleep(Duration::from_millis(200));

        // Thread B: short deadline — must time out in ~1s, not wait for A's 2s.
        let start = std::time::Instant::now();
        let result = client.test_connection(Deadline::from_now(Duration::from_secs(1)));
        let elapsed = start.elapsed();

        assert!(result.is_err(), "short-deadline request should fail");
        assert!(
            elapsed < Duration::from_secs(2),
            "short-deadline request waited too long: {:?}",
            elapsed
        );

        handle_a.join().unwrap();
        drop(client);
        server_handle.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }

    /// Regression: a response that arrives in slow chunks must be bounded by
    /// the absolute deadline, not reset on every underlying read().
    #[test]
    fn slow_chunked_response_hits_deadline() {
        let socket = std::env::temp_dir().join(format!("vcli-ipc-chunk-{}.sock", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();

        // Server: handshake, then for every request write the frame prefix and
        // 1 byte of payload every 500ms — a 4-byte response takes ~2s, which
        // exceeds the 1s deadline.
        let server_handle = thread::spawn(move || {
            use std::io::Write;
            let (mut stream, _) = listener.accept().unwrap();
            let _hello = {
                let mut reader = FrameReader::new(&mut stream);
                reader.read_frame().unwrap().unwrap()
            };
            let ack = serde_json::json!({
                "request_id": "handshake",
                "result": {"ok": {
                    "server_major": PROTOCOL_MAJOR,
                    "server_minor": PROTOCOL_MINOR,
                    "daemon_nonce": "n",
                    "capabilities": []
                }},
            });
            FrameWriter::new(&mut stream).write_frame(&serde_json::to_vec(&ack).unwrap()).unwrap();
            loop {
                let req = {
                    let mut reader = FrameReader::new(&mut stream);
                    reader.read_frame()
                };
                match req {
                    Ok(Some(_)) => {
                        // Write 4-byte length prefix (payload = 4 bytes).
                        stream.write_all(&4u32.to_be_bytes()).unwrap();
                        stream.flush().unwrap();
                        // Write 1 byte every 500ms — 4 bytes = 2s total.
                        for _ in 0..4 {
                            std::thread::sleep(Duration::from_millis(500));
                            if stream.write_all(&[b'x']).is_err() {
                                return;
                            }
                            stream.flush().unwrap();
                        }
                    }
                    _ => break,
                }
            }
        });

        let client = NativeTransportClient::connect(&socket, "test-profile", "").unwrap();

        // 1s deadline — the slow 2s response must be cut off at ~1s.
        let start = std::time::Instant::now();
        let result = client.test_connection(Deadline::from_now(Duration::from_secs(1)));
        let elapsed = start.elapsed();

        assert!(result.is_err(), "slow response should hit deadline");
        assert!(
            elapsed < Duration::from_secs(2),
            "deadline not enforced: waited {:?}",
            elapsed
        );

        drop(client);
        server_handle.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }

    /// Regression: a frame header that arrives just before the deadline must
    /// not cause the next read to block for a full fresh socket timeout.
    /// The read timeout must be re-applied using the *remaining* budget.
    ///
    /// Scenario: 1s deadline, server sends a 4-byte frame header at 0.9s
    /// then stops. Before the fix, the second read could block ~1s more
    /// (total ~1.9s). After the fix, the second read gets ~0.1s timeout.
    #[test]
    fn frame_header_near_deadline_then_stop() {
        let socket = std::env::temp_dir().join(format!("vcli-ipc-hdr-{}.sock", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();

        let server_handle = thread::spawn(move || {
            use std::io::Write;
            let (mut stream, _) = listener.accept().unwrap();
            // Handshake.
            let _hello = {
                let mut reader = FrameReader::new(&mut stream);
                reader.read_frame().unwrap().unwrap()
            };
            let ack = serde_json::json!({
                "request_id": "h",
                "result": {"ok": {
                    "server_major": PROTOCOL_MAJOR,
                    "server_minor": PROTOCOL_MINOR,
                    "daemon_nonce": "n",
                    "capabilities": []
                }},
            });
            FrameWriter::new(&mut stream).write_frame(&serde_json::to_vec(&ack).unwrap()).unwrap();
            // For each request: wait 0.9s, send frame header (100-byte payload),
            // then stop sending — the client must time out on the second read.
            loop {
                let req = {
                    let mut reader = FrameReader::new(&mut stream);
                    reader.read_frame()
                };
                match req {
                    Ok(Some(_)) => {
                        std::thread::sleep(Duration::from_millis(900));
                        // 4-byte big-endian length = 100, then no payload.
                        if stream.write_all(&100u32.to_be_bytes()).is_err() {
                            break;
                        }
                        stream.flush().unwrap();
                        // Stop sending payload. Loop back to read — when the
                        // client times out and drops the connection, read_frame
                        // returns EOF and we exit.
                    }
                    _ => break,
                }
            }
        });

        let client = NativeTransportClient::connect(&socket, "test-profile", "").unwrap();

        let start = std::time::Instant::now();
        let result = client.test_connection(Deadline::from_now(Duration::from_secs(1)));
        let elapsed = start.elapsed();

        assert!(result.is_err(), "should time out waiting for payload");
        // With dynamic timeouts: 0.9s (header) + ~0.1s (second read) = ~1.0s.
        // Without it: 0.9s + 1.0s = ~1.9s. Allow 1.3s for scheduling jitter.
        assert!(
            elapsed < Duration::from_millis(1300),
            "deadline not enforced on second read: waited {:?}",
            elapsed
        );

        drop(client);
        server_handle.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }

    /// Regression: a large request to a peer that never reads must hit the
    /// write deadline, not block forever once the socket send buffer fills.
    #[test]
    fn large_request_to_nonreading_peer_hits_deadline() {
        use crate::transport::contract::RequestId;
        use std::sync::mpsc;

        let socket = std::env::temp_dir().join(format!("vcli-ipc-wr-{}.sock", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();

        // Channel keeps the server alive without reading the socket. The server
        // blocks on recv() until the test tells it to exit — it never drains the
        // receive buffer, so the client's send buffer fills and write() blocks.
        let (tx, rx) = mpsc::channel::<()>();

        let server_handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            // Handshake only.
            let mut reader = FrameReader::new(&stream);
            let _hello = reader.read_frame().unwrap().unwrap();
            let mut writer = FrameWriter::new(&stream);
            let ack = serde_json::json!({
                "request_id": "h",
                "result": {"ok": {
                    "server_major": PROTOCOL_MAJOR,
                    "server_minor": PROTOCOL_MINOR,
                    "daemon_nonce": "n",
                    "capabilities": []
                }},
            });
            writer.write_frame(&serde_json::to_vec(&ack).unwrap()).unwrap();
            // Block here — do NOT read any request frames. The client's send
            // buffer fills and write() blocks on back-pressure.
            let _ = rx.recv();
        });

        let client = NativeTransportClient::connect(&socket, "test-profile", "").unwrap();

        // Explicitly cap the client's send buffer at 8 KiB so a 1 MiB upload is
        // guaranteed to block in write() — no reliance on the system default.
        // (Linux doubles the value for bookkeeping, so actual is ~16 KiB.)
        {
            use std::os::unix::io::AsRawFd;
            let guard = client.conn.lock().unwrap();
            let fd = guard.as_raw_fd();
            let bufsize = 8192i32;
            let rc = unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    &bufsize as *const _ as *const libc::c_void,
                    std::mem::size_of::<i32>() as libc::socklen_t,
                )
            };
            assert_eq!(rc, 0, "setsockopt(SO_SNDBUF) failed — test cannot guarantee back-pressure");
        }

        // 1 MiB text upload — far larger than the 8 KiB send buffer, so the
        // write will block once the buffer fills. Use an explicit 1s deadline.
        let big_text = "x".repeat(1024 * 1024);
        let req = crate::transport::contract::UploadTextRequest {
            id: RequestId::new(),
            deadline: Deadline::from_now(Duration::from_secs(1)),
            text: big_text,
            remote: "/tmp/big".into(),
        };

        let start = std::time::Instant::now();
        let result = client.upload_text(&req);
        let elapsed = start.elapsed();

        assert!(result.is_err(), "large write to non-reading peer should fail");
        // Must be close to the 1s budget — not instant (which would mean no
        // back-pressure and a fake pass) and not far over it.
        assert!(
            elapsed >= Duration::from_millis(500),
            "write returned too fast ({:?}) — no back-pressure, test is fake",
            elapsed
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "write deadline not enforced: waited {:?}",
            elapsed
        );

        drop(client);
        let _ = tx.send(());
        server_handle.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }
}
