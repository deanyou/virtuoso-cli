//! The single persistent configuration file: `config.toml`.
//!
//! RFC #83 replaced vcli's implicit `cwd → parent → …` `.env` lookup with one
//! fixed, queryable location. This module owns it.
//!
//! # Contract
//!
//! * **Read once.** [`ConfigFile::load`] is called from the configuration
//!   entry point and the parsed result is threaded through resolution. It is
//!   never called per field — a `Config` has ~36 fields and none of them may
//!   touch the disk.
//! * **Absent is fine, broken is not.** A missing file means "no file
//!   configuration" and defaults apply. A file that cannot be parsed, or that
//!   puts a non-scalar where a value is expected, is a **hard error naming the
//!   path** — never a silent fall back to defaults.
//! * **The path is overridable.** [`path`] resolves through
//!   [`crate::runtime_paths::config_subdir`], so `VB_CONFIG_DIR` /
//!   `VB_HOME` / `XDG_CONFIG_HOME` all redirect it. That is what lets the
//!   tests below run on every platform without touching a developer's real
//!   home directory (Windows' `dirs::home_dir()` ignores `HOME`, so a
//!   `HOME`-only override is not portable).
//!
//! # Layout
//!
//! ```toml
//! remote_host = "eda-server"      # global — applies to every profile
//!
//! [profile.prod]                  # wins when the active profile is "prod"
//! remote_host = "eda-prod"
//! ```
//!
//! Values are scalars only. A nested table other than `[profile.*]`, or an
//! array, is a configuration error rather than something to skip.

use crate::error::{Result, VirtuosoError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// File name, inside the per-app config directory.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// The `[profile]` table name.
const PROFILE_TABLE: &str = "profile";

/// Absolute path of the configuration file.
pub fn path() -> PathBuf {
    crate::runtime_paths::config_subdir(&[CONFIG_FILE_NAME])
}

/// Map an environment-variable key (`VB_REMOTE_HOST`) to the canonical
/// configuration-file key (`remote_host`).
///
/// The resolution layer passes environment-variable names so the file and the
/// process environment share one precedence rule. The file itself uses the
/// friendlier lower-case form that `vcli init` and the TUI write. Keys already
/// in file form pass through unchanged, so [`ConfigFile::get`] accepts both
/// spellings.
pub fn file_key(env_key: &str) -> String {
    let stripped = env_key.strip_prefix("VB_").unwrap_or(env_key);
    stripped.to_ascii_lowercase()
}

/// Inverse of [`file_key`]: the environment variable that outranks a file key.
pub fn env_var_for(file_key: &str) -> String {
    format!("VB_{}", file_key.to_ascii_uppercase())
}

/// A configuration value that survived parsing.
///
/// Deliberately not `toml::Value`: a `ConfigFile` is fully validated at load
/// time, so every value in it is known to be renderable as the string the rest
/// of the configuration layer expects.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl Scalar {
    /// Render as the string the configuration layer consumes.
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Int(i) => i.to_string(),
            Self::Float(f) => f.to_string(),
            Self::Bool(b) => b.to_string(),
        }
    }

    /// Convert to a TOML value for serialization. Scalars round-trip exactly:
    /// `Text` ↔ string, the numeric/boolean variants ↔ their TOML types.
    fn to_toml_value(&self) -> toml::Value {
        match self {
            Self::Text(s) => toml::Value::String(s.clone()),
            Self::Int(i) => toml::Value::Integer(*i),
            Self::Float(f) => toml::Value::Float(*f),
            Self::Bool(b) => toml::Value::Boolean(*b),
        }
    }

    /// Convert a parsed TOML value, rejecting the non-scalar shapes.
    fn from_toml(value: &toml::Value) -> Option<Self> {
        match value {
            toml::Value::String(s) => Some(Self::Text(s.clone())),
            toml::Value::Integer(i) => Some(Self::Int(*i)),
            toml::Value::Float(f) => Some(Self::Float(*f)),
            toml::Value::Boolean(b) => Some(Self::Bool(*b)),
            toml::Value::Datetime(d) => Some(Self::Text(d.to_string())),
            toml::Value::Array(_) | toml::Value::Table(_) => None,
        }
    }
}

/// Where inside `config.toml` a key was actually resolved.
///
/// Returned by [`crate::config_file::ConfigFile::get_with_loc`] so the caller
/// can distinguish "value came from a `[profile.<name>]` section" from "value
/// fell through to the global section" in a single lookup.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FileLoc {
    /// Hit a `[profile.<name>]` section. `actual_key` is the real TOML key
    /// (friendly or upper-case spelling) that matched.
    Profile { profile: String, actual_key: String },
    /// Hit the top-level global section. `actual_key` is the real TOML key
    /// that matched.
    Global { actual_key: String },
}

/// A parsed `config.toml`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConfigFile {
    /// Top-level keys — the global section.
    global: BTreeMap<String, Scalar>,
    /// `[profile.<name>]` tables, keyed by profile name.
    profiles: BTreeMap<String, BTreeMap<String, Scalar>>,
}

impl ConfigFile {
    /// Read and parse the configuration file.
    ///
    /// `Ok(None)` when the file does not exist. A file that exists but cannot
    /// be read or parsed is an error — see the module contract.
    pub fn load() -> Result<Option<Self>> {
        let p = path();
        if !p.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&p).map_err(|e| {
            VirtuosoError::Config(format!("cannot read config file {}: {e}", p.display()))
        })?;
        Ok(Some(Self::parse(&raw, &p)?))
    }

    /// Parse file contents. `origin` is only used to name the file in errors.
    pub(crate) fn parse(raw: &str, origin: &Path) -> Result<Self> {
        let doc: toml::Table = toml::from_str(raw).map_err(|e| {
            VirtuosoError::Config(format!("invalid config file {}: {e}", origin.display()))
        })?;

        let mut global = BTreeMap::new();
        let mut profiles = BTreeMap::new();

        for (key, value) in doc {
            if key == PROFILE_TABLE {
                let table = value.as_table().ok_or_else(|| {
                    VirtuosoError::Config(format!(
                        "invalid config file {}: [{PROFILE_TABLE}] must be a table, got {}",
                        origin.display(),
                        kind_of(&value)
                    ))
                })?;
                for (name, entries) in table {
                    let inner = entries.as_table().ok_or_else(|| {
                        VirtuosoError::Config(format!(
                            "invalid config file {}: [{PROFILE_TABLE}.{name}] must be a table, \
                             got {}",
                            origin.display(),
                            kind_of(entries)
                        ))
                    })?;
                    let mut section = BTreeMap::new();
                    for (k, v) in inner {
                        let scalar = Scalar::from_toml(v).ok_or_else(|| {
                            VirtuosoError::Config(format!(
                                "invalid config file {}: {PROFILE_TABLE}.{name}.{k} must be a \
                                 string, number, boolean or date, got {})",
                                origin.display(),
                                kind_of(v)
                            ))
                        })?;
                        section.insert(k.clone(), scalar);
                    }
                    profiles.insert(name.clone(), section);
                }
            } else {
                let scalar = Scalar::from_toml(&value).ok_or_else(|| {
                    VirtuosoError::Config(format!(
                        "invalid config file {}: {key} must be a string, number, boolean or \
                         date, got {}",
                        origin.display(),
                        kind_of(&value)
                    ))
                })?;
                global.insert(key, scalar);
            }
        }

        Ok(Self { global, profiles })
    }

    /// Look a key up: `[profile.<active>]` first, then the global section.
    ///
    /// `key` may be the environment-variable name (`VB_REMOTE_HOST`) passed by
    /// the resolution layer, or the friendly file name (`remote_host`) passed by
    /// the TUI. Both spellings are accepted so a hand-edited file is robust.
    pub fn get(&self, key: &str, profile: Option<&str>) -> Option<String> {
        // Layer-first, then key-spelling — same precedence as `get_with_loc`.
        // Keep the two in sync by delegating; any change to lookup order must
        // live in ONE place.
        self.get_with_loc(key, profile).map(|(v, _)| v)
    }

    /// Like [`ConfigFile::get`], but also reports **where** the key was found so
    /// callers can attribute the value to a precise layer in a single lookup.
    ///
    /// Returns `Some((value, loc))` where `loc` records the actual section hit —
    /// `FileLoc::Profile { profile }` if the value came from a
    /// `[profile.<name>]` section, or `FileLoc::Global` if it fell through to
    /// the top-level `[global]` section.
    ///
    /// This consolidates what used to be two separate passes:
    /// `profile_section_has` (probe) followed by `get` (read). The returns from
    /// those two passes could disagree on which key matched, which is how a
    /// `vbSshKey` in the global section used to be misattributed as coming from
    /// a `[profile.<name>]` section that only had `VB_SSH_KEY`. A single walk
    /// sees one truth.
    pub(crate) fn get_with_loc(
        &self,
        key: &str,
        profile: Option<&str>,
    ) -> Option<(String, FileLoc)> {
        let friendly = file_key(key);
        let env = env_var_for(&friendly);
        let candidates = [friendly.as_str(), env.as_str()];

        // Layer-first, then key-spelling. Within a layer we try every
        // candidate key before falling through to the next layer — otherwise
        // `timeout` in `[global]` would shadow `VB_TIMEOUT` in
        // `[profile.<name>]` even though the latter is the higher-precedence
        // layer. The correct order is: ALL profile candidates, THEN all
        // global candidates.
        if let Some(p) = profile {
            if let Some(section) = self.profiles.get(p) {
                for candidate in candidates {
                    if let Some(v) = section.get(candidate) {
                        return Some((
                            v.as_text(),
                            FileLoc::Profile {
                                profile: p.to_string(),
                                actual_key: candidate.to_string(),
                            },
                        ));
                    }
                }
            }
        }
        for candidate in candidates {
            if let Some(v) = self.global.get(candidate) {
                return Some((
                    v.as_text(),
                    FileLoc::Global {
                        actual_key: candidate.to_string(),
                    },
                ));
            }
        }
        None
    }

    /// Whether the key is present in the file — the `[profile.<name>]` section
    /// when `profile` is given, otherwise the global section. Used by the TUI to
    /// show whether a value comes from the file (and is therefore something the
    /// user can clear) versus an environment variable or a default.
    ///
    /// Accepts both the friendly file key and the upper-case env-var spelling,
    /// matching [`ConfigFile::get`].
    #[allow(dead_code)]
    pub fn has(&self, key: &str, profile: Option<&str>) -> bool {
        // Same symmetric spelling handling as [`ConfigFile::get`].
        let friendly = file_key(key);
        let env = env_var_for(&friendly);
        let candidates = [friendly.as_str(), env.as_str()];
        for candidate in candidates {
            if let Some(p) = profile {
                if let Some(section) = self.profiles.get(p) {
                    if section.contains_key(candidate) {
                        return true;
                    }
                }
            }
            if self.global.contains_key(candidate) {
                return true;
            }
        }
        false
    }

    /// Set a key in the global section, or in `[profile.<name>]`.
    pub fn set(&mut self, profile: Option<&str>, key: &str, value: &str) {
        let scalar = Scalar::Text(value.to_string());
        match profile {
            Some(p) => self.profiles.entry(p.to_string()).or_default(),
            None => &mut self.global,
        }
        .insert(key.to_string(), scalar);
    }

    /// Remove a key so the lookup falls through to the next layer.
    pub fn remove(&mut self, profile: Option<&str>, key: &str) {
        match profile {
            Some(p) => {
                if let Some(section) = self.profiles.get_mut(p) {
                    section.remove(key);
                }
            }
            None => {
                self.global.remove(key);
            }
        }
    }

    /// Render as TOML using the official serializer.
    ///
    /// Using `toml::to_string_pretty` (rather than string joins) guarantees that
    /// profile names containing dots are quoted (`[profile."prod.eu"]`), that
    /// newlines and other special characters inside string values are escaped,
    /// and that the output always re-parses to an equal [`ConfigFile`].
    pub fn to_toml_string(&self) -> Result<String> {
        let mut root = toml::Table::new();
        for (k, v) in &self.global {
            root.insert(k.clone(), v.to_toml_value());
        }
        if !self.profiles.is_empty() {
            let mut profiles = toml::Table::new();
            for (name, section) in &self.profiles {
                let mut tbl = toml::Table::new();
                for (k, v) in section {
                    tbl.insert(k.clone(), v.to_toml_value());
                }
                profiles.insert(name.clone(), toml::Value::Table(tbl));
            }
            root.insert("profile".to_string(), toml::Value::Table(profiles));
        }
        toml::to_string_pretty(&root)
            .map_err(|e| VirtuosoError::Config(format!("cannot serialize config file: {e}")))
    }

    /// Write the file, creating its parent directory. Returns the path.
    pub fn save(&self) -> Result<PathBuf> {
        let p = path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                VirtuosoError::Config(format!("cannot create {}: {e}", dir.display()))
            })?;
        }
        let body = self.to_toml_string()?;
        std::fs::write(&p, body).map_err(|e| {
            VirtuosoError::Config(format!("cannot write config file {}: {e}", p.display()))
        })?;
        Ok(p)
    }
}

/// Human-readable TOML value kind, for error messages.
fn kind_of(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "boolean",
        toml::Value::Datetime(_) => "datetime",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test that redirects the config path must run alone: the path is
    /// process-global.
    use serial_test::serial;

    fn parse(raw: &str) -> Result<ConfigFile> {
        ConfigFile::parse(raw, Path::new("<test>"))
    }

    #[test]
    fn global_keys_are_readable() {
        let f = parse("remote_host = \"eda\"\ntimeout = 45\n").unwrap();
        assert_eq!(f.get("remote_host", None).as_deref(), Some("eda"));
        assert_eq!(f.get("timeout", None).as_deref(), Some("45"));
    }

    #[test]
    fn profile_section_beats_global() {
        let f =
            parse("remote_host = \"global\"\n\n[profile.prod]\nremote_host = \"prod\"\n").unwrap();
        assert_eq!(f.get("remote_host", Some("prod")).as_deref(), Some("prod"));
        assert_eq!(
            f.get("remote_host", Some("other")).as_deref(),
            Some("global")
        );
        assert_eq!(f.get("remote_host", None).as_deref(), Some("global"));
    }

    #[test]
    fn missing_key_is_absent_not_empty() {
        let f = parse("remote_host = \"eda\"\n").unwrap();
        assert_eq!(f.get("port", None), None);
        assert!(!f.has("port", None));
        assert!(f.has("remote_host", None));
    }

    #[test]
    fn non_string_scalars_render_as_text() {
        let f = parse("port = 65432\nratio = 1.5\nkeep = true\n").unwrap();
        assert_eq!(f.get("port", None).as_deref(), Some("65432"));
        assert_eq!(f.get("ratio", None).as_deref(), Some("1.5"));
        assert_eq!(f.get("keep", None).as_deref(), Some("true"));
    }

    #[test]
    fn broken_toml_is_an_error_naming_the_file() {
        let err = parse("remote_host = ").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid config file"), "got: {msg}");
        assert!(msg.contains("<test>"), "must name the file, got: {msg}");
    }

    #[test]
    fn array_value_is_an_error_not_a_skip() {
        let err = parse("hosts = [\"a\", \"b\"]\n").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("must be a string, number, boolean or date"),
            "got: {msg}"
        );
        assert!(msg.contains("got array"), "got: {msg}");
    }

    #[test]
    fn nested_table_outside_profile_is_an_error() {
        let err = parse("[misc]\nx = 1\n").unwrap_err();
        assert!(err.to_string().contains("got table"), "got: {err}");
    }

    #[test]
    fn profile_that_is_not_a_table_is_an_error() {
        let err = parse("profile = \"prod\"\n").unwrap_err();
        assert!(
            err.to_string().contains("[profile] must be a table"),
            "got: {err}"
        );
    }

    #[test]
    fn profile_entry_that_is_not_a_table_is_an_error() {
        let err = parse("[profile]\nprod = \"x\"\n").unwrap_err();
        assert!(
            err.to_string().contains("[profile.prod] must be a table"),
            "got: {err}"
        );
    }

    #[test]
    fn render_puts_globals_before_profile_tables() {
        let mut f = ConfigFile::default();
        f.set(None, "remote_host", "eda");
        f.set(Some("prod"), "remote_host", "eda-prod");
        let rendered = f.to_toml_string().unwrap();
        assert!(
            rendered.starts_with("remote_host = \"eda\"\n"),
            "globals must come first: {rendered}"
        );
        assert!(rendered.contains("[profile.prod]\n"));
        // And it must re-parse to the same thing.
        assert_eq!(parse(&rendered).unwrap(), f);
    }

    #[test]
    fn special_characters_survive_a_round_trip() {
        let mut f = ConfigFile::default();
        f.set(None, "spectre_cmd", "/opt/cad\"ence/spec\\tre");
        let rendered = f.to_toml_string().unwrap();
        let reparsed = parse(&rendered).unwrap();
        assert_eq!(
            reparsed.get("spectre_cmd", None).as_deref(),
            Some("/opt/cad\"ence/spec\\tre")
        );
    }

    #[test]
    fn env_var_key_maps_to_friendly_file_key() {
        let f = parse("remote_host = \"eda\"\n").unwrap();
        // Resolution layer passes the env-var name…
        assert_eq!(f.get("VB_REMOTE_HOST", None).as_deref(), Some("eda"));
        assert!(f.has("VB_REMOTE_HOST", None));
        // …the TUI / init pass the friendly name directly.
        assert_eq!(f.get("remote_host", None).as_deref(), Some("eda"));
    }

    #[test]
    fn legacy_upper_case_file_key_is_also_accepted() {
        // A buggy early build wrote VB_REMOTE_HOST; accept it so nothing breaks.
        let f = parse("VB_REMOTE_HOST = \"eda\"\n").unwrap();
        assert_eq!(f.get("VB_REMOTE_HOST", None).as_deref(), Some("eda"));
        assert_eq!(f.get("remote_host", None).as_deref(), Some("eda"));
    }

    #[test]
    fn friendly_query_finds_upper_case_file_key() {
        // Symmetric to the above: a friendly query finds an env-var-spelled key.
        let f = parse("VB_REMOTE_HOST = \"eda\"\n").unwrap();
        assert_eq!(f.get("remote_host", None).as_deref(), Some("eda"));
    }

    #[test]
    fn dotted_profile_name_is_quoted_on_save() {
        let mut f = ConfigFile::default();
        f.set(Some("prod.eu"), "remote_host", "eda-eu");
        let rendered = f.to_toml_string().unwrap();
        assert!(
            rendered.contains("[profile.\"prod.eu\"]"),
            "dotted profile name must be quoted: {rendered}"
        );
        assert_eq!(parse(&rendered).unwrap(), f);
    }

    #[test]
    fn newline_in_value_is_escaped_on_save() {
        let mut f = ConfigFile::default();
        f.set(None, "spectre_args", "line1\nline2");
        let rendered = f.to_toml_string().unwrap();
        // The official serializer never emits an invalid bare string: it uses a
        // multiline string (`"""…"""`) or an escaped `\n`, both of which re-parse
        // to the same value. The real guarantee is that the result round-trips.
        assert_eq!(parse(&rendered).unwrap(), f);
    }

    #[test]
    fn remove_falls_through_to_the_next_layer() {
        let mut f =
            parse("remote_host = \"global\"\n\n[profile.p]\nremote_host = \"p\"\n").unwrap();
        f.remove(Some("p"), "remote_host");
        assert_eq!(f.get("remote_host", Some("p")).as_deref(), Some("global"));
        f.remove(None, "remote_host");
        assert_eq!(f.get("remote_host", Some("p")), None);
    }

    // -- Path resolution: platform-independent override -------------------

    #[test]
    #[serial]
    fn path_honours_vb_config_dir() {
        std::env::set_var("VB_CONFIG_DIR", "/tmp/vcli-cfg-path-test");
        let p = path();
        std::env::remove_var("VB_CONFIG_DIR");
        assert_eq!(p, PathBuf::from("/tmp/vcli-cfg-path-test/vcli/config.toml"));
    }
}
