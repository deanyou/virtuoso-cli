use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

const NAK: u8 = 0x15;
const RS: u8 = 0x1e;

static CALLS: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);
static START: OnceLock<std::time::Instant> = OnceLock::new();

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: virtuoso-daemon <host> <port>");
        process::exit(1);
    }

    let host = &args[1];
    let port: u16 = args[2].parse().unwrap_or_else(|_| {
        eprintln!("invalid port: {}", args[2]);
        process::exit(1);
    });

    let listener = TcpListener::bind(format!("{host}:{port}")).unwrap_or_else(|e| {
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

    for stream in listener.incoming() {
        match stream {
            Ok(conn) => {
                if let Err(e) = handle_connection(conn, cb_port, actual_port) {
                    eprintln!("[virtuoso-daemon] error: {e}");
                }
            }
            Err(e) => {
                eprintln!("[virtuoso-daemon] accept error: {e}");
            }
        }
    }
}

fn handle_connection(mut conn: TcpStream, cb_port: u16, actual_port: u16) -> io::Result<()> {
    CALLS.fetch_add(1, Ordering::Relaxed);

    let mut req_bytes = Vec::new();
    conn.read_to_end(&mut req_bytes)?;

    let req: SkillRequest = serde_json::from_slice(&req_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {e}")))?;

    let timeout = req.timeout.unwrap_or(30);

    // Clean up any stale callback files before sending the request
    let (data_file, done_file) = callback_files(cb_port);
    let _ = std::fs::remove_file(&data_file);
    let _ = std::fs::remove_file(&done_file);

    let stdout = io::stdout();
    let mut out = stdout.lock();
    out.write_all(req.skill.as_bytes())?;
    out.flush()?;

    let result = read_callback_file(cb_port, timeout)?;

    if result.first() == Some(&NAK) {
        ERRORS.fetch_add(1, Ordering::Relaxed);
    }
    write_stats(actual_port);

    conn.write_all(&result)?;
    let _ = conn.shutdown(std::net::Shutdown::Both);

    Ok(())
}

fn write_stats(port: u16) {
    let uptime = START.get().map(|t| t.elapsed().as_secs()).unwrap_or(0);
    let json = format!(
        r#"{{"calls":{},"errors":{},"uptime_secs":{}}}"#,
        CALLS.load(Ordering::Relaxed),
        ERRORS.load(Ordering::Relaxed),
        uptime
    );
    let _ = std::fs::write(format!("/tmp/.ramic_stats_{port}"), json);
}

fn callback_files(cb_port: u16) -> (String, String) {
    (
        format!("/tmp/.ramic_cb_{cb_port}"),
        format!("/tmp/.ramic_cb_{cb_port}.done"),
    )
}

/// Poll for the temp file written by RBSendCallback in ramic_bridge.il.
/// RBSendCallback writes data to /tmp/.ramic_cb_{port+1} then creates
/// /tmp/.ramic_cb_{port+1}.done as an atomic completion marker.
fn read_callback_file(cb_port: u16, timeout_secs: u64) -> io::Result<Vec<u8>> {
    let (data_file, done_file) = callback_files(cb_port);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        if std::time::Instant::now() > deadline {
            return Ok(vec![
                NAK, b'T', b'i', b'm', b'e', b'o', b'u', b't', b'E', b'r', b'r', b'o', b'r',
            ]);
        }

        if Path::new(&done_file).exists() {
            match std::fs::read(&data_file) {
                Ok(mut data) => {
                    let _ = std::fs::remove_file(&data_file);
                    let _ = std::fs::remove_file(&done_file);
                    // Strip trailing RS marker written by RBSendCallback's sprintf %c
                    if data.last() == Some(&RS) {
                        data.pop();
                    }
                    return Ok(data);
                }
                Err(_) => {
                    // done_file appeared but data_file not readable yet — retry
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[derive(serde::Deserialize)]
struct SkillRequest {
    skill: String,
    timeout: Option<u64>,
}
