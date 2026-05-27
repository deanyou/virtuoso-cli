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
    pub fn search(&self, query: &str, mode: SearchMode, limit: usize) -> Vec<&SkillEntry> {
        if !self.loaded {
            return Vec::new();
        }

        let results: Vec<&SkillEntry> = match mode {
            SearchMode::Exact => self.exact_match(query),
            SearchMode::Prefix => self.prefix_match(query),
            SearchMode::Suffix => self.suffix_match(query),
            SearchMode::Regex => self.regex_match(query),
            SearchMode::Fuzzy => self.fuzzy_match(query),
        };

        // Sort by name
        let mut sorted: Vec<_> = results;
        sorted.sort_by(|a, b| a.name.cmp(&b.name));

        // Apply limit
        sorted.into_iter().take(limit).collect()
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
    dirs::cache_dir().map(|d| d.join("virtuoso_bridge").join("skill_finder").join(host))
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
    cadence_cshrc: Option<&str>,
    progress: Option<F>,
) -> std::io::Result<usize>
where
    F: Fn(&str) + Copy,
{
    use std::process::Command;

    let cache = cache_dir(host).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not determine cache directory",
        )
    })?;

    // Create cache directory
    std::fs::create_dir_all(&cache)?;

    // Find remote SKILL Finder directory
    let remote_dir = find_remote_skill_finder_dir(ssh_target, cadence_cshrc)?;

    if let Some(p) = progress {
        p(&format!("Found remote SKILL Finder at: {}", remote_dir));
    }

    // List remote .fnd files
    let list_script = format!(
        r#"find {} -name "*.fnd" -type f 2>/dev/null | head -200"#,
        remote_dir
    );

    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=30"])
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
        p(&format!("Found {} .fnd files on remote", files.len()));
    }

    // Download each file
    let mut synced = 0;
    for remote_file in &files {
        let file_name = std::path::Path::new(remote_file)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.fnd");

        let local_path = cache.join(file_name);

        // Build SCP command
        let scp_result = Command::new("scp")
            .args(["-o", "BatchMode=yes"])
            .args(["-o", "ConnectTimeout=30"])
            .arg(format!("{}:{}", ssh_target, remote_file))
            .arg(&local_path)
            .output();

        match scp_result {
            Ok(out) if out.status.success() => {
                synced += 1;
                if let Some(p) = progress {
                    p(&format!("Downloaded: {}", file_name));
                }
            }
            Ok(out) => {
                tracing::warn!(
                    "Failed to download {}: {}",
                    file_name,
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            Err(e) => {
                tracing::warn!("SCP error for {}: {}", file_name, e);
            }
        }
    }

    if let Some(p) = progress {
        p(&format!("Cache sync complete: {} files", synced));
    }

    Ok(synced)
}

/// Find the SKILL Finder directory on a remote server via SSH.
fn find_remote_skill_finder_dir(
    ssh_target: &str,
    cadence_cshrc: Option<&str>,
) -> std::io::Result<String> {
    use std::process::Command;

    // Build environment setup script
    let env_setup = if let Some(cshrc) = cadence_cshrc {
        let escaped = cshrc.replace('\'', "'\"'\"'\"'\"'");
        format!(
            r#"eval "$(csh -c 'source {}; env' 2>/dev/null | grep -E '^(PATH|LM_LICENSE_FILE|CDS)=' | sed 's/^/export /')" 2>/dev/null"#,
            escaped
        )
    } else {
        String::new()
    };

    // Find virtuoso binary
    let find_virtuoso = format!(
        r#"{}
which spectre 2>/dev/null || which virtuoso 2>/dev/null || echo NOTFOUND"#,
        env_setup
    );

    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=30"])
        .arg(ssh_target)
        .arg(&find_virtuoso)
        .output()
        .map_err(|e| std::io::Error::other(format!("SSH failed: {}", e)))?;

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if path.is_empty() || path == "NOTFOUND" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not find virtuoso/spectre on remote server. Ensure Cadence is in PATH or set VB_CADENCE_CSHRC.",
        ));
    }

    // Walk up from virtuoso to find doc/finder/SKILL
    let walk_script = format!(
        r#"p="{}"
while [ -n "$p" ] && [ "$p" != "/" ]; do
  if [ -d "$p/doc/finder/SKILL" ]; then echo "$p/doc/finder/SKILL"; exit 0; fi
  p=$(dirname "$p")
done
exit 1"#,
        path.replace('\'', "'\"'\"'\"'\"'")
    );

    let output2 = Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=30"])
        .arg(ssh_target)
        .arg(&walk_script)
        .output()
        .map_err(|e| std::io::Error::other(format!("SSH failed: {}", e)))?;

    let finder_path = String::from_utf8_lossy(&output2.stdout).trim().to_string();

    if finder_path.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "SKILL Finder not found near {}. Is Cadence installed correctly?",
                path
            ),
        ));
    }

    Ok(finder_path)
}

/// Load from cache, or sync if cache doesn't exist.
///
/// Returns the path that was loaded from.
pub fn load_or_sync(
    finder: &mut SKILLFinder,
    host: &str,
    ssh_target: &str,
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
    let _ = sync_from_remote(host, ssh_target, cadence_cshrc, Some(|_: &str| ()))?;

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
}
