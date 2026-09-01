//! Versioned IPC protocol between business-side `RemoteTransport` clients and the
//! transport daemon.
//!
//! This module owns the wire format only. It exposes no russh, Tokio, SSH, or
//! daemon-internal types. See [`framing`] for the byte layout and [`messages`]
//! for the request/response vocabulary.

/// Synchronous `NativeTransportClient` (the business-side client of the
/// versioned IPC protocol). Compiled only on Unix (domain sockets) and
/// either under `#[cfg(test)]` or with the `native-ssh` feature: the client
/// is part of the native backend and absent from feature-stripped builds.
#[cfg(all(unix, any(test, feature = "native-ssh")))]
pub mod daemon;

pub mod framing;
pub mod messages;
/// Server half of the IPC protocol. Compiled only on Unix (domain sockets)
/// and either under `#[cfg(test)]` or with the `native-ssh` feature — the
/// real daemon is feature-gated, but the test suite exercises the same code
/// without requiring the feature.
#[cfg(all(unix, any(test, feature = "native-ssh")))]
pub mod server;
