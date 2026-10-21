//! MCP resources — the repository's Claude skills published as `skill://<name>`.
//!
//! Discovery walks up from the working directory to the nearest `.claude/skills`
//! directory (`VB_SKILLS_DIR` overrides it). Every skill that owns a
//! `SKILL.md` becomes one read-only resource whose body is that file's text.
//!
//! Path safety is a hard requirement, not a nicety: a resource URI carries a
//! *skill name*, never a path. Names containing `..`, a path separator, or an
//! absolute prefix are rejected before any filesystem access happens, and the
//! resolved file is re-checked against the canonicalized skills root so that a
//! symlink cannot escape it either.

use serde_json::{json, Value};
use std::fmt;
use std::path::{Path, PathBuf};

/// URI scheme of every skill resource.
pub const SKILL_URI_SCHEME: &str = "skill://";
/// File that defines a skill.
pub const SKILL_FILE_NAME: &str = "SKILL.md";
/// MIME type of a skill resource body.
pub const SKILL_MIME_TYPE: &str = "text/markdown";
/// Env override for the skills root.
pub const SKILLS_DIR_ENV: &str = "VB_SKILLS_DIR";
/// Repository-relative location of the skills root.
pub const SKILLS_RELATIVE_DIR: &str = ".claude/skills";

/// One repository skill published as an MCP resource.
#[derive(Debug, Clone)]
pub struct SkillResource {
    /// Skill name (the directory name under the skills root).
    pub name: String,
    /// `skill://<name>`.
    pub uri: String,
    /// Frontmatter description, empty when the skill declares none.
    pub description: String,
}

impl SkillResource {
    /// The `resources/list` entry for this skill.
    pub fn to_json(&self) -> Value {
        json!({
            "uri": self.uri,
            "name": self.name,
            "description": self.description,
            "mimeType": SKILL_MIME_TYPE,
        })
    }
}

/// Why a resource could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceError {
    /// The URI is malformed or tries to escape the skills root.
    InvalidUri(String),
    /// The URI is well formed but names no known skill.
    NotFound(String),
    /// The skill exists but its body could not be read.
    Io(String),
}

impl ResourceError {
    /// JSON-RPC error code: invalid params for a bad URI, a server error for a
    /// missing or unreadable resource.
    pub fn code(&self) -> i32 {
        match self {
            ResourceError::InvalidUri(_) => -32602,
            ResourceError::NotFound(_) | ResourceError::Io(_) => -32001,
        }
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceError::InvalidUri(m) => write!(f, "invalid resource uri: {m}"),
            ResourceError::NotFound(m) => write!(f, "resource not found: {m}"),
            ResourceError::Io(m) => write!(f, "failed to read resource: {m}"),
        }
    }
}

/// Locate the skills root: env override first, then the nearest ancestor of
/// the working directory that owns `.claude/skills`.
pub fn discover_skills_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(SKILLS_DIR_ENV) {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if path.is_dir() {
                return Some(path);
            }
        }
    }
    let cwd = std::env::current_dir().ok()?;
    find_skills_dir(&cwd)
}

/// Nearest ancestor of `start` that contains `.claude/skills`.
pub fn find_skills_dir(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join(SKILLS_RELATIVE_DIR);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// Load every skill under `skills_dir`. A missing or empty directory yields an
/// empty list — never an error.
pub fn load_skills(skills_dir: &Path) -> Vec<SkillResource> {
    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return Vec::new();
    };

    let mut skills: Vec<SkillResource> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let skill_file = path.join(SKILL_FILE_NAME);
            if !skill_file.is_file() {
                return None;
            }
            let description = std::fs::read_to_string(&skill_file)
                .map(|content| frontmatter_description(&content))
                .unwrap_or_default();
            Some(SkillResource {
                uri: format!("{SKILL_URI_SCHEME}{name}"),
                name,
                description,
            })
        })
        .collect();

    // Stable, deterministic order — clients list resources in this order.
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Extract the skill name from a `skill://<name>` URI.
///
/// Rejects anything that is not a bare skill name: `..`, path separators,
/// absolute paths, and control characters never reach the filesystem.
pub fn parse_skill_uri(uri: &str) -> Result<String, ResourceError> {
    let name = uri.strip_prefix(SKILL_URI_SCHEME).ok_or_else(|| {
        ResourceError::InvalidUri(format!(
            "expected a {SKILL_URI_SCHEME}<skill> uri, got '{uri}'"
        ))
    })?;

    if name.is_empty() {
        return Err(ResourceError::InvalidUri("missing skill name".into()));
    }
    if name.contains("..") {
        return Err(ResourceError::InvalidUri(format!(
            "skill name '{name}' contains '..'"
        )));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(ResourceError::InvalidUri(format!(
            "skill name '{name}' contains a path separator"
        )));
    }
    if Path::new(name).is_absolute() {
        return Err(ResourceError::InvalidUri(format!(
            "skill name '{name}' is an absolute path"
        )));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(ResourceError::InvalidUri(format!(
            "skill name '{name}' contains control characters"
        )));
    }

    Ok(name.to_string())
}

/// Read the body of a skill resource.
///
/// The URI is validated first, then the file is resolved under the skills root
/// and re-checked against the canonicalized root, so neither `..` nor a
/// symlink can leave the skills directory.
pub fn read_skill(skills_dir: &Path, uri: &str) -> Result<String, ResourceError> {
    let name = parse_skill_uri(uri)?;
    let path = resolve_skill_path(skills_dir, &name)?;
    std::fs::read_to_string(&path)
        .map_err(|e| ResourceError::Io(format!("{}: {e}", path.display())))
}

/// Resolve `<skills_dir>/<name>/SKILL.md`, proving it stays inside the root.
fn resolve_skill_path(skills_dir: &Path, name: &str) -> Result<PathBuf, ResourceError> {
    let root = skills_dir
        .canonicalize()
        .map_err(|e| ResourceError::Io(format!("{}: {e}", skills_dir.display())))?;
    let candidate = root.join(name).join(SKILL_FILE_NAME);
    let resolved = candidate
        .canonicalize()
        .map_err(|_| ResourceError::NotFound(format!("no skill named '{name}'")))?;

    if !resolved.starts_with(&root) {
        return Err(ResourceError::InvalidUri(format!(
            "skill '{name}' resolves outside the skills directory"
        )));
    }
    if resolved.file_name().and_then(|n| n.to_str()) != Some(SKILL_FILE_NAME) {
        return Err(ResourceError::InvalidUri(format!(
            "skill '{name}' does not resolve to {SKILL_FILE_NAME}"
        )));
    }
    if !resolved.is_file() {
        return Err(ResourceError::NotFound(format!("no skill named '{name}'")));
    }
    Ok(resolved)
}

/// Read the `description` field from a SKILL.md frontmatter block.
///
/// Supports the three shapes the repository actually uses: a plain scalar, a
/// quoted scalar, and a `|`/`>` block. A missing or unparsable frontmatter
/// yields an empty description — never a fabricated one.
pub fn frontmatter_description(content: &str) -> String {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }

    let mut key: Option<String> = None;
    let mut scalar = String::new();
    let mut block: Vec<String> = Vec::new();
    let mut in_block = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if in_block {
            if line.starts_with(' ') || line.starts_with('\t') {
                block.push(line.trim().to_string());
                continue;
            }
            in_block = false;
        }
        let Some((candidate_key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let candidate_key = candidate_key.trim().to_lowercase();
        if candidate_key != "description" {
            continue;
        }
        key = Some(candidate_key);
        let value = value.trim();
        if value == "|" || value == ">" || value == "|-" || value == ">-" {
            in_block = true;
            continue;
        }
        scalar = value
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_string();
    }

    match key {
        Some(_) if !block.is_empty() => block.join(" ").trim().to_string(),
        Some(_) => scalar,
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, body: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE_NAME), body).unwrap();
    }

    #[test]
    fn load_skills_lists_every_skill_with_its_uri() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(
            tmp.path(),
            "maestro",
            "---\nname: maestro\ndescription: \"Maestro sessions\"\n---\nbody\n",
        );
        write_skill(
            tmp.path(),
            "sim-run",
            "---\nname: sim-run\ndescription: |\n  Run a simulation.\n  Second line.\n---\nbody\n",
        );

        let skills = load_skills(tmp.path());
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "maestro");
        assert_eq!(skills[0].uri, "skill://maestro");
        assert_eq!(skills[0].description, "Maestro sessions");
        assert_eq!(skills[1].name, "sim-run");
        assert_eq!(skills[1].description, "Run a simulation. Second line.");
    }

    #[test]
    fn load_skills_ignores_directories_without_a_skill_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("empty")).unwrap();
        write_skill(tmp.path(), "real", "---\nname: real\n---\n");

        let skills = load_skills(tmp.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "real");
    }

    #[test]
    fn load_skills_on_a_missing_directory_is_empty_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_skills(&tmp.path().join("nope")).is_empty());
    }

    #[test]
    fn frontmatter_description_handles_plain_quoted_and_block_forms() {
        assert_eq!(frontmatter_description("plain line\n"), "");
        assert_eq!(
            frontmatter_description("---\ndescription: plain\n---\n"),
            "plain"
        );
        assert_eq!(
            frontmatter_description("---\ndescription: \"quoted\"\n---\n"),
            "quoted"
        );
        assert_eq!(
            frontmatter_description("---\ndescription: |\n  one\n  two\n---\n"),
            "one two"
        );
        assert_eq!(
            frontmatter_description("---\ndescription: >\n  folded\n  text\n---\n"),
            "folded text"
        );
        assert_eq!(frontmatter_description("---\nname: x\n---\n"), "");
    }

    #[test]
    fn parse_skill_uri_accepts_a_bare_skill_name() {
        assert_eq!(parse_skill_uri("skill://maestro").unwrap(), "maestro");
    }

    #[test]
    fn parse_skill_uri_rejects_traversal_and_path_shaped_names() {
        for uri in [
            "skill://../etc/passwd",
            "skill://..",
            "skill://a/../../b",
            "skill://nested/skill",
            "skill://nested\\skill",
            "skill:///etc/passwd",
            "skill://",
            "file:///etc/passwd",
            "skill://na\u{0}me",
        ] {
            let err = parse_skill_uri(uri).unwrap_err();
            assert_eq!(err.code(), -32602, "uri '{uri}' must be rejected");
        }
    }

    #[test]
    fn read_skill_returns_the_full_skill_text() {
        let tmp = tempfile::tempdir().unwrap();
        let body = "---\nname: demo\ndescription: Demo\n---\n\n# Demo\n\nbody text\n";
        write_skill(tmp.path(), "demo", body);

        let text = read_skill(tmp.path(), "skill://demo").unwrap();
        assert_eq!(text, body);
    }

    #[test]
    fn read_skill_rejects_unknown_skills() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill(tmp.path(), "demo", "body");

        let err = read_skill(tmp.path(), "skill://missing").unwrap_err();
        assert_eq!(err.code(), -32001);
    }

    #[test]
    fn read_skill_refuses_to_escape_the_skills_root() {
        let tmp = tempfile::tempdir().unwrap();
        let secret = tmp.path().join("secret.md");
        std::fs::write(&secret, "top secret").unwrap();
        let root = tmp.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        write_skill(&root, "demo", "body");

        // A symlink inside the root must not become a read primitive.
        #[cfg(unix)]
        {
            let link_dir = root.join("linked");
            std::fs::create_dir_all(&link_dir).unwrap();
            std::os::unix::fs::symlink(&secret, link_dir.join("SKILL.md")).unwrap();
            let err = read_skill(&root, "skill://linked").unwrap_err();
            assert_eq!(err.code(), -32602, "symlinked SKILL.md must not be served");
        }

        // `..` never even reaches the filesystem.
        let err = read_skill(&root, "skill://../secret").unwrap_err();
        assert_eq!(err.code(), -32602);
    }
}
