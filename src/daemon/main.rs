use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::io::AsRawFd;
use std::process;

const STX: u8 = 0x02;
const NAK: u8 = 0x15;
const RS: u8 = 0x1e;

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

    set_nonblocking_stdin();

    let listener = TcpListener::bind(format!("{host}:{port}")).unwrap_or_else(|e| {
        eprintln!("failed to bind {host}:{port}: {e}");
        process::exit(1);
    });

    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    // Print actual port so bridge.il can read it (important when port=0 was passed)
    eprintln!("PORT:{actual_port}");
    eprintln!("[virtuoso-daemon] listening on {host}:{actual_port}");

    for stream in listener.incoming() {
        match stream {
            Ok(conn) => {
                if let Err(e) = handle_connection(conn) {
                    eprintln!("[virtuoso-daemon] error: {e}");
                }
            }
            Err(e) => {
                eprintln!("[virtuoso-daemon] accept error: {e}");
            }
        }
    }
}

fn handle_connection(mut conn: TcpStream) -> io::Result<()> {
    let mut req_bytes = Vec::new();
    conn.read_to_end(&mut req_bytes)?;

    let req: SkillRequest = serde_json::from_slice(&req_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid json: {e}")))?;

    let timeout = req.timeout.unwrap_or(30);

    let stdout = io::stdout();
    let mut out = stdout.lock();
    out.write_all(req.skill.as_bytes())?;
    out.flush()?;

    let result = read_until_delimiter(timeout)?;

    conn.write_all(&result)?;
    let _ = conn.shutdown(std::net::Shutdown::Both);

    Ok(())
}

fn read_until_delimiter(timeout_secs: u64) -> io::Result<Vec<u8>> {
    let stdin = io::stdin();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    let mut buf = Vec::new();
    let mut started = false;
    let mut one_byte = [0u8; 1];

    loop {
        if std::time::Instant::now() > deadline {
            return Ok(vec![
                NAK, b'T', b'i', b'm', b'e', b'o', b'u', b't', b'E', b'r', b'r', b'o', b'r', RS,
            ]);
        }

        let n = match stdin.lock().read(&mut one_byte) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            Err(e) => return Err(e),
        };
        if n == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }

        let ch = one_byte[0];

        if !started {
            if ch == STX || ch == NAK {
                started = true;
                buf.push(ch);
            }
            continue;
        }

        if ch == RS {
            break;
        }
        buf.push(ch);
    }

    Ok(buf)
}

fn set_nonblocking_stdin() {
    let fd = io::stdin().lock().as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

#[derive(serde::Deserialize)]
struct SkillRequest {
    skill: String,
    timeout: Option<u64>,
}
