//! Wire framing for the transport IPC protocol.
//!
//! Every message is a UTF-8 JSON document preceded by a four-byte big-endian
//! length prefix. A single frame may not exceed [`MAX_FRAME_SIZE`] (8 MiB);
//! file contents travel out of band, so this bound is a hard safety rail
//! against a malformed or hostile peer, not a throughput limit.
//!
//! The framing layer is deliberately transport-agnostic: it operates on any
//! `std::io::Read` / `std::io::Write`, so the same code serves Unix domain
//! sockets, named pipes, and loopback TCP.

#![allow(dead_code)]

use std::io::{self, Read, Write};

/// Hard ceiling on a single IPC frame, in bytes (8 MiB).
///
/// Mirrors the design's "8 MiB maximum frame". File contents are never carried
/// in frames, so a frame this large always indicates a bug or an attack.
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

/// Length of the big-endian size prefix that precedes every frame.
pub const FRAME_PREFIX_LEN: usize = 4;

/// Protocol version negotiated during the `Hello` handshake.
///
/// `major` must match exactly between client and daemon; a mismatch is a hard
/// `ProtocolMismatch`. `minor` is negotiated downward via capabilities.
pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 0;

/// Failure while reading or writing a framed stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// A payload exceeded [`MAX_FRAME_SIZE`].
    TooLarge(usize),
    /// The underlying stream ended before a complete frame arrived.
    Truncated,
    /// An I/O error on the underlying stream.
    Io(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::TooLarge(n) => {
                write!(f, "ipc frame too large: {n} > {MAX_FRAME_SIZE}")
            }
            FrameError::Truncated => write!(f, "ipc stream ended mid-frame"),
            FrameError::Io(m) => write!(f, "ipc transport io error: {m}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e.to_string())
    }
}

/// Encode `payload` as a length-prefixed frame.
///
/// Returns the four-byte big-endian length followed by the payload. Rejects a
/// payload larger than [`MAX_FRAME_SIZE`] before writing anything, so a bad
/// caller can never place an oversized frame on the wire.
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.len() > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let mut out = Vec::with_capacity(FRAME_PREFIX_LEN + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// A streaming frame reader over any [`Read`].
///
/// Buffers partial frames so callers read one complete message at a time,
/// independent of how the OS delivers bytes. Returns `None` at a clean EOF.
pub struct FrameReader<R: Read> {
    inner: R,
    buf: Vec<u8>,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::with_capacity(MAX_FRAME_SIZE + FRAME_PREFIX_LEN),
        }
    }

    /// Read the next complete frame payload, or `None` at a clean EOF.
    pub fn read_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        self.read_frame_until(None)
    }

    /// Like [`read_frame`], but aborts with `FrameError::Io("deadline exceeded")`
    /// if the absolute `deadline` passes before the frame is complete. Each
    /// underlying `read` is bounded by the socket timeout, but a multi-segment
    /// response can reset that timer on every read — this method enforces a
    /// single absolute deadline across the whole frame.
    pub fn read_frame_until(
        &mut self,
        deadline: Option<std::time::Instant>,
    ) -> Result<Option<Vec<u8>>, FrameError> {
        loop {
            if let Some(d) = deadline {
                if std::time::Instant::now() >= d {
                    return Err(FrameError::Io("ipc read deadline exceeded".into()));
                }
            }
            if self.buf.len() >= FRAME_PREFIX_LEN {
                let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]])
                    as usize;
                if len > MAX_FRAME_SIZE {
                    return Err(FrameError::TooLarge(len));
                }
                let total = FRAME_PREFIX_LEN + len;
                if self.buf.len() >= total {
                    let payload = self.buf[FRAME_PREFIX_LEN..total].to_vec();
                    self.buf.drain(..total);
                    return Ok(Some(payload));
                }
                // Prefix says more bytes are coming; read again.
            }
            let mut chunk = [0u8; 8192];
            let n = self.inner.read(&mut chunk)?;
            if n == 0 {
                // EOF. A non-empty residual buffer means a partial frame.
                return if self.buf.is_empty() {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

/// A streaming frame writer over any [`Write`].
pub struct FrameWriter<W: Write> {
    inner: W,
}

impl<W: Write> FrameWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Encode `payload` and write it to the underlying stream.
    pub fn write_frame(&mut self, payload: &[u8]) -> Result<(), FrameError> {
        let framed = encode_frame(payload)?;
        self.inner.write_all(&framed)?;
        self.inner.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A reader that returns at most `chunk` bytes per `read` call, to force
    /// partial-frame and split-prefix handling.
    struct ChunkedReader<'a> {
        data: &'a [u8],
        pos: usize,
        chunk: usize,
    }
    impl<'a> ChunkedReader<'a> {
        fn new(data: &'a [u8], chunk: usize) -> Self {
            Self {
                data,
                pos: 0,
                chunk,
            }
        }
    }
    impl<'a> Read for ChunkedReader<'a> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let end = (self.pos + self.chunk).min(self.data.len());
            let n = (end - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn round_trips_arbitrary_payload() {
        let payload = b"hello world, this is a frame";
        let framed = encode_frame(payload).unwrap();
        let len = u32::from_be_bytes([framed[0], framed[1], framed[2], framed[3]]);
        assert_eq!(len as usize, payload.len());
        assert_eq!(&framed[FRAME_PREFIX_LEN..], payload);
    }

    #[test]
    fn empty_payload_is_allowed() {
        let framed = encode_frame(b"").unwrap();
        assert_eq!(framed.len(), FRAME_PREFIX_LEN);
        let len = u32::from_be_bytes([framed[0], framed[1], framed[2], framed[3]]);
        assert_eq!(len, 0);
    }

    #[test]
    fn rejects_oversize_payload() {
        let big = vec![0u8; MAX_FRAME_SIZE + 1];
        match encode_frame(&big) {
            Err(FrameError::TooLarge(n)) => assert_eq!(n, MAX_FRAME_SIZE + 1),
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn accepts_exactly_max_size() {
        let big = vec![0u8; MAX_FRAME_SIZE];
        assert!(encode_frame(&big).is_ok());
    }

    #[test]
    fn stream_reader_reassembles_split_writes() {
        let payload = b"split me across reads";
        let framed = encode_frame(payload).unwrap();
        let mut reader = FrameReader::new(ChunkedReader::new(&framed, 3));
        assert_eq!(reader.read_frame().unwrap().unwrap(), payload);
        assert!(reader.read_frame().unwrap().is_none());
    }

    #[test]
    fn stream_reader_handles_concatenated_frames() {
        let a = encode_frame(b"first").unwrap();
        let b = encode_frame(b"second").unwrap();
        let mut all = Vec::new();
        all.extend_from_slice(&a);
        all.extend_from_slice(&b);
        let mut reader = FrameReader::new(Cursor::new(all));
        assert_eq!(reader.read_frame().unwrap().unwrap(), b"first");
        assert_eq!(reader.read_frame().unwrap().unwrap(), b"second");
        assert!(reader.read_frame().unwrap().is_none());
    }

    #[test]
    fn truncated_frame_is_an_error() {
        let framed = encode_frame(b"incomplete").unwrap();
        let partial = &framed[..framed.len() - 5];
        let mut reader = FrameReader::new(Cursor::new(partial.to_vec()));
        assert!(matches!(reader.read_frame(), Err(FrameError::Truncated)));
    }

    #[test]
    fn writer_and_reader_agree() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = FrameWriter::new(&mut buf);
            w.write_frame(b"one").unwrap();
            w.write_frame(b"two").unwrap();
        }
        let mut reader = FrameReader::new(Cursor::new(buf));
        assert_eq!(reader.read_frame().unwrap().unwrap(), b"one");
        assert_eq!(reader.read_frame().unwrap().unwrap(), b"two");
    }

    #[test]
    fn oversize_claim_is_rejected_before_buffering() {
        // A prefix claiming 9 MiB must fail even though only the prefix arrived.
        let mut prefix = Vec::new();
        prefix.extend_from_slice(&(MAX_FRAME_SIZE as u32 + 1).to_be_bytes());
        let mut reader = FrameReader::new(Cursor::new(prefix));
        assert!(matches!(
            reader.read_frame(),
            Err(FrameError::TooLarge(n)) if n == MAX_FRAME_SIZE + 1
        ));
    }
}
