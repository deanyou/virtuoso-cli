//! Fake IPC daemon and the synchronous `NativeTransportClient`.
//!
//! This module proves the IPC wiring end-to-end: a `NativeTransportClient`
//! (the business-side client the native backend will use) talks the framed,
//! versioned protocol to a daemon that dispatches onto an in-memory
//! `FakeTransport`. The same `shared_contract_suite` that runs against the
//! OpenSSH backend therefore also runs against the IPC path, which is the gate
//! for step 2.
//!
//! The client is Unix-only because the first transport is a Unix domain socket.
//! Named-pipe (Windows) and TCP variants share this code through the
//! transport-agnostic [`framing`] layer.

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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

// ───────────────────── wire payloads (id/deadline live in the envelope) ─────────────────────

#[derive(Serialize, Deserialize)]
struct WireCommand {
    command: String,
    /// Execution timeout in seconds, carried separately because `Deadline` is
    /// not serializable; the envelope carries the absolute deadline instead.
    timeout: Option<u64>,
}

/// Wire mirror of [`CommandResult`].
///
/// `CommandResult` carries a `std::time::Duration`, which this build's serde
/// configuration does not serialize, so the daemon and client translate through
/// this millisecond-based projection instead of deriving `Serialize` on the
/// contract type.
#[derive(Serialize, Deserialize)]
struct WireCommandResult {
    exit_status: i32,
    stdout: String,
    stderr: String,
    success: bool,
    duration_ms: u128,
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
            duration: Duration::from_millis(w.duration_ms as u64),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct WireUploadFile {
    local: String,
    remote: String,
}

#[derive(Serialize, Deserialize)]
struct WireUploadText {
    text: String,
    remote: String,
}

#[derive(Serialize, Deserialize)]
struct WireDownloadFile {
    remote: String,
    local: String,
}

#[derive(Serialize, Deserialize)]
struct WireDownloadDir {
    remote: String,
    local: String,
}

// ───────────────────────────── client (production-shaped) ─────────────────────────────

#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[cfg(unix)]
pub struct NativeTransportClient {
    conn: Mutex<UnixStream>,
    profile: String,
    auth_token: String,
    /// Nonce issued by the daemon during `Hello`. Every request must echo it so
    /// a daemon restart (new nonce) invalidates this client.
    daemon_nonce: String,
}

#[cfg(unix)]
impl NativeTransportClient {
    /// Connect to a daemon's Unix socket and complete the `Hello` handshake.
    pub fn connect(
        socket_path: &Path,
        profile: &str,
        auth_token: &str,
    ) -> Result<Self, TransportError> {
        let stream =
            UnixStream::connect(socket_path).map_err(|_| TransportError::DaemonUnavailable)?;
        let conn = Mutex::new(stream);
        let nonce = do_handshake(&conn, profile, auth_token)?;
        Ok(Self {
            conn,
            profile: profile.to_string(),
            auth_token: auth_token.to_string(),
            daemon_nonce: nonce,
        })
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

#[cfg(unix)]
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

#[cfg(unix)]
fn frame_err_to_transport(e: FrameError) -> TransportError {
    match e {
        FrameError::Io(_) => TransportError::DaemonUnavailable,
        FrameError::Truncated | FrameError::TooLarge(_) => {
            TransportError::LocalIo(format!("ipc framing: {e}"))
        }
    }
}

#[cfg(unix)]
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

// ───────────────────────────── fake daemon + integration test ─────────────────────────────

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::transport::contract::test_support::{shared_contract_suite, FakeTransport};
    use crate::transport::ipc::messages::IpcError;
    use std::collections::BTreeSet;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::Arc;
    use std::thread;

    /// Serve one connection: Hello handshake, then dispatch every request onto
    /// `transport` until the client closes.
    fn serve_one(stream: UnixStream, transport: Arc<dyn RemoteTransport>) {
        // Two independent handles to the same socket: reads and writes don't
        // fight over a single borrowed stream.
        let read_stream = stream.try_clone().unwrap();
        let mut reader = FrameReader::new(read_stream);
        let mut writer = FrameWriter::new(stream);

        // Hello handshake.
        let hello_frame = reader.read_frame().unwrap().unwrap();
        let hello_env: RequestEnvelope = serde_json::from_slice(&hello_frame).unwrap();
        assert_eq!(hello_env.operation, Operation::Hello);
        let ack = HelloAck {
            server_major: PROTOCOL_MAJOR,
            server_minor: PROTOCOL_MINOR,
            daemon_nonce: "fake-daemon-nonce".to_string(),
            capabilities: BTreeSet::new(),
        };
        let ack_env = ResponseEnvelope {
            request_id: hello_env.request_id,
            result: ResponseResult::Ok(serde_json::to_value(&ack).unwrap()),
        };
        writer
            .write_frame(&serde_json::to_vec(&ack_env).unwrap())
            .unwrap();

        // Dispatch loop.
        while let Some(frame) = reader.read_frame().unwrap() {
            let env: RequestEnvelope = serde_json::from_slice(&frame).unwrap();
            let result = dispatch(&*transport, &env);
            let resp = ResponseEnvelope {
                request_id: env.request_id.clone(),
                result,
            };
            writer
                .write_frame(&serde_json::to_vec(&resp).unwrap())
                .unwrap();
        }
    }

    fn dispatch(transport: &dyn RemoteTransport, env: &RequestEnvelope) -> ResponseResult {
        let deadline = Deadline::from_unix_ms(env.deadline_unix_ms);
        let request_id = env.request_id.clone();
        match &env.operation {
            Operation::RunCommand => {
                let w: WireCommand = match serde_json::from_value(env.payload.clone()) {
                    Ok(w) => w,
                    Err(e) => return ResponseResult::Err(IpcError::Configuration(e.to_string())),
                };
                let req = CommandRequest {
                    id: RequestId(request_id),
                    deadline,
                    command: w.command,
                    timeout: w.timeout.map(Duration::from_secs),
                };
                match transport.run_command(&req) {
                    Ok(r) => ResponseResult::Ok(
                        serde_json::to_value(WireCommandResult::from(&r)).unwrap(),
                    ),
                    Err(e) => ResponseResult::Err(IpcError::from(e)),
                }
            }
            Operation::TestConnection => match transport.test_connection(deadline) {
                Ok(b) => ResponseResult::Ok(serde_json::to_value(b).unwrap()),
                Err(e) => ResponseResult::Err(IpcError::from(e)),
            },
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
            other => ResponseResult::Err(IpcError::UnsupportedOperation(format!("{other:?}"))),
        }
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
            let (stream, _) = listener.accept().unwrap();
            serve_one(stream, Arc::new(FakeTransport::ok()));
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
