//! Authentication-method selection for the native (russh-based) transport.
//!
//! Pure policy logic — it lands before the russh client that will enumerate the
//! server's offered methods and call [`select`]. No live connection required.

// Consumed by the native client (later increment); some variants are
// unreferenced until then. Mirrors `contract.rs`.
#![allow(dead_code)]

use std::path::PathBuf;

/// An SSH authentication method, as named on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    None,
    /// Path to the client identity (private key).
    PublicKey(PathBuf),
    Password,
    KeyboardInteractive,
    GssApi,
    HostBased,
}

/// Payload-free discriminator, used for preference comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    None,
    PublicKey,
    Password,
    KeyboardInteractive,
    GssApi,
    HostBased,
}

impl AuthMethod {
    pub fn kind(&self) -> AuthKind {
        match self {
            AuthMethod::None => AuthKind::None,
            AuthMethod::PublicKey(_) => AuthKind::PublicKey,
            AuthMethod::Password => AuthKind::Password,
            AuthMethod::KeyboardInteractive => AuthKind::KeyboardInteractive,
            AuthMethod::GssApi => AuthKind::GssApi,
            AuthMethod::HostBased => AuthKind::HostBased,
        }
    }

    /// The name the SSH protocol uses to advertise/select the method.
    pub fn ssh_name(&self) -> &'static str {
        match self {
            AuthMethod::None => "none",
            AuthMethod::PublicKey(_) => "publickey",
            AuthMethod::Password => "password",
            AuthMethod::KeyboardInteractive => "keyboard-interactive",
            AuthMethod::GssApi => "gssapi-with-mic",
            AuthMethod::HostBased => "hostbased",
        }
    }
}

impl std::fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.ssh_name())
    }
}

/// Preference order, highest first: key-based auth wins, then interactive, then
/// password. Mirrors what `SSHRunner` already does (key auth, fallback to
/// interactive/password) without the ControlMaster retry machinery.
pub fn preference_order() -> &'static [AuthKind] {
    &[
        AuthKind::PublicKey,
        AuthKind::KeyboardInteractive,
        AuthKind::Password,
        AuthKind::GssApi,
        AuthKind::HostBased,
        AuthKind::None,
    ]
}

/// Choose the highest-preference method the client can actually perform from the
/// methods the server offered.
///
/// `client_has_key` gates `PublicKey`: a server may offer `publickey` while the
/// client has no usable identity, in which case it must fall through rather than
/// fail the handshake on a method it cannot satisfy.
pub fn select(offered: &[AuthMethod], client_has_key: bool) -> Option<AuthMethod> {
    let usable: Vec<AuthKind> = offered
        .iter()
        .filter(|m| match m.kind() {
            AuthKind::PublicKey => client_has_key,
            _ => true,
        })
        .map(|m| m.kind())
        .collect();

    preference_order()
        .iter()
        .find(|pref| usable.contains(pref))
        .and_then(|kind| offered.iter().find(|m| m.kind() == *kind).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk() -> AuthMethod {
        AuthMethod::PublicKey(PathBuf::from("/tmp/id_ed25519"))
    }

    #[test]
    fn prefers_public_key_when_offered_and_available() {
        let offered = vec![pk(), AuthMethod::Password];
        assert_eq!(select(&offered, true), Some(pk()));
    }

    #[test]
    fn falls_through_to_password_when_key_not_offered() {
        let offered = vec![AuthMethod::Password, AuthMethod::KeyboardInteractive];
        assert_eq!(
            select(&offered, true),
            Some(AuthMethod::KeyboardInteractive)
        );
    }

    #[test]
    fn skips_public_key_when_client_has_no_identity() {
        let offered = vec![pk(), AuthMethod::Password];
        assert_eq!(select(&offered, false), Some(AuthMethod::Password));
    }

    #[test]
    fn none_offered_and_none_usable() {
        let offered: Vec<AuthMethod> = vec![];
        assert_eq!(select(&offered, true), None);
    }

    #[test]
    fn ssh_names_round_trip() {
        assert_eq!(
            AuthMethod::PublicKey(PathBuf::new()).ssh_name(),
            "publickey"
        );
        assert_eq!(AuthMethod::GssApi.ssh_name(), "gssapi-with-mic");
    }
}
