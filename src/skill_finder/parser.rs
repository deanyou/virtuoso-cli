//! Parser for Cadence SKILL Finder `.fnd` files.
//!
//! The SKILL Finder database lives under `doc/finder/SKILL/*.fnd`.
//! Each entry has three fields: name, syntax, and description.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, warn};

/// A single SKILL function entry from the .fnd database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Function name
    pub name: String,
    /// Function signature (Lisp-style parameter notation)
    pub syntax: String,
    /// One-line description
    pub description: String,
    /// Source .fnd filename
    #[serde(default)]
    pub source_file: Option<String>,
}

impl SkillEntry {
    /// Format entry for human-readable CLI output
    pub fn format(&self) -> String {
        let desc = self.description.trim_matches('"').trim();
        let syntax = collapse_whitespace(&self.syntax.trim_matches('"'));
        let source = self
            .source_file
            .as_ref()
            .map(|s| format!(" [{}]", s))
            .unwrap_or_default();

        format!(
            " {}{}\n Syntax : {}\n Desc : {}\n",
            self.name, source, syntax, desc
        )
    }
}

/// Collapse multiple whitespace characters into a single space.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a single .fnd file content.
/// Format: each entry is separated by blank lines with fields on separate lines.
/// Field order: name, syntax, description
fn parse_fnd_content(content: &str, source_file: &str) -> Vec<SkillEntry> {
    let mut entries = Vec::new();

    for block in content.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        if lines.len() < 3 {
            continue;
        }

        // Remove empty lines within the block
        let lines: Vec<&str> = lines.iter().map(|l| *l).filter(|l| !l.trim().is_empty()).collect();
        if lines.len() < 3 {
            continue;
        }

        // In .fnd files, the first line is the name, second is syntax, rest is description
        // But sometimes there are embedded newlines in syntax
        let name = lines[0].trim().to_string();

        // Find where description starts (usually after a line that looks like a closing paren)
        let syntax_end = lines
            .iter()
            .position(|l| l.trim().ends_with(')'))
            .map(|i| i + 1)
            .unwrap_or(2)
            .min(lines.len());

        let syntax = lines[1..syntax_end].join(" ");
        let description = lines[syntax_end..].join(" ");

        if !name.is_empty() {
            entries.push(SkillEntry {
                name,
                syntax: syntax.trim().to_string(),
                description: description.trim().to_string(),
                source_file: Some(source_file.to_string()),
            });
        }
    }

    entries
}

/// Parse all .fnd files in a directory.
pub fn parse_fnd_directory(dir: &Path) -> Vec<SkillEntry> {
    let mut all_entries = Vec::new();

    if !dir.exists() {
        warn!("SKILL Finder directory does not exist: {}", dir.display());
        return all_entries;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("Failed to read directory {}: {}", dir.display(), e);
            return all_entries;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "fnd") {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let file_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let entries = parse_fnd_content(&content, &file_name);
                    debug!(
                        "Parsed {} entries from {}",
                        entries.len(),
                        path.display()
                    );
                    all_entries.extend(entries);
                }
                Err(e) => {
                    warn!("Failed to read {}: {}", path.display(), e);
                }
            }
        }
    }

    all_entries
}

/// Alternative .fnd format: one entry per file with name@syntax@description
/// Returns a map of name -> SkillEntry
pub fn parse_fnd_map(dir: &Path) -> HashMap<String, SkillEntry> {
    let entries = parse_fnd_directory(dir);
    entries
        .into_iter()
        .map(|e| (e.name.clone(), e))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fnd_content_basic() {
        let content = r#"dbOpenCellView
dbOpenCellView(gt_lib t_cellName lt_viewName [t_viewTypeName] [t_mode] [d_contextCellView]) => d_cellView / nil
Opens a cellView in the database"#;

        let entries = parse_fnd_content(content, "test.fnd");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "dbOpenCellView");
        assert!(entries[0].syntax.contains("gt_lib"));
        assert_eq!(entries[0].source_file, Some("test.fnd".to_string()));
    }

    #[test]
    fn test_parse_fnd_content_multiple() {
        let content = r#"func1
syntax1
desc1

func2
syntax2
desc2"#;

        let entries = parse_fnd_content(content, "multi.fnd");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "func1");
        assert_eq!(entries[1].name, "func2");
    }

    #[test]
    fn test_collapse_whitespace() {
        assert_eq!(collapse_whitespace("a  b\n  c"), "a b c");
        assert_eq!(collapse_whitespace("a\nb"), "a b");
    }

    #[test]
    fn test_skill_entry_format() {
        let entry = SkillEntry {
            name: "dbOpenCellView".to_string(),
            syntax: "dbOpenCellView( lib cell view )".to_string(),
            description: "\"Opens a cellView\"".to_string(),
            source_file: Some("test.fnd".to_string()),
        };
        let formatted = entry.format();
        assert!(formatted.contains("dbOpenCellView"));
        assert!(formatted.contains("test.fnd"));
        assert!(formatted.contains("Syntax :"));
        assert!(formatted.contains("Desc :"));
    }
}
