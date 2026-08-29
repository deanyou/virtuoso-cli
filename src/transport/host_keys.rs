//! Host-key verification policy for the native (russh-based) transport.
//!
//! This is the client-side half of step 3's "host-key verification, and
//! authentication" — it is pure policy logic and needs neither a live SSH
//! server nor the `russh` dependency, so it can land before the native client
//! that will consume it.
//!
//! It implements the OpenSSH `known_hosts` contract:
//! - a key that matches a stored entry is `Trusted`;
//! - a key with no stored entry is `Unknown` (the caller must decide);
//! - a key that *mismatches* a stored entry is `Changed` (potential MITM, must
//!   be rejected);
//! - a key matching a `@revoked` entry is `Revoked`.
//!
//! Both plaintext host entries and hashed (`|1|salt|hash`) entries are
//! supported for lookup. Writing emits plaintext entries; hashed *writing*
//! would require a salted HMAC with a fresh salt, which is supported but the
//! default `trust` keeps entries readable for fixtures.

// The native client that consumes this module lands in a later increment; until
// then some variants/branches are unreferenced. Mirrors `contract.rs`.
#![allow(dead_code)]

use base64::Engine;
use std::path::{Path, PathBuf};

/// SSH public-key algorithm as it appears in a `known_hosts` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyType {
    SshEd25519,
    SshRsa,
    EcdsaSha2Nistp256,
    EcdsaSha2Nistp384,
    EcdsaSha2Nistp521,
    SkSshEd25519,
    SkSshRsa,
    Other(String),
}

impl KeyType {
    /// Parse from the token OpenSSH writes in the second column.
    pub fn from_known_hosts_token(token: &str) -> Self {
        match token {
            "ssh-ed25519" => KeyType::SshEd25519,
            "ssh-rsa" => KeyType::SshRsa,
            "ecdsa-sha2-nistp256" => KeyType::EcdsaSha2Nistp256,
            "ecdsa-sha2-nistp384" => KeyType::EcdsaSha2Nistp384,
            "ecdsa-sha2-nistp521" => KeyType::EcdsaSha2Nistp521,
            "sk-ssh-ed25519@openssh.com" => KeyType::SkSshEd25519,
            "sk-ssh-rsa@openssh.com" => KeyType::SkSshRsa,
            other => KeyType::Other(other.to_string()),
        }
    }

    /// Render back to the token OpenSSH expects.
    pub fn as_known_hosts_token(&self) -> String {
        match self {
            KeyType::SshEd25519 => "ssh-ed25519".into(),
            KeyType::SshRsa => "ssh-rsa".into(),
            KeyType::EcdsaSha2Nistp256 => "ecdsa-sha2-nistp256".into(),
            KeyType::EcdsaSha2Nistp384 => "ecdsa-sha2-nistp384".into(),
            KeyType::EcdsaSha2Nistp521 => "ecdsa-sha2-nistp521".into(),
            KeyType::SkSshEd25519 => "sk-ssh-ed25519@openssh.com".into(),
            KeyType::SkSshRsa => "sk-ssh-rsa@openssh.com".into(),
            KeyType::Other(s) => s.clone(),
        }
    }
}

/// Result of checking a presented host key against the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// Matches a stored entry. Safe to proceed.
    Trusted,
    /// No stored entry. The caller decides (prompt / trust-on-first-use).
    Unknown,
    /// Matches a host pattern but the key differs — possible MITM. Reject.
    Changed {
        stored_key_type: KeyType,
        stored_key: String,
    },
    /// Matches a `@revoked` entry. Reject unconditionally.
    Revoked,
}

#[derive(Debug, Clone)]
struct Entry {
    revoked: bool,
    /// Each hostspec is either `host`, `[host]:port`, `*`, or hashed `|1|..|..`.
    hosts: Vec<HostPattern>,
    key_type: KeyType,
    key: String,
}

#[derive(Debug, Clone)]
enum HostPattern {
    Any,
    Plain { host: String, port: Option<u16> },
    Hashed { salt: Vec<u8>, hash: Vec<u8> },
}

impl HostPattern {
    /// Does this pattern cover `(host, port)`?
    fn matches(&self, host: &str, port: Option<u16>) -> bool {
        match self {
            HostPattern::Any => true,
            HostPattern::Plain { host: h, port: p } => {
                // A bare host entry matches regardless of port; a `[h]:p` entry
                // is port-specific.
                h == host && (p.is_none() || *p == port)
            }
            HostPattern::Hashed { .. } => false, // resolved by hashed_key_matches
        }
    }

    /// For hashed patterns we compare the HMAC over `host_string` against the
    /// stored hash. OpenSSH hashes either `host` or `[host]:port`.
    fn hashed_matches(&self, host_string: &[u8]) -> bool {
        match self {
            HostPattern::Hashed { salt, hash } => hmac_sha1(salt, host_string) == *hash,
            _ => false,
        }
    }
}

/// An in-memory, optionally file-backed `known_hosts` store.
#[derive(Debug, Clone, Default)]
pub struct KnownHosts {
    path: Option<PathBuf>,
    entries: Vec<Entry>,
}

impl KnownHosts {
    /// Parse from a `known_hosts` file. Missing file yields an empty store
    /// (so first connection is `Unknown`, never an error).
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(KnownHosts {
                    path: Some(path.to_path_buf()),
                    entries: Vec::new(),
                })
            }
            Err(e) => return Err(e),
        };
        Ok(KnownHosts {
            path: Some(path.to_path_buf()),
            entries: parse_known_hosts(&text),
        })
    }

    /// An empty, path-less store — convenient for tests and TOFU decisions.
    pub fn memory() -> Self {
        KnownHosts {
            path: None,
            entries: Vec::new(),
        }
    }

    /// Check a presented key against stored entries.
    pub fn check(
        &self,
        host: &str,
        port: Option<u16>,
        key_type: &KeyType,
        key: &str,
    ) -> Verification {
        // OpenSSH hashes the host string as either `host` or `[host]:port`.
        let plain_string = host.to_string();
        let port_string = match port {
            Some(p) => format!("[{host}]:{p}"),
            None => plain_string.clone(),
        };

        for entry in &self.entries {
            let host_hit = entry.hosts.iter().any(|h| match h {
                HostPattern::Hashed { .. } => {
                    h.hashed_matches(plain_string.as_bytes())
                        || h.hashed_matches(port_string.as_bytes())
                }
                _ => h.matches(host, port),
            });

            if !host_hit {
                continue;
            }
            if entry.revoked {
                return Verification::Revoked;
            }
            if entry.key_type == *key_type && entry.key == key {
                return Verification::Trusted;
            }
            // Host matched but key did not → possible MITM.
            return Verification::Changed {
                stored_key_type: entry.key_type.clone(),
                stored_key: entry.key.clone(),
            };
        }
        Verification::Unknown
    }

    /// Record a key as trusted. Appends a plaintext entry; re-checking the same
    /// host/key is then `Trusted`.
    pub fn trust(
        &mut self,
        host: &str,
        port: Option<u16>,
        key_type: &KeyType,
        key: &str,
    ) -> std::io::Result<()> {
        let host_spec = match port {
            Some(p) => format!("[{host}]:{p}"),
            None => host.to_string(),
        };
        self.entries.push(Entry {
            revoked: false,
            hosts: vec![parse_host_pattern(&host_spec)],
            key_type: key_type.clone(),
            key: key.to_string(),
        });
        self.save()
    }

    /// Persist the store. When `path` is `None` (a `memory()` store) this is a
    /// no-op success.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = String::new();
        for entry in &self.entries {
            let revoked = if entry.revoked { "@revoked " } else { "" };
            let hosts: Vec<String> = entry
                .hosts
                .iter()
                .map(|h| match h {
                    HostPattern::Any => "*".to_string(),
                    HostPattern::Plain { host, port } => match port {
                        Some(p) => format!("[{host}]:{p}"),
                        None => host.clone(),
                    },
                    HostPattern::Hashed { salt, hash } => {
                        format!("|1|{}|{}", b64_encode(salt), b64_encode(hash))
                    }
                })
                .collect();
            out.push_str(&format!(
                "{revoked}{} {} {}\n",
                hosts.join(" "),
                entry.key_type.as_known_hosts_token(),
                entry.key
            ));
        }
        std::fs::write(path, out)
    }
}

fn parse_known_hosts(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // OpenSSH: `@revoked host1 host2 type key` (or without `@revoked`).
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 3 {
            continue;
        }
        let revoked = tokens[0] == "@revoked";
        let start = if revoked { 1 } else { 0 };
        if tokens.len() < start + 3 {
            continue;
        }
        let key_type = KeyType::from_known_hosts_token(tokens[tokens.len() - 2]);
        let key = tokens[tokens.len() - 1].to_string();
        let host_tokens = &tokens[start..tokens.len() - 2];
        let hosts: Vec<HostPattern> = host_tokens.iter().map(|h| parse_host_pattern(h)).collect();
        entries.push(Entry {
            revoked,
            hosts,
            key_type,
            key,
        });
    }
    entries
}

fn parse_host_pattern(s: &str) -> HostPattern {
    if s == "*" {
        return HostPattern::Any;
    }
    if let Some(rest) = s.strip_prefix("|1|") {
        // |1|base64salt|base64hash
        let mut it = rest.split('|');
        let salt = it.next().and_then(b64_decode);
        let hash = it.next().and_then(b64_decode);
        if let (Some(salt), Some(hash)) = (salt, hash) {
            return HostPattern::Hashed { salt, hash };
        }
    }
    if let Some(rest) = s.strip_prefix('[') {
        // `[host]:port` → split on the literal "]:".
        if let Some((host, port)) = rest.split_once("]:") {
            if let Ok(port) = port.parse::<u16>() {
                return HostPattern::Plain {
                    host: host.to_string(),
                    port: Some(port),
                };
            }
        }
    }
    HostPattern::Plain {
        host: s.to_string(),
        port: None,
    }
}

// ---------------------------------------------------------------------------
// SHA-1 + HMAC-SHA1 (inline, no external crate — needed for hashed known_hosts)
// ---------------------------------------------------------------------------

fn sha1(message: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let ml = (message.len() as u64).wrapping_mul(8);
    let mut msg = message.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        #[allow(clippy::needless_range_loop)]
        for i in 0..80 {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A827999)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9EBA1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1BBCDC)
            } else {
                (b ^ c ^ d, 0xCA62C1D6)
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let d = sha1(key);
        k[..20].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = ipad.to_vec();
    inner.extend_from_slice(message);
    let inner_digest = sha1(&inner);
    let mut outer = opad.to_vec();
    outer.extend_from_slice(&inner_digest);
    sha1(&outer).to_vec()
}

// Base64 for hashed known_hosts fields. Uses the `base64` crate (already a
// dependency) rather than a hand-rolled codec. OpenSSH writes these fields
// *without* '=' padding, so encoding is unpadded; decoding tolerates both.
fn b64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(s.trim_end_matches('='))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_rfc_vector() {
        // FIPS 180-1 example: "abc" -> a9993e364706816aba3e25717850c26c9cd0d89d
        let d = sha1(b"abc");
        assert_eq!(hex::encode(d), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn hmac_sha1_rfc2202_case1() {
        // key = 0x0b * 20, data = "Hi There"
        let key = vec![0x0b; 20];
        let digest = hmac_sha1(&key, b"Hi There");
        assert_eq!(
            hex::encode(digest),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
    }

    #[test]
    fn trust_then_check_is_trusted() {
        let mut kh = KnownHosts::memory();
        kh.trust("compute-1", Some(22), &KeyType::SshEd25519, "AAAAC3...")
            .unwrap();
        assert_eq!(
            kh.check("compute-1", Some(22), &KeyType::SshEd25519, "AAAAC3..."),
            Verification::Trusted
        );
    }

    #[test]
    fn unknown_when_no_entry() {
        let kh = KnownHosts::memory();
        assert_eq!(
            kh.check("ghost", None, &KeyType::SshEd25519, "k"),
            Verification::Unknown
        );
    }

    #[test]
    fn changed_key_is_rejected() {
        let mut kh = KnownHosts::memory();
        kh.trust("host", None, &KeyType::SshRsa, "orig").unwrap();
        match kh.check("host", None, &KeyType::SshRsa, "evil") {
            Verification::Changed { .. } => {}
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn port_specific_entry_does_not_match_other_port() {
        let mut kh = KnownHosts::memory();
        kh.trust("host", Some(2222), &KeyType::SshEd25519, "k")
            .unwrap();
        // Bare port 22 is not covered by the [host]:2222 entry.
        assert_eq!(
            kh.check("host", Some(22), &KeyType::SshEd25519, "k"),
            Verification::Unknown
        );
        assert_eq!(
            kh.check("host", Some(2222), &KeyType::SshEd25519, "k"),
            Verification::Trusted
        );
    }

    #[test]
    fn wildcard_matches_any_host() {
        let mut kh = KnownHosts::memory();
        kh.trust("*", None, &KeyType::SshEd25519, "star").unwrap();
        // The stored host string is literally "*"; check matches any host.
        let parsed = parse_known_hosts("* ssh-ed25519 star");
        assert!(matches!(parsed[0].hosts[0], HostPattern::Any));
        assert_eq!(
            kh.check("anything", None, &KeyType::SshEd25519, "star"),
            Verification::Trusted
        );
    }

    #[test]
    fn revoked_entry_is_detected() {
        let text = "@revoked host ssh-ed25519 BADKEY\n";
        let kh = KnownHosts {
            path: None,
            entries: parse_known_hosts(text),
        };
        assert_eq!(
            kh.check("host", None, &KeyType::SshEd25519, "BADKEY"),
            Verification::Revoked
        );
    }

    #[test]
    fn hashed_entry_round_trips_and_matches() {
        // Build a hashed entry the way OpenSSH would, then verify lookup.
        let salt = vec![0x11u8; 16];
        let host_string = "[host]:22";
        let hash = hmac_sha1(&salt, host_string.as_bytes());
        let line = format!(
            "|1|{}|{} ssh-ed25519 KEYXYZ\n",
            b64_encode(&salt),
            b64_encode(&hash)
        );
        let kh = KnownHosts {
            path: None,
            entries: parse_known_hosts(&line),
        };
        assert_eq!(
            kh.check("host", Some(22), &KeyType::SshEd25519, "KEYXYZ"),
            Verification::Trusted
        );
        // Wrong key for the same host → Changed (host matched, key differs).
        match kh.check("host", Some(22), &KeyType::SshEd25519, "WRONG") {
            Verification::Changed { .. } => {}
            other => panic!("expected Changed, got {other:?}"),
        }
        // A different host is Unknown.
        assert_eq!(
            kh.check("other", Some(22), &KeyType::SshEd25519, "KEYXYZ"),
            Verification::Unknown
        );
    }

    #[test]
    fn load_and_recheck_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let mut kh = KnownHosts::load(&path).unwrap();
        kh.trust("h", None, &KeyType::SshEd25519, "v").unwrap();
        drop(kh);

        let kh2 = KnownHosts::load(&path).unwrap();
        assert_eq!(
            kh2.check("h", None, &KeyType::SshEd25519, "v"),
            Verification::Trusted
        );
    }
}
