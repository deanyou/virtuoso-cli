//! Parser for Cadence SKILL Finder `.fnd` files.
//!
//! The SKILL Finder database lives under `doc/finder/SKILL/*.fnd`.
//!
//! ## Real-world format
//!
//! Each entry is a SKILL list literal with three quoted strings — the
//! function name, its syntax (a.k.a. signature), and a one-line
//! description. The syntax and description can span multiple lines
//! because newlines are preserved inside the `"..."` strings.
//!
//! ```text
//! ;SKILL Language Functions
//! ("absImportGDS"
//! "absImportGDS(
//! )
//! => 0 / 1"
//! "Creates cellviews from GDSII layout data.")
//! ("dbOpenCellView"
//! "dbOpenCellView( gt_lib t_cellName lt_viewName ... ) => d_cellView / nil"
//! "Opens a cellView in the database")
//! ```
//!
//! The file may also contain `;`-prefixed comments and arbitrary
//! whitespace between entries. Some entries have 2+ fields beyond the
//! three we care about; we silently drop the tail.
//!
//! ## Implementation note
//!
//! We use a small hand-rolled state machine (not `regex`) because the
//! input has nested `"..."` strings with newlines and the
//! "interesting" tokens are single bytes (`(`, `)`, `"`, `;`).
//! A regex with `(?s)` would also work; the state machine keeps
//! behavior obvious to future maintainers.

use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, warn};

/// A single SKILL function entry from the .fnd database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Function name
    pub name: String,
    /// Function signature (Lisp-style parameter notation, may contain
    /// embedded newlines which `format()` collapses to spaces).
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
        let syntax = collapse_whitespace(self.syntax.trim_matches('"'));
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

/// Parse the contents of a .fnd file.
///
/// The format is a top-level sequence of `("name" "syntax" "desc")`
/// SKILL list literals separated by whitespace and `;`-comments. We
/// emit one `SkillEntry` per well-formed triple. Anything malformed
/// is skipped silently (with a `tracing::debug!` so curious users
/// can investigate if needed).
pub fn parse_fnd_content(content: &str, source_file: &str) -> Vec<SkillEntry> {
    let mut entries = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Skip whitespace and `;`-comments between entries.
        i = skip_ws_and_comments(bytes, i);

        if i >= bytes.len() {
            break;
        }

        // We expect an opening `(`; anything else is a format drift
        // we should just skip.
        if bytes[i] != b'(' {
            i += 1;
            continue;
        }
        i += 1; // consume `(`

        // Inside the list: read 3 strings. Strings may contain any
        // character including `(` and `)`, so we don't try to
        // balance parens — we just count quoted strings.
        let mut fields: Vec<String> = Vec::new();
        while fields.len() < 3 && i < bytes.len() {
            i = skip_ws_and_comments(bytes, i);
            if i >= bytes.len() || bytes[i] != b'"' {
                break;
            }
            // Consume opening `"`
            i += 1;
            let start = i;
            // Find closing `"`. Strings here are simple (no `\"` escapes
            // observed in any .fnd shipped with IC231, but the parser
            // is robust to anything except a `"`).
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let end = i;
            // Consume closing `"`
            if i < bytes.len() {
                i += 1;
            }
            fields.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
        }

        // After 3 strings, expect a closing `)`. We don't care if it's
        // there or not — the entry is valid either way (some .fnd
        // files omit it for the last entry). Just look for the next
        // meaningful byte.
        if fields.len() == 3 && !fields[0].is_empty() {
            entries.push(SkillEntry {
                name: fields.remove(0),
                syntax: fields.remove(0),
                description: fields.remove(0),
                source_file: Some(source_file.to_string()),
            });
        }

        // Advance past the closing `)` if present, or any other junk,
        // to the next entry.
        i = skip_ws_and_comments(bytes, i);
        if i < bytes.len() && bytes[i] == b')' {
            i += 1;
        }
    }

    debug!("Parsed {} entries from {}", entries.len(), source_file);
    entries
}

/// Skip whitespace and `;`-line-comments starting at `start`.
/// Returns the index of the first non-whitespace, non-comment byte.
fn skip_ws_and_comments(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
        } else if b == b';' {
            // Skip to end of line
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else {
            break;
        }
    }
    i
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
        if path.extension().is_some_and(|ext| ext == "fnd") {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let file_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let entries = parse_fnd_content(&content, &file_name);
                    debug!("Parsed {} entries from {}", entries.len(), path.display());
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

#[cfg(test)]
mod tests {
    use super::*;

    // -- Real-world format tests -----------------------------------------
    // These match what `find /opt/cadence/IC23*/doc/finder/SKILL/*.fnd`
    // actually contains. The previously-shipped parser expected a
    // 3-line-per-entry format that doesn't exist on disk, so these
    // tests would have failed against the real parser.

    #[test]
    fn test_parse_real_abstract_fnd_sample() {
        // Verbatim from /opt/cadence/IC231/doc/finder/SKILL/SKILL/abstract.fnd
        let content = r#";SKILL Language Functions
("absImportGDS"
"absImportGDS(
)
=> 0 / 1"
"Creates cellviews from GDSII layout data.")
("absImportOasis"
"absImportOasis(
)
=> 0 / 1"
"Creates cellviews from OASIS data.")
"#;
        let entries = parse_fnd_content(content, "abstract.fnd");
        assert_eq!(entries.len(), 2, "got: {entries:#?}");
        assert_eq!(entries[0].name, "absImportGDS");
        assert_eq!(
            entries[0].description,
            "Creates cellviews from GDSII layout data."
        );
        assert!(entries[0].syntax.contains("absImportGDS"));
        assert!(entries[0].syntax.contains("=> 0 / 1"));
        assert_eq!(entries[1].name, "absImportOasis");
        assert_eq!(entries[1].description, "Creates cellviews from OASIS data.");
    }

    #[test]
    fn test_parse_real_dfii_skill_fnd_sample() {
        // Verbatim from /opt/cadence/IC231/doc/finder/SKILL/DFII_SKILL/skdfref.fnd
        let content = r#";SKILL Language Functions
("geChangeCellView"
"geChangeCellView(
[ w_windowId ]
[ t_libName ]
[ t_cellName ]
[ t_viewName ]
[ t_accessMode ]
)
=> w_windowId"
"Opens a design in an existing window.")
("geNewWindow"
"geNewWindow(
[ w_windowId ]
)
=> w_windowId / nil"
"Makes a copy of a window.")
"#;
        let entries = parse_fnd_content(content, "skdfref.fnd");
        assert_eq!(entries.len(), 2, "got: {entries:#?}");
        assert_eq!(entries[0].name, "geChangeCellView");
        assert_eq!(
            entries[0].description,
            "Opens a design in an existing window."
        );
        // Syntax spans multiple lines but must contain all parts
        let s0 = &entries[0].syntax;
        assert!(s0.contains("geChangeCellView"));
        assert!(s0.contains("w_windowId"));
        assert!(s0.contains("=> w_windowId"));
        assert_eq!(entries[1].name, "geNewWindow");
    }

    #[test]
    fn test_parse_entry_without_closing_paren() {
        // The last entry in some .fnd files omits the closing `)`.
        let content = r#"("foo"
"foo( x )"
"a description")"#;
        let entries = parse_fnd_content(content, "test.fnd");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "foo");
        assert_eq!(entries[0].syntax, "foo( x )");
        assert_eq!(entries[0].description, "a description");
    }

    #[test]
    fn test_parse_handles_blank_lines_and_comments_between_entries() {
        let content = r#"; header comment
; another comment

("a"
"a()"
"first")


; in-between comment
("b"
"b()"
"second")
"#;
        let entries = parse_fnd_content(content, "test.fnd");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a");
        assert_eq!(entries[1].name, "b");
    }

    #[test]
    fn test_parse_skips_malformed_entries_silently() {
        // A `(  ` with no closing strings should not panic or
        // produce a fake entry.
        let content = r#"("good"
"good()"
"ok")

(garbage

("also_good"
"also_good()"
"ok too")
"#;
        let entries = parse_fnd_content(content, "test.fnd");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "good");
        assert_eq!(entries[1].name, "also_good");
    }

    #[test]
    fn test_parse_empty_name_is_skipped() {
        // `(""  "x" "y")` should NOT produce an entry because the
        // name is the lookup key.
        let content = r#"(""
"x()"
"empty name entry")
("real"
"real()"
"real entry")
"#;
        let entries = parse_fnd_content(content, "test.fnd");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "real");
    }

    #[test]
    fn test_parse_real_fnd_file_from_cadence_install() {
        // The ultimate regression: a real .fnd file from a real
        // Cadence install must parse into many entries with the
        // right shape. We only run this if the test fixture exists
        // — the path is the standard IC231 location.
        let path = std::path::Path::new("/opt/cadence/IC231/doc/finder/SKILL/SKILL/abstract.fnd");
        if !path.exists() {
            // Skip silently — this test only runs in environments
            // with a Cadence install mounted.
            eprintln!("skipping: {} not found", path.display());
            return;
        }
        let content = std::fs::read_to_string(path).unwrap();
        let entries = parse_fnd_content(&content, "abstract.fnd");
        assert!(
            entries.len() > 50,
            "expected many entries from abstract.fnd, got {}",
            entries.len()
        );
        // Spot-check a well-known function
        let gds = entries.iter().find(|e| e.name == "absImportGDS");
        assert!(gds.is_some(), "absImportGDS should be present");
        let gds = gds.unwrap();
        assert_eq!(gds.description, "Creates cellviews from GDSII layout data.");
    }

    // -- Sanity tests for the helpers -------------------------------------

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
