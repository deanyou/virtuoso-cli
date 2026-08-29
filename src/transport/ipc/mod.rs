//! Versioned IPC protocol between business-side `RemoteTransport` clients and the
//! transport daemon.
//!
//! This module owns the wire format only. It exposes no russh, Tokio, SSH, or
//! daemon-internal types. See [`framing`] for the byte layout and [`messages`]
//! for the request/response vocabulary.

pub mod daemon;
pub mod framing;
pub mod messages;
