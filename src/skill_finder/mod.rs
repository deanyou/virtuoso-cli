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
//! # Search Modes
//!
//! - `Fuzzy`: Case-insensitive substring match (default)
//! - `Prefix`: Name starts with query
//! - `Suffix`: Name ends with query
//! - `Exact`: Exact name match
//! - `Regex`: Regular expression match

mod parser;

pub use parser::{parse_fnd_directory, parse_fnd_map, SkillEntry};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Search mode for SKILL Finder queries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Case-insensitive substring match (default)
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

impl Default for SearchMode {
    fn default() -> Self {
        Self::Fuzzy
    }
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
        self.entries
            .iter()
            .filter(|e| e.name == query)
            .collect()
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

        let header = format!("SKILL Finder — {} result(s) for '{}':\n", results.len(), query);
        let lines: Vec<String> = results.iter().map(|e| e.format()).collect();
        header + &lines.join("\n")
    }
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
}
