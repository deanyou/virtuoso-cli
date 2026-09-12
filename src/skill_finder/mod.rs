//! SKILL Finder — query Cadence SKILL API documentation from .fnd database.
//!
//! # Usage
//!
//! ```ignore
//! use virtuoso_cli::skill_finder::{SKILLFinder, SearchMode};
//!
//! let mut finder = SKILLFinder::new();
//! // Load from a local directory (requires Cadence installation)
//! if let Ok(()) = finder.load("/path/to/doc/finder/SKILL") {
//!     // Search
//!     let results = finder.search("dbOpen", SearchMode::Prefix, 10);
//!     for entry in results {
//!         println!("{}", entry.format());
//!     }
//! }
//! ```
//!
//! # Remote Cache
//!
//! For SSH remote scenarios, the SKILL Finder database is cached locally:
//! - Cache path: `~/.cache/virtuoso_bridge/skill_finder/<host>/`
//! - Use `sync_from_remote()` to download, or `load_or_sync()` for lazy sync
//! - Use `--refresh` flag to force refresh the cache

#![allow(dead_code)]

mod parser;

pub use parser::{parse_fnd_directory, SkillEntry};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Search mode for SKILL Finder queries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SearchMode {
    /// Case-insensitive substring match (default)
    #[default]
    Fuzzy,
    /// Name starts with query
    Prefix,
    /// Name ends with query
    Suffix,
    /// Exact name match
    Exact,
    /// Regular expression match
    Regex,
}

impl std::fmt::Display for SearchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchMode::Fuzzy => write!(f, "fuzzy"),
            SearchMode::Prefix => write!(f, "prefix"),
            SearchMode::Suffix => write!(f, "suffix"),
            SearchMode::Exact => write!(f, "exact"),
            SearchMode::Regex => write!(f, "regex"),
        }
    }
}

impl std::str::FromStr for SearchMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "fuzzy" => Ok(Self::Fuzzy),
            "prefix" => Ok(Self::Prefix),
            "suffix" => Ok(Self::Suffix),
            "exact" => Ok(Self::Exact),
            "regex" => Ok(Self::Regex),
            _ => Err(format!("unknown search mode: {}", s)),
        }
    }
}

/// SKILL Finder for querying Cadence SKILL API documentation.
pub struct SKILLFinder {
    /// Path to the SKILL Finder root directory
    source_dir: Option<PathBuf>,
    /// All loaded entries
    entries: Vec<SkillEntry>,
    /// Whether entries have been loaded from disk
    loaded: bool,
}

impl Default for SKILLFinder {
    fn default() -> Self {
        Self::new()
    }
}

impl SKILLFinder {
    /// Create a new empty SKILL Finder
    pub fn new() -> Self {
        Self {
            source_dir: None,
            entries: Vec::new(),
            loaded: false,
        }
    }

    /// Create a SKILLFinder pre-loaded with the given entries.
    ///
    /// Available to integration tests in `tests/` (via the public re-export)
    /// and to in-source `#[cfg(test)]` unit tests (via `pub(crate)`). The
    /// `#[doc(hidden)]` keeps it from appearing in the public API docs.
    #[doc(hidden)]
    pub fn for_test(entries: Vec<SkillEntry>) -> Self {
        Self {
            source_dir: None,
            entries,
            loaded: true,
        }
    }

    /// Load entries from a directory path
    pub fn load(&mut self, source_dir: impl Into<PathBuf>) -> std::io::Result<()> {
        let dir = source_dir.into();
        if !dir.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("SKILL Finder directory not found: {}", dir.display()),
            ));
        }
        self.source_dir = Some(dir.clone());
        self.entries = parse_fnd_directory(&dir);
        self.loaded = true;
        Ok(())
    }

    /// Check if entries are loaded
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Get the number of loaded entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if there are no entries
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Search for SKILL entries matching query.
    ///
    /// # Arguments
    ///
    /// * `query` - Search string
    /// * `mode` - Search mode (default: Fuzzy)
    /// * `limit` - Maximum number of results (default: 50)
    /// * `include_desc` - When `true`, also matches the description field.
    ///   Useful for `dbOpen`-style queries where the user wants functions
    ///   whose description mentions a keyword even if the function name does not.
    pub fn search(
        &self,
        query: &str,
        mode: SearchMode,
        limit: usize,
        include_desc: bool,
    ) -> Vec<&SkillEntry> {
        if !self.loaded {
            return Vec::new();
        }

        let results: Vec<&SkillEntry> = if include_desc {
            self.search_with_description(query, mode)
        } else {
            match mode {
                SearchMode::Exact => self.exact_match(query),
                SearchMode::Prefix => self.prefix_match(query),
                SearchMode::Suffix => self.suffix_match(query),
                SearchMode::Regex => self.regex_match(query),
                SearchMode::Fuzzy => self.fuzzy_match(query),
            }
        };

        // Sort by name
        let mut sorted: Vec<_> = results;
        sorted.sort_by(|a, b| a.name.cmp(&b.name));

        // Apply limit
        sorted.into_iter().take(limit).collect()
    }

    /// Name OR description match. Used when `include_desc` is true.
    /// For non-fuzzy modes we fall back to the same name-only matcher so
    /// users don't get surprising description-only hits for `prefix:`/`suffix:`
    /// (which are name-shape filters by definition).
    fn search_with_description(&self, query: &str, mode: SearchMode) -> Vec<&SkillEntry> {
        let q = query.to_lowercase();
        match mode {
            SearchMode::Exact => self.exact_match(query),
            SearchMode::Prefix => self
                .entries
                .iter()
                .filter(|e| e.name.to_lowercase().starts_with(&q))
                .collect(),
            SearchMode::Suffix => self
                .entries
                .iter()
                .filter(|e| e.name.to_lowercase().ends_with(&q))
                .collect(),
            SearchMode::Regex => {
                let pattern = match regex::Regex::new(&format!("(?i){query}")) {
                    Ok(p) => p,
                    Err(_) => return Vec::new(),
                };
                self.entries
                    .iter()
                    .filter(|e| pattern.is_match(&e.name) || pattern.is_match(&e.description))
                    .collect()
            }
            SearchMode::Fuzzy => self
                .entries
                .iter()
                .filter(|e| {
                    let name = e.name.to_lowercase();
                    let desc = e.description.to_lowercase();
                    name.contains(&q) || desc.contains(&q)
                })
                .collect(),
        }
    }

    fn exact_match(&self, query: &str) -> Vec<&SkillEntry> {
        self.entries.iter().filter(|e| e.name == query).collect()
    }

    fn prefix_match(&self, query: &str) -> Vec<&SkillEntry> {
        self.entries
            .iter()
            .filter(|e| e.name.starts_with(query))
            .collect()
    }

    fn suffix_match(&self, query: &str) -> Vec<&SkillEntry> {
        self.entries
            .iter()
            .filter(|e| e.name.ends_with(query))
            .collect()
    }

    fn regex_match(&self, query: &str) -> Vec<&SkillEntry> {
        match regex::Regex::new(&format!("(?i){}", query)) {
            Ok(pattern) => self
                .entries
                .iter()
                .filter(|e| pattern.is_match(&e.name))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn fuzzy_match(&self, query: &str) -> Vec<&SkillEntry> {
        let q = query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| e.name.to_lowercase().contains(&q))
            .collect()
    }

    /// Format search results for CLI output
    pub fn format_results(&self, results: &[&SkillEntry], query: &str) -> String {
        if results.is_empty() {
            return format!("No results for: {}", query);
        }

        let header = format!(
            "SKILL Finder — {} result(s) for '{}':\n",
            results.len(),
            query
        );
        let lines: Vec<String> = results.iter().map(|e| e.format()).collect();
        header + &lines.join("\n")
    }
}

// =============================================================================
// Cache Management
// =============================================================================

/// Get the cache directory for a host.
/// Returns: `~/.cache/virtuoso_bridge/skill_finder/<host>/`
pub fn cache_dir(host: &str) -> Option<PathBuf> {
    Some(crate::runtime_paths::cache_subdir(&["skill_finder", host]))
}

/// Check if cache exists for a host
pub fn cache_exists(host: &str) -> bool {
    cache_dir(host)
        .map(|d| d.exists() && d.is_dir())
        .unwrap_or(false)
}

/// Get the number of cached files for a host
pub fn cache_file_count(host: &str) -> usize {
    cache_dir(host)
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "fnd"))
                .count()
        })
        .unwrap_or(0)
}

/// Sync SKILL Finder database from a remote server via SSH.
///
/// Downloads all `.fnd` files from the remote `doc/finder/SKILL/` directory
/// to the local cache.
///
/// # Arguments
///
/// * `host` - Remote hostname
/// * `ssh_target` - SSH target string (e.g., "user@host" or "host")
/// * `cadence_cshrc` - Path to Cadence cshrc file (for loading environment)
/// * `progress` - Optional callback for progress updates
///
/// # Returns
///
/// Number of files synced, or error
pub fn sync_from_remote<F>(
    host: &str,
    ssh_target: &str,
    ssh_key: Option<&str>,
    cadence_cshrc: Option<&str>,
    progress: Option<F>,
) -> std::io::Result<usize>
where
    F: Fn(&str) + Copy,
{
    let cache = cache_dir(host).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not determine cache directory",
        )
    })?;

    // Create cache directory
    std::fs::create_dir_all(&cache)?;

    // Find every remote SKILL Finder directory (DFII tree first, Spectre tree
    // after). A site has several Cadence installs and they ship different
    // databases, so we union them rather than stopping at the first hit.
    let remote_dirs = find_remote_skill_finder_dirs(ssh_target, ssh_key, cadence_cshrc)?;

    if let Some(p) = progress {
        p(&format!(
            "Found {} remote SKILL Finder dir(s): {}",
            remote_dirs.len(),
            remote_dirs.join(", ")
        ));
    }

    let mut synced = 0;
    let mut failed = 0;
    let mut written: Vec<std::ffi::OsString> = Vec::new();

    for remote_dir in &remote_dirs {
        // List remote .fnd files (recursive: IC231 groups them into
        // Core_SKILL/, DFII_SKILL/, Schematics/, Virtuoso_ADE/, … subdirs)
        let list_script = format!(
            r#"find {} -name "*.fnd" -type f 2>/dev/null | head -200"#,
            remote_dir
        );

        let output = crate::transport::ssh::doc_transfer_command("ssh", ssh_key)
            .arg(ssh_target)
            .arg(&list_script)
            .output()
            .map_err(|e| std::io::Error::other(format!("SSH failed: {}", e)))?;

        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "Failed to list remote files: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if let Some(p) = progress {
            p(&format!(
                "Found {} .fnd files under {}",
                files.len(),
                remote_dir
            ));
        }

        // Download each file
        for remote_file in &files {
            let base = std::path::Path::new(remote_file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown.fnd");

            let file_name = cache_file_name(remote_dir, base);
            let local_path = cache.join(&file_name);

            // Build SCP command
            let scp_result = crate::transport::ssh::doc_transfer_command("scp", ssh_key)
                .arg(format!("{}:{}", ssh_target, remote_file))
                .arg(&local_path)
                .output();

            match scp_result {
                Ok(out) if out.status.success() => {
                    synced += 1;
                    written.push(std::ffi::OsString::from(&file_name));
                    if let Some(p) = progress {
                        p(&format!("Downloaded: {}", file_name));
                    }
                }
                Ok(out) => {
                    failed += 1;
                    tracing::warn!(
                        "Failed to download {}: {}",
                        file_name,
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
                Err(e) => {
                    failed += 1;
                    tracing::warn!("SCP error for {}: {}", file_name, e);
                }
            }
        }
    }

    // Drop cached databases the remote no longer offers, so a sync leaves the
    // cache matching the server. Skipped when anything failed to download —
    // a half-finished sync must not delete the copies we still have.
    if failed == 0 && synced > 0 {
        prune_stale_cache(&cache, &written, progress);
    }

    if let Some(p) = progress {
        p(&format!("Cache sync complete: {} files", synced));
    }

    Ok(synced)
}

/// Remove `.fnd` files in `cache` that were not written by this sync.
fn prune_stale_cache<F>(cache: &std::path::Path, written: &[std::ffi::OsString], progress: Option<F>)
where
    F: Fn(&str) + Copy,
{
    let Ok(entries) = std::fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|ext| ext == "fnd") {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        if written.iter().any(|w| w == name) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                if let Some(p) = progress {
                    p(&format!("Removed stale: {}", name.to_string_lossy()));
                }
            }
            Err(e) => tracing::warn!("Failed to remove stale {}: {}", path.display(), e),
        }
    }
}

/// Build the shell snippet that exports the Cadence environment from a cshrc.
///
/// Returns an empty string when no cshrc is configured.
pub fn cadence_env_setup(cadence_cshrc: Option<&str>) -> String {
    match cadence_cshrc {
        Some(cshrc) => {
            let escaped = cshrc.replace('\'', "'\"'\"'\"'\"'");
            format!(
                r#"eval "$(csh -c 'source {}; env' 2>/dev/null | grep -E '^(PATH|LM_LICENSE_FILE|CDS)=' | sed 's/^/export /')" 2>/dev/null"#,
                escaped
            )
        }
        None => String::new(),
    }
}

/// Shell script that prints **every** `doc/finder/SKILL` directory reachable
/// from the Cadence binaries on `PATH`, one per line, most relevant first.
///
/// A site typically has several Cadence installs side by side, and they do
/// **not** ship the same documentation. On the reference box:
///
/// ```text
/// virtuoso -> /opt/Cadence/IC231/tools/dfII/bin/virtuoso
///              -> /opt/Cadence/IC231/doc/finder/SKILL        39 .fnd
/// spectre  -> /opt/Cadence/SPECTRE231/tools/spectre/bin/spectre
///              -> /opt/Cadence/SPECTRE231/doc/finder/SKILL    2 .fnd
/// ```
///
/// The previous implementation probed `which spectre || which virtuoso`, so
/// the `||` short-circuited on the Spectre-only tree and the DFII databases
/// (`skdfref`, `skcompref`, `skuiref`, `maeSKILLref`, …) were never synced —
/// i.e. lookups for the schematic/UI/Maestro APIs we actually drive silently
/// returned "not found". Probe both, prefer `virtuoso`, and union the results.
pub fn remote_finder_probe_script(cadence_cshrc: Option<&str>) -> String {
    format!(
        r#"{}
walk_up() {{
  p="$1"
  while [ -n "$p" ] && [ "$p" != "/" ]; do
    if [ -d "$p/doc/finder/SKILL" ]; then echo "$p/doc/finder/SKILL"; return 0; fi
    p=$(dirname "$p")
  done
  return 1
}}
for b in virtuoso spectre; do
  x=$(command -v "$b" 2>/dev/null) || continue
  [ -n "$x" ] && walk_up "$x"
done"#,
        cadence_env_setup(cadence_cshrc)
    )
}

/// Parse the output of [`remote_finder_probe_script`]: trim, drop blanks and
/// `NOTFOUND` markers, and de-duplicate while preserving discovery order.
pub fn parse_finder_dirs(stdout: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line == "NOTFOUND" || !line.starts_with('/') {
            continue;
        }
        if !out.iter().any(|d| d == line) {
            out.push(line.to_string());
        }
    }
    out
}

/// Extract the Cadence release directory name from a finder path.
///
/// `/opt/Cadence/IC231/doc/finder/SKILL` → `IC231`
pub fn release_tag(finder_dir: &str) -> Option<&str> {
    finder_dir
        .trim_end_matches('/')
        .strip_suffix("/doc/finder/SKILL")?
        .rsplit('/')
        .find(|s| !s.is_empty())
}

/// Local cache file name for a remote `.fnd`, namespaced by Cadence release.
///
/// Releases share basenames — `caiskill.fnd` exists in both `IC231` and
/// `SPECTRE231` — so a flat cache would let one silently clobber the other.
/// Prefixing also makes the `source` shown in search results say which release
/// (and which database) an entry came from.
pub fn cache_file_name(finder_dir: &str, base: &str) -> String {
    match release_tag(finder_dir) {
        Some(tag) => format!("{}__{}", tag, base),
        None => base.to_string(),
    }
}

/// Find every SKILL Finder directory on a remote server via SSH.
///
/// Ordered most-relevant-first (the `virtuoso`/DFII tree before the Spectre
/// tree). Returns an error only when no Cadence install is reachable at all.
fn find_remote_skill_finder_dirs(
    ssh_target: &str,
    ssh_key: Option<&str>,
    cadence_cshrc: Option<&str>,
) -> std::io::Result<Vec<String>> {
    let output = crate::transport::ssh::doc_transfer_command("ssh", ssh_key)
        .arg(ssh_target)
        .arg(remote_finder_probe_script(cadence_cshrc))
        .output()
        .map_err(|e| std::io::Error::other(format!("SSH failed: {}", e)))?;

    let dirs = parse_finder_dirs(&String::from_utf8_lossy(&output.stdout));

    if dirs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No doc/finder/SKILL directory found near virtuoso/spectre on the remote server. \
             Ensure Cadence is in PATH or set VB_CADENCE_CSHRC.",
        ));
    }

    Ok(dirs)
}

/// Load from cache, or sync if cache doesn't exist.
///
/// Returns the path that was loaded from.
pub fn load_or_sync(
    finder: &mut SKILLFinder,
    host: &str,
    ssh_target: &str,
    ssh_key: Option<&str>,
    cadence_cshrc: Option<&str>,
) -> std::io::Result<PathBuf> {
    // Try cache first
    if let Some(cache) = cache_dir(host) {
        if cache.exists() {
            let count = cache_file_count(host);
            if count > 0 {
                finder.load(&cache)?;
                return Ok(cache);
            }
        }
    }

    // Sync from remote with empty progress function
    let _ = sync_from_remote(host, ssh_target, ssh_key, cadence_cshrc, Some(|_: &str| ()))?;

    // Load from cache
    let cache = cache_dir(host).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Cache directory not found")
    })?;

    finder.load(&cache)?;
    Ok(cache)
}

/// Clear cache for a host
pub fn clear_cache(host: &str) -> std::io::Result<()> {
    if let Some(cache) = cache_dir(host) {
        if cache.exists() {
            std::fs::remove_dir_all(&cache)?;
        }
    }
    Ok(())
}

/// Get cache info for a host
pub fn cache_info(host: &str) -> Option<CacheInfo> {
    let cache = cache_dir(host)?;
    if !cache.exists() {
        return None;
    }

    let file_count = std::fs::read_dir(&cache)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "fnd"))
        .count();

    let modified = std::fs::metadata(&cache).ok()?.modified().ok();

    Some(CacheInfo {
        path: cache,
        file_count,
        modified,
    })
}

/// Information about a cached SKILL Finder database
#[derive(Debug)]
pub struct CacheInfo {
    /// Path to cache directory
    pub path: PathBuf,
    /// Number of .fnd files
    pub file_count: usize,
    /// Last modified time
    pub modified: Option<std::time::SystemTime>,
}

/// Search options for SKILL Finder
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Search mode (default: Fuzzy)
    pub mode: SearchMode,
    /// Maximum number of results (default: 50)
    pub limit: usize,
    /// Case sensitive (default: false)
    pub case_sensitive: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- remote finder discovery (defect K) ---------------------------------

    #[test]
    fn probe_script_checks_virtuoso_before_spectre() {
        let script = remote_finder_probe_script(None);
        let v = script.find("virtuoso").expect("probes virtuoso");
        let s = script.find("spectre").expect("probes spectre");
        assert!(v < s, "virtuoso (DFII tree) must be probed first:\n{script}");
        // The old `which a || which b` short-circuit is what hid 37 of 39
        // databases; make sure it does not come back — both must be walked.
        assert!(
            script.contains("for b in virtuoso spectre"),
            "must probe every binary, not short-circuit on the first:\n{script}"
        );
        assert!(!script.contains("which "), "use `command -v` in a loop:\n{script}");
    }

    #[test]
    fn probe_script_embeds_cshrc_setup() {
        assert!(remote_finder_probe_script(None).starts_with('\n'));
        let with = remote_finder_probe_script(Some("/opt/cadence/cshrc"));
        assert!(with.contains("source /opt/cadence/cshrc"));
    }

    #[test]
    fn parse_finder_dirs_dedupes_and_keeps_order() {
        let out = "/opt/Cadence/IC231/doc/finder/SKILL\n\
                   /opt/Cadence/SPECTRE231/doc/finder/SKILL\n\
                   /opt/Cadence/IC231/doc/finder/SKILL\n";
        assert_eq!(
            parse_finder_dirs(out),
            vec![
                "/opt/Cadence/IC231/doc/finder/SKILL",
                "/opt/Cadence/SPECTRE231/doc/finder/SKILL",
            ]
        );
    }

    #[test]
    fn parse_finder_dirs_rejects_noise() {
        // Blank lines, NOTFOUND markers and shell chatter are not paths.
        let out = "\nNOTFOUND\nbash: command not found\n  /opt/Cadence/IC231/doc/finder/SKILL  \n";
        assert_eq!(
            parse_finder_dirs(out),
            vec!["/opt/Cadence/IC231/doc/finder/SKILL"]
        );
        assert!(parse_finder_dirs("").is_empty());
    }

    #[test]
    fn release_tag_names_the_cadence_install() {
        assert_eq!(
            release_tag("/opt/Cadence/IC231/doc/finder/SKILL"),
            Some("IC231")
        );
        assert_eq!(
            release_tag("/opt/Cadence/SPECTRE231/doc/finder/SKILL/"),
            Some("SPECTRE231")
        );
        assert_eq!(release_tag("/somewhere/else"), None);
    }

    #[test]
    fn cache_file_name_keeps_colliding_basenames_apart() {
        // caiskill.fnd ships in BOTH IC231 and SPECTRE231 — a flat cache let
        // one silently overwrite the other.
        let a = cache_file_name("/opt/Cadence/IC231/doc/finder/SKILL", "caiskill.fnd");
        let b = cache_file_name("/opt/Cadence/SPECTRE231/doc/finder/SKILL", "caiskill.fnd");
        assert_eq!(a, "IC231__caiskill.fnd");
        assert_eq!(b, "SPECTRE231__caiskill.fnd");
        assert_ne!(a, b);
        // Unrecognisable roots fall back to the plain basename.
        assert_eq!(cache_file_name("/weird/path", "x.fnd"), "x.fnd");
    }

    #[test]
    fn prune_stale_cache_removes_only_unwritten_fnd_files() {
        let dir = std::env::temp_dir().join(format!("vcli_prune_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("IC231__skdfref.fnd"), "x").unwrap();
        std::fs::write(dir.join("skdfref.fnd"), "stale flat name").unwrap();
        std::fs::write(dir.join("notes.txt"), "keep me").unwrap();

        let written = vec![std::ffi::OsString::from("IC231__skdfref.fnd")];
        prune_stale_cache(&dir, &written, None::<fn(&str)>);

        assert!(dir.join("IC231__skdfref.fnd").exists());
        assert!(!dir.join("skdfref.fnd").exists(), "stale .fnd must go");
        assert!(dir.join("notes.txt").exists(), "non-.fnd must be left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_mode_display() {
        assert_eq!(SearchMode::Fuzzy.to_string(), "fuzzy");
        assert_eq!(SearchMode::Prefix.to_string(), "prefix");
        assert_eq!(SearchMode::Suffix.to_string(), "suffix");
        assert_eq!(SearchMode::Exact.to_string(), "exact");
        assert_eq!(SearchMode::Regex.to_string(), "regex");
    }

    #[test]
    fn test_search_mode_from_str() {
        assert_eq!("fuzzy".parse::<SearchMode>().unwrap(), SearchMode::Fuzzy);
        assert_eq!("prefix".parse::<SearchMode>().unwrap(), SearchMode::Prefix);
        assert_eq!("exact".parse::<SearchMode>().unwrap(), SearchMode::Exact);
        assert!("unknown".parse::<SearchMode>().is_err());
    }

    #[test]
    fn test_skill_finder_empty() {
        let finder = SKILLFinder::new();
        assert!(!finder.is_loaded());
        assert!(finder.is_empty());
    }

    #[test]
    fn test_cache_dir_contains_host() {
        let cache = cache_dir("eda-server");
        assert!(cache.is_some());
        let path = cache.unwrap();
        assert!(path.to_string_lossy().contains("eda-server"));
        assert!(path.to_string_lossy().contains("skill_finder"));
    }

    #[test]
    fn test_cache_exists_nonexistent() {
        // Random host that doesn't exist should return false
        assert!(!cache_exists("nonexistent-host-xyz-12345"));
    }

    #[test]
    fn test_cache_info_nonexistent() {
        assert!(cache_info("nonexistent-host-xyz-12345").is_none());
    }

    // ========================================================================
    // include_desc tests
    // ========================================================================

    fn finder_with_corpus() -> SKILLFinder {
        SKILLFinder::for_test(vec![
            SkillEntry {
                name: "dbOpenCellView".into(),
                syntax: "dbOpenCellView(lib cell view)".into(),
                description: "Opens a cellView in the database".into(),
                source_file: Some("db.fnd".into()),
            },
            SkillEntry {
                name: "rodCreateRect".into(),
                syntax: "rodCreateRect(...)".into(),
                description: "Create a rectangle in the layout editor".into(),
                source_file: Some("rod.fnd".into()),
            },
            SkillEntry {
                name: "schOpen".into(),
                syntax: "schOpen()".into(),
                description: "Opens a cellView in the schematic".into(),
                source_file: Some("sch.fnd".into()),
            },
        ])
    }

    #[test]
    fn search_fuzzy_default_does_not_match_description_only() {
        let f = finder_with_corpus();
        // "rectangle" appears in rodCreateRect's description but not its name
        let r = f.search("rectangle", SearchMode::Fuzzy, 50, false);
        assert!(
            r.is_empty(),
            "default search should miss description-only matches, got {:?}",
            r.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn search_fuzzy_with_desc_matches_description() {
        let f = finder_with_corpus();
        let r = f.search("rectangle", SearchMode::Fuzzy, 50, true);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "rodCreateRect");
    }

    #[test]
    fn search_fuzzy_with_desc_still_matches_name() {
        let f = finder_with_corpus();
        // "dbOpen" is in dbOpenCellView's NAME; should also be found with desc
        let r = f.search("dbOpen", SearchMode::Fuzzy, 50, true);
        assert!(r.iter().any(|e| e.name == "dbOpenCellView"));
    }

    #[test]
    fn search_fuzzy_with_desc_case_insensitive() {
        let f = finder_with_corpus();
        let r = f.search("LAYOUT", SearchMode::Fuzzy, 50, true);
        // matches "layout" in rodCreateRect's description
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "rodCreateRect");
    }

    #[test]
    fn search_prefix_with_desc_still_uses_name_shape() {
        let f = finder_with_corpus();
        // "db" prefix on names → should match dbOpenCellView even with include_desc
        let r = f.search("db", SearchMode::Prefix, 50, true);
        assert!(r.iter().any(|e| e.name == "dbOpenCellView"));

        // "rectangle" is NOT a name prefix anywhere; should return empty for prefix
        let r = f.search("rectangle", SearchMode::Prefix, 50, true);
        assert!(
            r.is_empty(),
            "prefix mode with include_desc should still only match name prefixes, got {:?}",
            r.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn search_suffix_with_desc_still_uses_name_shape() {
        let f = finder_with_corpus();
        // "CellView" is a suffix of dbOpenCellView
        let r = f.search("CellView", SearchMode::Suffix, 50, true);
        let names: Vec<&str> = r.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"dbOpenCellView"),
            "dbOpenCellView ends with CellView"
        );
        assert!(!names.contains(&"schOpen"));
        assert!(!names.contains(&"rodCreateRect"));

        // "Open" is a suffix of schOpen only
        let r = f.search("Open", SearchMode::Suffix, 50, true);
        let names: Vec<&str> = r.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"schOpen"), "schOpen ends with Open");
        assert!(
            !names.contains(&"dbOpenCellView"),
            "dbOpenCellView ends with View, not Open"
        );
    }

    #[test]
    fn search_exact_with_desc_still_uses_name_exact() {
        let f = finder_with_corpus();
        let r = f.search("dbOpenCellView", SearchMode::Exact, 50, true);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "dbOpenCellView");

        // "Opens" is the first word in dbOpenCellView's description; not a name
        let r = f.search("Opens", SearchMode::Exact, 50, true);
        assert!(r.is_empty());
    }

    #[test]
    fn search_regex_with_desc_matches_description() {
        let f = finder_with_corpus();
        // regex matching the description "layout editor"
        let r = f.search("layout.*editor", SearchMode::Regex, 50, true);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "rodCreateRect");
    }

    #[test]
    fn search_with_desc_respects_limit() {
        // The corpus has 2 entries whose name/description contains "cellView"
        // (dbOpenCellView, schOpen). limit=1 must return exactly one,
        // proving the cap is applied AFTER sort, not before filtering.
        let f = finder_with_corpus();
        let r = f.search("cellView", SearchMode::Fuzzy, 1, true);
        assert_eq!(r.len(), 1, "limit=1 must be respected");

        // limit=2 returns both — verifies the cap is not stuck at 1.
        let r = f.search("cellView", SearchMode::Fuzzy, 2, true);
        assert_eq!(r.len(), 2, "limit=2 must allow both matches");
    }

    #[test]
    fn search_unloaded_finder_returns_empty_regardless_of_desc() {
        let f = SKILLFinder::new();
        assert!(!f.loaded);
        let r = f.search("anything", SearchMode::Fuzzy, 50, true);
        assert!(r.is_empty());
    }
}
