//! analogLib device reference — query Cadence's `doc/analoglibref/` HTML.
//!
//! The sibling of [`crate::skill_finder`]: that one answers *"what is the
//! signature of `schCreateWire`?"*, this one answers *"what is the CDF
//! parameter for a `vsin`'s amplitude?"* — `va`, labelled *Amplitude 1 (Vpk)*,
//! not `ampl`, which does not exist and which the netlister will accept and
//! ignore.
//!
//! # Usage
//!
//! ```ignore
//! use virtuoso_cli::analoglib_finder::AnalogLibFinder;
//! use virtuoso_cli::skill_finder::SearchMode;
//!
//! let mut finder = AnalogLibFinder::new();
//! finder.load("/opt/Cadence/IC231/doc/analoglibref")?;
//! let vsin = finder.get("vsin").unwrap();
//! for hit in finder.search_params("amplitude", SearchMode::Fuzzy, 10) {
//!     println!("{}/{} — {}", hit.symbol.name, hit.param.name, hit.param.label);
//! }
//! ```
//!
//! # Remote cache
//!
//! Same shape as the SKILL Finder cache, and namespaced by Cadence release for
//! the same reason: basenames collide across installs, and a flat cache lets
//! one release silently overwrite another.
//!
//! - Cache path: `~/.cache/virtuoso_bridge/analoglib/<host>/`
//! - Cached names: `IC231__appA.html`, `IC231__analoglibref.json`, …
//!
//! Only releases reachable from the Cadence binaries actually on `PATH` are
//! collected. This box also has `IC618` and `ICADVM201` trees with their own
//! `analoglibref/`; mixing in documentation for a release nobody is running is
//! how you end up confidently reading the wrong manual.

#![allow(dead_code)]

mod parser;

// Re-exported as the module's public surface; the binaries use only a
// subset of it, the way `skill_finder` re-exports `parse_fnd_directory`.
#[allow(unused_imports)]
pub use parser::{
    parse_analoglib_directory, parse_analoglib_directory_reported, AnalogLibSymbol, CdfParam,
    ParseReport, APPENDIX_BASENAME, INDEX_BASENAME,
};

use crate::skill_finder::{cadence_env_setup, SearchMode};
use std::path::PathBuf;

/// Directory name, relative to a Cadence release root, holding the reference.
pub const DOC_SUBDIR: &str = "doc/analoglibref";

/// A parameter hit: which symbol, and which of its parameters matched.
#[derive(Debug, Clone, Copy)]
pub struct ParamHit<'a> {
    pub symbol: &'a AnalogLibSymbol,
    pub param: &'a CdfParam,
}

/// Queryable analogLib device reference.
#[derive(Debug, Default)]
pub struct AnalogLibFinder {
    source_dir: Option<PathBuf>,
    symbols: Vec<AnalogLibSymbol>,
    report: ParseReport,
    loaded: bool,
}

impl AnalogLibFinder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a finder over the given symbols, for tests.
    #[doc(hidden)]
    pub fn for_test(symbols: Vec<AnalogLibSymbol>) -> Self {
        Self {
            source_dir: None,
            report: ParseReport {
                indexed: symbols.len(),
                parsed: symbols.len(),
                ..Default::default()
            },
            symbols,
            loaded: true,
        }
    }

    /// Load from a directory holding `analoglibref.json` + the chapter HTML.
    pub fn load(&mut self, source_dir: impl Into<PathBuf>) -> std::io::Result<()> {
        let dir = source_dir.into();
        if !dir.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("analogLib reference directory not found: {}", dir.display()),
            ));
        }
        let (symbols, report) = parse_analoglib_directory_reported(&dir);
        self.source_dir = Some(dir);
        self.symbols = symbols;
        self.report = report;
        self.loaded = true;
        Ok(())
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    pub fn source_dir(&self) -> Option<&std::path::Path> {
        self.source_dir.as_deref()
    }

    /// What the last load could and could not parse.
    pub fn report(&self) -> &ParseReport {
        &self.report
    }

    pub fn symbols(&self) -> &[AnalogLibSymbol] {
        &self.symbols
    }

    /// Exact symbol lookup, case-insensitive.
    pub fn get(&self, name: &str) -> Option<&AnalogLibSymbol> {
        self.symbols
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Search symbol names (and titles/categories in fuzzy mode).
    pub fn search(&self, query: &str, mode: SearchMode, limit: usize) -> Vec<&AnalogLibSymbol> {
        let mut hits: Vec<&AnalogLibSymbol> = self
            .symbols
            .iter()
            .filter(|s| match_text(&s.name, query, mode) || {
                mode == SearchMode::Fuzzy
                    && (contains_ci(&s.title, query) || contains_ci(&s.category, query))
            })
            .collect();
        hits.sort_by(|a, b| a.name.cmp(&b.name));
        hits.into_iter().take(limit).collect()
    }

    /// Search CDF parameters across every symbol, by name, label or description.
    ///
    /// This is the query the `ampl`/`va` mistake needed: `"amplitude"` returns
    /// `va` (*Amplitude 1 (Vpk)*), `ia`, `vaDBm` and friends, each with the
    /// symbol it belongs to.
    pub fn search_params(&self, query: &str, mode: SearchMode, limit: usize) -> Vec<ParamHit<'_>> {
        let mut hits = Vec::new();
        for symbol in &self.symbols {
            for param in &symbol.params {
                let matched = match_text(&param.name, query, mode)
                    // `F7` is a real parameter of `vsin`; the manual just
                    // prints it as part of `F1 - F50`. Searching the name a
                    // caller will actually use has to find it.
                    || param
                        .expands_to
                        .iter()
                        .any(|n| match_text(n, query, mode))
                    || (mode == SearchMode::Fuzzy
                        && (contains_ci(&param.label, query)
                            || contains_ci(&param.description, query)));
                if matched {
                    hits.push(ParamHit { symbol, param });
                }
            }
        }
        // Exact CDF-name matches first, then name substrings, then the GUI
        // label by how squarely it matches, and only then prose. Without the
        // tiers, `"amplitude"` sorts purely by symbol name and the answer it
        // exists to give — `vsin`/`va`, labelled exactly *Amplitude* — lands at
        // rank 24 behind description-only hits, which reads as "vsin has no
        // amplitude parameter" to anyone who passed a sane `limit`.
        let q = query.to_lowercase();
        hits.sort_by(|a, b| {
            let rank = |h: &ParamHit| {
                let name = h.param.name.to_lowercase();
                let label = h.param.label.to_lowercase();
                if name == q || h.param.expands_to.iter().any(|n| n.to_lowercase() == q) {
                    0
                } else if name.contains(&q) {
                    1
                } else if label == q {
                    2
                } else if label.starts_with(&q) {
                    3
                } else if label.contains(&q) {
                    4
                } else {
                    5
                }
            };
            rank(a)
                .cmp(&rank(b))
                .then_with(|| a.symbol.name.cmp(&b.symbol.name))
                .then_with(|| a.param.name.cmp(&b.param.name))
        });
        hits.truncate(limit);
        hits
    }

    /// Closest symbol names to `query`, for "did you mean" output.
    pub fn suggest(&self, query: &str, limit: usize) -> Vec<&str> {
        let q = query.to_lowercase();
        let mut names: Vec<&str> = self
            .symbols
            .iter()
            .map(|s| s.name.as_str())
            .filter(|n| {
                let n = n.to_lowercase();
                n.contains(&q) || q.contains(&n) || shares_prefix(&n, &q, 3)
            })
            .collect();
        names.sort();
        names.truncate(limit);
        names
    }
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn shares_prefix(a: &str, b: &str, n: usize) -> bool {
    a.len() >= n && b.len() >= n && a[..n] == b[..n]
}

fn match_text(text: &str, query: &str, mode: SearchMode) -> bool {
    match mode {
        SearchMode::Exact => text.eq_ignore_ascii_case(query),
        SearchMode::Prefix => text.to_lowercase().starts_with(&query.to_lowercase()),
        SearchMode::Suffix => text.to_lowercase().ends_with(&query.to_lowercase()),
        SearchMode::Fuzzy => contains_ci(text, query),
        SearchMode::Regex => regex::Regex::new(&format!("(?i){query}"))
            .map(|r| r.is_match(text))
            .unwrap_or(false),
    }
}

// =============================================================================
// Cache management
// =============================================================================

/// `~/.cache/virtuoso_bridge/analoglib/<host>/`
pub fn cache_dir(host: &str) -> Option<PathBuf> {
    Some(crate::runtime_paths::cache_subdir(&["analoglib", host]))
}

pub fn cache_exists(host: &str) -> bool {
    cache_dir(host)
        .map(|d| d.exists() && d.is_dir())
        .unwrap_or(false)
}

/// Count cached documentation files (`.html` and the `.json` index).
pub fn cache_file_count(host: &str) -> usize {
    cache_dir(host)
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| is_doc_file(&e.path()))
                .count()
        })
        .unwrap_or(0)
}

fn is_doc_file(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "html" || ext == "json")
}

/// `/opt/Cadence/IC231/doc/analoglibref` → `IC231`
pub fn release_tag(doc_dir: &str) -> Option<&str> {
    doc_dir
        .trim_end_matches('/')
        .strip_suffix(&format!("/{DOC_SUBDIR}"))?
        .rsplit('/')
        .find(|s| !s.is_empty())
}

/// Local cache name for a remote documentation file, namespaced by release.
pub fn cache_file_name(doc_dir: &str, base: &str) -> String {
    match release_tag(doc_dir) {
        Some(tag) => format!("{tag}__{base}"),
        None => base.to_string(),
    }
}

/// Shell script printing every `doc/analoglibref` directory reachable from the
/// Cadence binaries on `PATH`, one per line, `virtuoso` tree first.
///
/// Same walk-up-from-the-binary probe as the SKILL Finder, and deliberately so:
/// the `which spectre || which virtuoso` short-circuit is what left that cache
/// holding 2 of 41 databases while every lookup answered "no such function".
pub fn remote_doc_probe_script(cadence_cshrc: Option<&str>) -> String {
    format!(
        r#"{}
walk_up() {{
  p="$1"
  while [ -n "$p" ] && [ "$p" != "/" ]; do
    if [ -d "$p/{sub}" ]; then echo "$p/{sub}"; return 0; fi
    p=$(dirname "$p")
  done
  return 1
}}
for b in virtuoso spectre; do
  x=$(command -v "$b" 2>/dev/null) || continue
  [ -n "$x" ] && walk_up "$x"
done"#,
        cadence_env_setup(cadence_cshrc),
        sub = DOC_SUBDIR
    )
}

/// Parse the probe output: trim, drop non-paths, de-duplicate in order.
pub fn parse_doc_dirs(stdout: &str) -> Vec<String> {
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

fn find_remote_doc_dirs(
    ssh_target: &str,
    cadence_cshrc: Option<&str>,
) -> std::io::Result<Vec<String>> {
    let output = std::process::Command::new("ssh")
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=30"])
        .arg(ssh_target)
        .arg(remote_doc_probe_script(cadence_cshrc))
        .output()
        .map_err(|e| std::io::Error::other(format!("SSH failed: {e}")))?;

    let dirs = parse_doc_dirs(&String::from_utf8_lossy(&output.stdout));
    if dirs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "No {DOC_SUBDIR} directory found near virtuoso/spectre on the remote server. \
                 Ensure Cadence is in PATH or set VB_CADENCE_CSHRC."
            ),
        ));
    }
    Ok(dirs)
}

/// Download the analogLib reference from a remote host into the local cache.
///
/// Only `*.html` and `analoglibref.json` are fetched — about 2.4 MB. The full
/// directory is 16 MB, the rest being images that nothing here reads.
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
    std::fs::create_dir_all(&cache)?;

    let remote_dirs = find_remote_doc_dirs(ssh_target, cadence_cshrc)?;
    if let Some(p) = progress {
        p(&format!(
            "Found {} remote analogLib reference dir(s): {}",
            remote_dirs.len(),
            remote_dirs.join(", ")
        ));
    }

    let mut synced = 0usize;
    let mut failed = 0usize;
    let mut written: Vec<std::ffi::OsString> = Vec::new();

    for remote_dir in &remote_dirs {
        let list_script = format!(
            r#"find {remote_dir} -maxdepth 1 -type f \( -name '*.html' -o -name '{INDEX_BASENAME}' \) 2>/dev/null | head -100"#
        );
        let output = Command::new("ssh")
            .args(["-o", "BatchMode=yes"])
            .args(["-o", "ConnectTimeout=30"])
            .arg(ssh_target)
            .arg(&list_script)
            .output()
            .map_err(|e| std::io::Error::other(format!("SSH failed: {e}")))?;

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
            p(&format!("Found {} files under {}", files.len(), remote_dir));
        }

        for remote_file in &files {
            let base = std::path::Path::new(remote_file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown.html");
            let file_name = cache_file_name(remote_dir, base);
            let local_path = cache.join(&file_name);

            let scp_result = Command::new("scp")
                .args(["-o", "BatchMode=yes"])
                .args(["-o", "ConnectTimeout=30"])
                .arg(format!("{ssh_target}:{remote_file}"))
                .arg(&local_path)
                .output();

            match scp_result {
                Ok(out) if out.status.success() => {
                    synced += 1;
                    written.push(std::ffi::OsString::from(&file_name));
                    if let Some(p) = progress {
                        p(&format!("Downloaded: {file_name}"));
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

    // A half-finished sync must not delete the copies we still have.
    if failed == 0 && synced > 0 {
        prune_stale_cache(&cache, &written, progress);
    }

    if let Some(p) = progress {
        p(&format!("Cache sync complete: {synced} files"));
    }
    Ok(synced)
}

fn prune_stale_cache<F>(cache: &std::path::Path, written: &[std::ffi::OsString], progress: Option<F>)
where
    F: Fn(&str) + Copy,
{
    let Ok(entries) = std::fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_doc_file(&path) {
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

/// Load from cache, syncing first if the cache is empty.
pub fn load_or_sync(
    finder: &mut AnalogLibFinder,
    host: &str,
    ssh_target: &str,
    cadence_cshrc: Option<&str>,
) -> std::io::Result<PathBuf> {
    if let Some(cache) = cache_dir(host) {
        if cache.exists() && cache_file_count(host) > 0 {
            finder.load(&cache)?;
            return Ok(cache);
        }
    }

    let _ = sync_from_remote(host, ssh_target, cadence_cshrc, Some(|_: &str| ()))?;

    let cache = cache_dir(host).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Cache directory not found")
    })?;
    finder.load(&cache)?;
    Ok(cache)
}

pub fn clear_cache(host: &str) -> std::io::Result<()> {
    if let Some(cache) = cache_dir(host) {
        if cache.exists() {
            std::fs::remove_dir_all(&cache)?;
        }
    }
    Ok(())
}

pub fn cache_info(host: &str) -> Option<CacheInfo> {
    let cache = cache_dir(host)?;
    if !cache.exists() {
        return None;
    }
    let file_count = cache_file_count(host);
    let modified = std::fs::metadata(&cache).ok()?.modified().ok();
    Some(CacheInfo {
        path: cache,
        file_count,
        modified,
    })
}

/// Information about a cached analogLib reference.
#[derive(Debug)]
pub struct CacheInfo {
    pub path: PathBuf,
    pub file_count: usize,
    pub modified: Option<std::time::SystemTime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(name: &str, label: &str, desc: &str) -> CdfParam {
        CdfParam {
            name: name.into(),
            label: label.into(),
            description: desc.into(),
            ..Default::default()
        }
    }

    fn corpus() -> AnalogLibFinder {
        AnalogLibFinder::for_test(vec![
            AnalogLibSymbol {
                name: "vsin".into(),
                category: "Sources - Independent".into(),
                title: "Independent Sinusoidal Voltage Source".into(),
                params: vec![
                    param("va", "Amplitude 1 (Vpk)", ""),
                    param("vaDBm", "Amplitude 1 (dBm)", ""),
                    param("acm", "AC magnitude", ""),
                    param("freq", "Frequency 1", "Frequency of the source"),
                ],
                release: "IC231".into(),
                source_file: "independent.html".into(),
                ..Default::default()
            },
            AnalogLibSymbol {
                name: "idc".into(),
                category: "Sources - Independent".into(),
                title: "Independent DC Current Source".into(),
                params: vec![param("ia", "Amplitude 1 (Apk)", ""), param("idc", "DC current", "")],
                release: "IC231".into(),
                source_file: "independent.html".into(),
                ..Default::default()
            },
            AnalogLibSymbol {
                name: "cap".into(),
                category: "Passive Components".into(),
                title: "Two Terminal Capacitor".into(),
                params: vec![param("c", "Capacitance", "Capacitance")],
                release: "IC231".into(),
                source_file: "passives.html".into(),
                ..Default::default()
            },
        ])
    }

    /// Prose matches must not outrank the parameter the query is about.
    ///
    /// On the real corpus `"amplitude"` has 31 hits; before the tiers, six
    /// description-only matches interleaved alphabetically ahead of `vsin`/`va`,
    /// pushing it to rank 24 — invisible at any sane `limit`.
    #[test]
    fn label_matches_outrank_description_only_matches() {
        let mut symbols = corpus().symbols().to_vec();
        symbols.push(AnalogLibSymbol {
            // Sorts before `vsin` alphabetically, and matches only in prose.
            name: "aaa_prose".into(),
            params: vec![param("zz", "Modulation", "Sets the amplitude modulation type")],
            ..Default::default()
        });
        let f = AnalogLibFinder::for_test(symbols);
        let hits = f.search_params("amplitude", SearchMode::Fuzzy, 3);
        assert!(
            hits.iter().all(|h| h.param.name != "zz"),
            "a description-only hit outranked the labelled ones"
        );
        assert!(hits.iter().any(|h| h.param.name == "va"));
    }

    /// Looking up `F7` must find the row that documents it, even though no
    /// row is literally named `F7`.
    #[test]
    fn a_name_inside_a_documented_range_is_searchable() {
        let f = AnalogLibFinder::for_test(vec![AnalogLibSymbol {
            name: "vsin".into(),
            params: vec![
                param("va", "Amplitude", ""),
                CdfParam {
                    name: "F1 - F50".into(),
                    label: "Freq 1 to Freq 50".into(),
                    expands_to: (1..=50).map(|i| format!("F{i}")).collect(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }]);
        let hits = f.search_params("F7", SearchMode::Exact, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].param.name, "F1 - F50");
        assert!(f.search_params("F99", SearchMode::Exact, 10).is_empty());
    }

    #[test]
    fn get_is_case_insensitive_and_exact() {
        let f = corpus();
        assert_eq!(f.get("VSIN").unwrap().name, "vsin");
        assert!(f.get("vsi").is_none(), "get() is exact, not a prefix match");
    }

    /// The lookup the `ampl=5m` mistake needed: searching the human word finds
    /// the CDF name, with the label that tells you which one you want.
    #[test]
    fn searching_amplitude_finds_va_with_its_label() {
        let f = corpus();
        let hits = f.search_params("amplitude", SearchMode::Fuzzy, 10);
        let found: Vec<(&str, &str)> = hits
            .iter()
            .map(|h| (h.symbol.name.as_str(), h.param.name.as_str()))
            .collect();
        assert!(found.contains(&("vsin", "va")));
        assert!(found.contains(&("vsin", "vaDBm")));
        assert!(found.contains(&("idc", "ia")));
        assert_eq!(
            hits.iter().find(|h| h.param.name == "va").unwrap().param.label,
            "Amplitude 1 (Vpk)"
        );
    }

    /// `ampl` is not a CDF parameter of anything. Saying so is the whole point.
    #[test]
    fn the_guessed_name_matches_nothing() {
        assert!(corpus().search_params("ampl", SearchMode::Exact, 10).is_empty());
    }

    #[test]
    fn exact_cdf_name_hits_rank_before_prose_hits() {
        let f = corpus();
        let hits = f.search_params("c", SearchMode::Fuzzy, 10);
        assert_eq!(hits[0].param.name, "c", "the parameter named `c` leads");
        assert!(hits.len() > 1, "prose matches still come back, just after");
    }

    #[test]
    fn search_modes_apply_to_symbol_names() {
        let f = corpus();
        assert_eq!(f.search("v", SearchMode::Prefix, 10).len(), 1);
        assert_eq!(f.search("dc", SearchMode::Suffix, 10)[0].name, "idc");
        assert_eq!(f.search("cap", SearchMode::Exact, 10).len(), 1);
        assert_eq!(f.search("^c.p$", SearchMode::Regex, 10)[0].name, "cap");
    }

    #[test]
    fn fuzzy_symbol_search_also_matches_title_and_category() {
        let f = corpus();
        assert_eq!(f.search("sinusoidal", SearchMode::Fuzzy, 10)[0].name, "vsin");
        assert_eq!(f.search("Passive", SearchMode::Fuzzy, 10)[0].name, "cap");
        // Non-fuzzy modes stay name-shape filters.
        assert!(f.search("sinusoidal", SearchMode::Prefix, 10).is_empty());
    }

    #[test]
    fn limit_is_honoured_by_both_searches() {
        let f = corpus();
        assert_eq!(f.search("", SearchMode::Fuzzy, 2).len(), 2);
        assert_eq!(f.search_params("a", SearchMode::Fuzzy, 3).len(), 3);
    }

    #[test]
    fn suggest_offers_near_misses() {
        assert!(corpus().suggest("vsine", 5).contains(&"vsin"));
        assert!(corpus().suggest("capa", 5).contains(&"cap"));
    }

    #[test]
    fn an_unloaded_finder_is_empty_rather_than_wrong() {
        let f = AnalogLibFinder::new();
        assert!(!f.is_loaded());
        assert!(f.get("vsin").is_none());
        assert_eq!(f.search_params("amplitude", SearchMode::Fuzzy, 10).len(), 0);
    }

    #[test]
    fn load_reports_a_missing_directory_instead_of_loading_nothing() {
        let mut f = AnalogLibFinder::new();
        let err = f.load("/nonexistent/analoglibref").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(!f.is_loaded());
    }

    #[test]
    fn release_tag_and_cache_names_namespace_by_release() {
        assert_eq!(
            release_tag("/opt/Cadence/IC231/doc/analoglibref"),
            Some("IC231")
        );
        assert_eq!(
            release_tag("/opt/Cadence/IC618/doc/analoglibref/"),
            Some("IC618")
        );
        assert_eq!(release_tag("/opt/Cadence/IC231/doc/finder/SKILL"), None);
        assert_eq!(
            cache_file_name("/opt/Cadence/IC231/doc/analoglibref", "appA.html"),
            "IC231__appA.html"
        );
        assert_eq!(cache_file_name("/somewhere/else", "appA.html"), "appA.html");
    }

    /// The probe must try both binaries. `which spectre || which virtuoso`
    /// short-circuiting on a Spectre-only tree is defect K; do not reintroduce
    /// the shape here.
    #[test]
    fn probe_script_walks_up_from_both_binaries() {
        let script = remote_doc_probe_script(None);
        assert!(script.contains("for b in virtuoso spectre"));
        assert!(script.contains("walk_up"));
        assert!(script.contains(DOC_SUBDIR));
        // A `||` *inside* the loop body (`command -v … || continue`) is
        // per-binary and correct. What defect K needs excluded is one
        // expression that stops once the first binary resolves.
        assert!(
            !script
                .lines()
                .any(|l| l.contains("||") && l.contains("virtuoso") && l.contains("spectre")),
            "no short-circuit between binaries"
        );
        assert!(
            !script.contains("break"),
            "the loop must visit both binaries, not stop at the first hit"
        );
    }

    #[test]
    fn probe_output_is_deduplicated_in_discovery_order() {
        let dirs = parse_doc_dirs(
            "/opt/Cadence/IC231/doc/analoglibref\n\
             \n  /opt/Cadence/SPECTRE231/doc/analoglibref  \n\
             /opt/Cadence/IC231/doc/analoglibref\n\
             NOTFOUND\nsome noise\n",
        );
        assert_eq!(
            dirs,
            vec![
                "/opt/Cadence/IC231/doc/analoglibref",
                "/opt/Cadence/SPECTRE231/doc/analoglibref"
            ]
        );
    }

    #[test]
    fn only_documentation_files_count_as_cache_content() {
        assert!(is_doc_file(std::path::Path::new("IC231__appA.html")));
        assert!(is_doc_file(std::path::Path::new("IC231__analoglibref.json")));
        assert!(!is_doc_file(std::path::Path::new("IC231__skdfref.fnd")));
        assert!(!is_doc_file(std::path::Path::new("notes.txt")));
    }
}
