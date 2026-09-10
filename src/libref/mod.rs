//! Virtuoso library reference — query the manuals Cadence ships under `doc/`.
//!
//! The sibling of [`crate::skill_finder`]: that one answers *"what is the
//! signature of `schCreateWire`?"*, this one answers *"what is the CDF
//! parameter for a `vsin`'s amplitude?"* — `va`, labelled *Amplitude 1 (Vpk)*,
//! not `ampl`, which does not exist and which the netlister will accept and
//! ignore.
//!
//! # Which libraries
//!
//! [`registry::LIBRARIES`] is the table. `analogLib` and `basic` are parsed —
//! between them they cover every cell in the schematics here, sources and
//! passives from one, pins and supplies from the other. `rfLib`, `fBlockLib`
//! and `pcLib` are registered with their manual's location but no parser;
//! `ahdlLib` is registered as having no manual at all in IC23.1. Those four
//! answer with an explanation of what to do instead. **None of them answers
//! with an empty result set** — a tool reporting its own gap as a fact about
//! the library is the defect this whole module was built to stop repeating.
//!
//! # Usage
//!
//! ```ignore
//! use virtuoso_cli::libref::LibRefFinder;
//! use virtuoso_cli::skill_finder::SearchMode;
//!
//! let mut finder = LibRefFinder::new();
//! finder.load("/opt/Cadence/IC231")?;          // or the local cache root
//! let vsin = finder.get("vsin").unwrap();       // analogLib/vsin
//! let ipin = finder.get_in("basic", "ipin");    // basic/ipin
//! for hit in finder.search_params("amplitude", SearchMode::Fuzzy, 10) {
//!     println!("{} — {}", hit.param.name, hit.param.label);
//! }
//! ```
//!
//! # Remote cache
//!
//! Same shape as the SKILL Finder cache, and namespaced by Cadence release for
//! the same reason: basenames collide across installs, and a flat cache lets
//! one release silently overwrite another. Each library gets its own
//! subdirectory so `basicLib.html` and `analoglibref`'s chapter files cannot
//! collide either.
//!
//! - Cache path: `~/.cache/virtuoso_bridge/libref/<host>/<lib>/`
//! - Cached names: `libref/host/analoglib/IC231__appA.html`,
//!   `libref/host/basic/IC231__basicLib.html`, …
//!
//! Only releases reachable from the Cadence binaries actually on `PATH` are
//! collected. This box also has `IC618` and `ICADVM201` trees with their own
//! `analoglibref/`; mixing in documentation for a release nobody is running is
//! how you end up confidently reading the wrong manual.

#![allow(dead_code)]

mod analoglib;
mod basiclib;
mod html;
pub mod registry;
mod types;

// Re-exported as the module's public surface; the binaries use only a
// subset of it, the way `skill_finder` re-exports `parse_fnd_directory`.
#[allow(unused_imports)]
pub use analoglib::{
    parse_analoglib_directory, parse_analoglib_directory_reported, APPENDIX_BASENAME,
};
#[allow(unused_imports)]
pub use basiclib::{parse_basiclib_directory, parse_basiclib_directory_reported};
#[allow(unused_imports)]
pub use registry::{DocFlavor, LibraryDoc, LIBRARIES};
#[allow(unused_imports)]
pub use types::{CdfParam, LibSymbol, ParseReport};

use crate::skill_finder::{cadence_env_setup, SearchMode};
use std::path::{Path, PathBuf};

/// Root of the per-library cache directories, under the bridge cache.
const CACHE_ROOT: &str = "libref";

/// A parameter hit: which symbol, and which of its parameters matched.
#[derive(Debug, Clone, Copy)]
pub struct ParamHit<'a> {
    pub symbol: &'a LibSymbol,
    pub param: &'a CdfParam,
}

/// What happened to one library when the finder loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryState {
    /// Parsed, with symbols.
    Loaded,
    /// A parser exists but no documentation was found to feed it — the cache is
    /// empty or the release root has no such directory. **Not** "the library
    /// has no cells".
    DocsMissing,
    /// Registered, the manual is installed, nothing here reads its shape yet.
    NoParser(&'static str),
    /// The library exists in Virtuoso but the release ships no manual for it.
    NoManual(&'static str),
}

impl LibraryState {
    /// Short tag for JSON output.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::DocsMissing => "docs_missing",
            Self::NoParser(_) => "no_parser",
            Self::NoManual(_) => "no_manual",
        }
    }

    /// The sentence explaining a state that yields no symbols.
    pub fn note(&self) -> Option<&'static str> {
        match self {
            Self::Loaded => None,
            Self::DocsMissing => Some(
                "No documentation for this library was found in the cache or the \
                 local Cadence tree. This is a missing sync, not an empty library \
                 — re-run with refresh, or check VB_LIBREF_DIR.",
            ),
            Self::NoParser(n) | Self::NoManual(n) => Some(n),
        }
    }
}

/// Per-library outcome of a load, reported with every answer.
#[derive(Debug, Clone)]
pub struct LibraryStatus {
    pub lib: &'static str,
    pub state: LibraryState,
    pub symbols: usize,
    /// Directory the symbols came from, when there was one.
    pub source: Option<PathBuf>,
}

/// Queryable Virtuoso library reference, across every registered library.
#[derive(Debug, Default)]
pub struct LibRefFinder {
    source_dir: Option<PathBuf>,
    symbols: Vec<LibSymbol>,
    report: ParseReport,
    libraries: Vec<LibraryStatus>,
    loaded: bool,
}

impl LibRefFinder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a finder over the given symbols, for tests.
    #[doc(hidden)]
    pub fn for_test(symbols: Vec<LibSymbol>) -> Self {
        Self {
            source_dir: None,
            report: ParseReport {
                indexed: symbols.len(),
                parsed: symbols.len(),
                ..Default::default()
            },
            libraries: Vec::new(),
            symbols,
            loaded: true,
        }
    }

    /// Load every registered library found under `root`.
    ///
    /// `root` may be any of three shapes, because the three callers have three:
    /// the cache root (`…/libref/<host>/`, with one subdirectory per library),
    /// a Cadence release root (`/opt/Cadence/IC231`, with `doc/analoglibref`
    /// and `doc/basicLib` under it), or a single library's documentation
    /// directory loaded directly. Each library is looked for in all three
    /// places and takes the first that holds its index.
    pub fn load(&mut self, root: impl Into<PathBuf>) -> std::io::Result<()> {
        let root = root.into();
        if !root.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("library reference directory not found: {}", root.display()),
            ));
        }

        self.symbols.clear();
        self.report = ParseReport::default();
        self.libraries.clear();

        for doc in registry::LIBRARIES {
            let status = match doc.flavor {
                DocFlavor::NoManual { note } => LibraryStatus {
                    lib: doc.lib,
                    state: LibraryState::NoManual(note),
                    symbols: 0,
                    source: None,
                },
                DocFlavor::Unparsed { note } => LibraryStatus {
                    lib: doc.lib,
                    state: LibraryState::NoParser(note),
                    symbols: 0,
                    source: library_dir(&root, doc),
                },
                DocFlavor::AnalogLib | DocFlavor::BasicLib => {
                    let Some(dir) = library_dir(&root, doc) else {
                        self.libraries.push(LibraryStatus {
                            lib: doc.lib,
                            state: LibraryState::DocsMissing,
                            symbols: 0,
                            source: None,
                        });
                        continue;
                    };
                    let (mut symbols, report) = match doc.flavor {
                        DocFlavor::AnalogLib => parse_analoglib_directory_reported(&dir),
                        _ => parse_basiclib_directory_reported(&dir),
                    };
                    let count = symbols.len();
                    self.symbols.append(&mut symbols);
                    self.report.absorb(report);
                    LibraryStatus {
                        lib: doc.lib,
                        state: if count > 0 {
                            LibraryState::Loaded
                        } else {
                            LibraryState::DocsMissing
                        },
                        symbols: count,
                        source: Some(dir),
                    }
                }
            };
            self.libraries.push(status);
        }

        self.source_dir = Some(root);
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

    pub fn source_dir(&self) -> Option<&Path> {
        self.source_dir.as_deref()
    }

    /// What the last load could and could not parse.
    pub fn report(&self) -> &ParseReport {
        &self.report
    }

    /// Per-library outcome of the last load, in registry order.
    pub fn libraries(&self) -> &[LibraryStatus] {
        &self.libraries
    }

    /// The status of one library, by name.
    pub fn library(&self, lib: &str) -> Option<&LibraryStatus> {
        self.libraries
            .iter()
            .find(|s| s.lib.eq_ignore_ascii_case(lib))
    }

    pub fn symbols(&self) -> &[LibSymbol] {
        &self.symbols
    }

    /// Symbols of one library only.
    pub fn symbols_in(&self, lib: &str) -> impl Iterator<Item = &LibSymbol> {
        let lib = lib.to_string();
        self.symbols
            .iter()
            .filter(move |s| s.lib.eq_ignore_ascii_case(&lib))
    }

    /// Exact symbol lookup, case-insensitive, across every loaded library.
    ///
    /// Returns the first match in registry order. Use [`Self::get_all`] when
    /// the answer has to disambiguate: `vdd` is a cell in both `analogLib` and
    /// `basic`, and they are not the same symbol.
    pub fn get(&self, name: &str) -> Option<&LibSymbol> {
        self.symbols
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Exact lookup within one library.
    pub fn get_in(&self, lib: &str, name: &str) -> Option<&LibSymbol> {
        self.symbols
            .iter()
            .find(|s| s.lib.eq_ignore_ascii_case(lib) && s.name.eq_ignore_ascii_case(name))
    }

    /// Every library that documents a cell of this name.
    pub fn get_all(&self, name: &str) -> Vec<&LibSymbol> {
        self.symbols
            .iter()
            .filter(|s| s.name.eq_ignore_ascii_case(name))
            .collect()
    }

    /// Search symbol names (and titles/categories in fuzzy mode).
    pub fn search(&self, query: &str, mode: SearchMode, limit: usize) -> Vec<&LibSymbol> {
        let mut hits: Vec<&LibSymbol> = self
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
// Locating a library's files
// =============================================================================

/// Where one library's documentation sits under `root`, if it is there.
///
/// Tried in order: the cache layout (`root/<lib>/`), a Cadence release root
/// (`root/doc/<manual>/`), and `root` itself for a documentation directory
/// loaded directly. Presence is decided by the index file, with or without a
/// `<RELEASE>__` cache prefix — an empty directory is not a hit, because
/// "found the directory" and "found the manual" are different claims.
fn library_dir(root: &Path, doc: &LibraryDoc) -> Option<PathBuf> {
    if !doc.has_manual() {
        return None;
    }
    let candidates = [
        root.join(doc.cache_key()),
        root.join(doc.doc_subdir),
        root.to_path_buf(),
    ];
    candidates
        .into_iter()
        .find(|d| holds_index(d, doc.index_basename))
}

/// Whether `dir` holds `<index>` or `<RELEASE>__<index>`.
fn holds_index(dir: &Path, index_basename: &str) -> bool {
    if index_basename.is_empty() || !dir.is_dir() {
        return false;
    }
    std::fs::read_dir(dir)
        .map(|mut entries| {
            entries.any(|e| {
                e.ok()
                    .and_then(|e| e.file_name().to_str().map(|n| n.ends_with(index_basename)))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// =============================================================================
// Cache management
// =============================================================================

/// `~/.cache/virtuoso_bridge/libref/<host>/`
pub fn cache_dir(host: &str) -> Option<PathBuf> {
    Some(crate::runtime_paths::cache_subdir(&[CACHE_ROOT, host]))
}

/// `~/.cache/virtuoso_bridge/libref/<host>/<lib>/`
pub fn library_cache_dir(host: &str, doc: &LibraryDoc) -> Option<PathBuf> {
    Some(cache_dir(host)?.join(doc.cache_key()))
}

pub fn cache_exists(host: &str) -> bool {
    cache_dir(host)
        .map(|d| d.exists() && d.is_dir())
        .unwrap_or(false)
}

/// Count cached documentation files across every library's subdirectory.
pub fn cache_file_count(host: &str) -> usize {
    let Some(root) = cache_dir(host) else {
        return 0;
    };
    registry::LIBRARIES
        .iter()
        .map(|doc| count_doc_files(&root.join(doc.cache_key())))
        .sum()
}

fn count_doc_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| is_doc_file(&e.path()))
                .count()
        })
        .unwrap_or(0)
}

fn is_doc_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "html" || ext == "json")
}

/// `/opt/Cadence/IC231/doc/analoglibref` → `IC231`
///
/// The suffix is whichever registered manual the path ends with, so the tag is
/// the release either way — `…/IC231/doc/basicLib` must not come back as
/// `basicLib` and namespace the cache by manual instead of by install.
pub fn release_tag(doc_dir: &str) -> Option<&str> {
    let trimmed = doc_dir.trim_end_matches('/');
    registry::LIBRARIES
        .iter()
        .filter(|l| l.has_manual())
        .find_map(|l| trimmed.strip_suffix(&format!("/{}", l.doc_subdir)))?
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

/// Shell script printing every registered manual reachable from the Cadence
/// binaries on `PATH`, as `<lib><TAB><path>`, `virtuoso` tree first.
///
/// Same walk-up-from-the-binary probe as the SKILL Finder, and deliberately so:
/// the `which spectre || which virtuoso` short-circuit is what left that cache
/// holding 2 of 41 databases while every lookup answered "no such function".
/// Every library is probed for on every binary — one manual missing must not
/// stop the walk, for the same reason.
pub fn remote_doc_probe_script(cadence_cshrc: Option<&str>) -> String {
    let probes = registry::LIBRARIES
        .iter()
        .filter(|l| l.has_manual())
        .map(|l| format!("  walk_up \"$x\" '{}' '{}'", l.doc_subdir, l.lib))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"{setup}
walk_up() {{
  p="$1"
  sub="$2"
  lib="$3"
  while [ -n "$p" ] && [ "$p" != "/" ]; do
    if [ -d "$p/$sub" ]; then printf '%s\t%s\n' "$lib" "$p/$sub"; return 0; fi
    p=$(dirname "$p")
  done
  return 1
}}
for b in virtuoso spectre; do
  x=$(command -v "$b" 2>/dev/null) || continue
  [ -z "$x" ] && continue
{probes}
done"#,
        setup = cadence_env_setup(cadence_cshrc),
    )
}

/// Parse the probe output into `(library, directory)` pairs: trim, drop
/// non-paths, drop libraries that are not registered, de-duplicate in order.
pub fn parse_doc_dirs(stdout: &str) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line == "NOTFOUND" {
            continue;
        }
        let Some((lib, dir)) = line.split_once('\t') else {
            continue;
        };
        let dir = dir.trim();
        if !dir.starts_with('/') {
            continue;
        }
        // A name the registry does not know is a probe/table mismatch, not a
        // library — syncing it would fill the cache with unattributable files.
        let Some(doc) = registry::lookup(lib.trim()) else {
            continue;
        };
        if !out.iter().any(|(l, d)| *l == doc.lib && d == dir) {
            out.push((doc.lib, dir.to_string()));
        }
    }
    out
}

fn find_remote_doc_dirs(
    ssh_target: &str,
    cadence_cshrc: Option<&str>,
) -> std::io::Result<Vec<(&'static str, String)>> {
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
            "No Cadence library reference directory (doc/analoglibref, doc/basicLib, …) \
             found near virtuoso/spectre on the remote server. \
             Ensure Cadence is in PATH or set VB_CADENCE_CSHRC."
                .to_string(),
        ));
    }
    Ok(dirs)
}

/// Download every registered library reference from a remote host into the
/// local cache, each into its own subdirectory.
///
/// Only `*.html` and the library's `.json` index are fetched — about 2.4 MB for
/// analogLib, 100 KB for basic. The full directories are 16 MB and 1.7 MB, the
/// rest being images that nothing here reads.
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
            "Found {} remote reference dir(s): {}",
            remote_dirs.len(),
            remote_dirs
                .iter()
                .map(|(l, d)| format!("{l} -> {d}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let mut synced = 0usize;
    // Written files per library, so a library that failed to download does not
    // get its previous copies pruned by another library's success.
    let mut written: std::collections::BTreeMap<String, Vec<std::ffi::OsString>> =
        Default::default();
    let mut failed: std::collections::BTreeSet<String> = Default::default();

    for (lib, remote_dir) in &remote_dirs {
        let Some(doc) = registry::lookup(lib) else {
            continue;
        };
        // An unparsed library's manual is on the box; downloading it would fill
        // the cache with files nothing reads. Its status already says so.
        if !doc.is_parsed() {
            continue;
        }
        let lib_cache = cache.join(doc.cache_key());
        std::fs::create_dir_all(&lib_cache)?;
        let index = doc.index_basename;

        let list_script = format!(
            r#"find {remote_dir} -maxdepth 1 -type f \( -name '*.html' -o -name '{index}' \) 2>/dev/null | head -100"#
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
                "Failed to list remote files under {remote_dir}: {}",
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
                "Found {} files under {remote_dir} ({lib})",
                files.len()
            ));
        }

        for remote_file in &files {
            let base = Path::new(remote_file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown.html");
            let file_name = cache_file_name(remote_dir, base);
            let local_path = lib_cache.join(&file_name);

            let scp_result = Command::new("scp")
                .args(["-o", "BatchMode=yes"])
                .args(["-o", "ConnectTimeout=30"])
                .arg(format!("{ssh_target}:{remote_file}"))
                .arg(&local_path)
                .output();

            match scp_result {
                Ok(out) if out.status.success() => {
                    synced += 1;
                    written
                        .entry(doc.cache_key())
                        .or_default()
                        .push(std::ffi::OsString::from(&file_name));
                    if let Some(p) = progress {
                        p(&format!("Downloaded: {lib}/{file_name}"));
                    }
                }
                Ok(out) => {
                    failed.insert(doc.cache_key());
                    tracing::warn!(
                        "Failed to download {}: {}",
                        file_name,
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
                Err(e) => {
                    failed.insert(doc.cache_key());
                    tracing::warn!("SCP error for {}: {}", file_name, e);
                }
            }
        }
    }

    // A half-finished sync must not delete the copies we still have.
    for (key, names) in &written {
        if failed.contains(key) || names.is_empty() {
            continue;
        }
        prune_stale_cache(&cache.join(key), names, progress);
    }

    if let Some(p) = progress {
        p(&format!("Cache sync complete: {synced} files"));
    }
    Ok(synced)
}

fn prune_stale_cache<F>(cache: &Path, written: &[std::ffi::OsString], progress: Option<F>)
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
    finder: &mut LibRefFinder,
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

/// Information about a cached library reference.
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

    fn corpus() -> LibRefFinder {
        LibRefFinder::for_test(vec![
            LibSymbol {
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
            LibSymbol {
                name: "idc".into(),
                category: "Sources - Independent".into(),
                title: "Independent DC Current Source".into(),
                params: vec![param("ia", "Amplitude 1 (Apk)", ""), param("idc", "DC current", "")],
                release: "IC231".into(),
                source_file: "independent.html".into(),
                ..Default::default()
            },
            LibSymbol {
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
        symbols.push(LibSymbol {
            // Sorts before `vsin` alphabetically, and matches only in prose.
            name: "aaa_prose".into(),
            params: vec![param("zz", "Modulation", "Sets the amplitude modulation type")],
            ..Default::default()
        });
        let f = LibRefFinder::for_test(symbols);
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
        let f = LibRefFinder::for_test(vec![LibSymbol {
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
        let f = LibRefFinder::new();
        assert!(!f.is_loaded());
        assert!(f.get("vsin").is_none());
        assert_eq!(f.search_params("amplitude", SearchMode::Fuzzy, 10).len(), 0);
    }

    #[test]
    fn load_reports_a_missing_directory_instead_of_loading_nothing() {
        let mut f = LibRefFinder::new();
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

    /// Every manual in the registry has to be probed for. A library added to
    /// the table but not to the script is a library that reports `docs_missing`
    /// forever, which reads to a caller as "not documented".
    #[test]
    fn probe_script_asks_for_every_registered_manual() {
        let script = remote_doc_probe_script(None);
        for l in registry::LIBRARIES.iter().filter(|l| l.has_manual()) {
            assert!(script.contains(l.doc_subdir), "{} not probed", l.lib);
            assert!(script.contains(l.lib), "{} not labelled", l.lib);
        }
        // `NoManual` has no directory; probing for `""` would match `$p/` and
        // hand back the release root as this library's manual.
        assert!(!script.contains("'' 'ahdlLib'"));
    }

    #[test]
    fn probe_output_is_deduplicated_in_discovery_order() {
        let dirs = parse_doc_dirs(
            "analogLib\t/opt/Cadence/IC231/doc/analoglibref\n\
             \n  basic\t/opt/Cadence/IC231/doc/basicLib  \n\
             analogLib\t/opt/Cadence/SPECTRE231/doc/analoglibref\n\
             analogLib\t/opt/Cadence/IC231/doc/analoglibref\n\
             NOTFOUND\nsome noise\n",
        );
        assert_eq!(
            dirs,
            vec![
                ("analogLib", "/opt/Cadence/IC231/doc/analoglibref".to_string()),
                ("basic", "/opt/Cadence/IC231/doc/basicLib".to_string()),
                (
                    "analogLib",
                    "/opt/Cadence/SPECTRE231/doc/analoglibref".to_string()
                ),
            ]
        );
    }

    /// A probe line naming a library the table does not know is a mismatch
    /// between the script and the registry — caching it would leave files no
    /// parser claims.
    #[test]
    fn an_unregistered_library_in_the_probe_output_is_dropped() {
        assert!(parse_doc_dirs("pdkLib\t/opt/Cadence/IC231/doc/pdk\n").is_empty());
        assert!(parse_doc_dirs("analogLib\trelative/path\n").is_empty());
    }

    /// `release_tag` has to strip whichever manual the path names, or the
    /// cache gets namespaced by manual instead of by install.
    #[test]
    fn release_tag_strips_any_registered_manual_not_just_analoglib() {
        assert_eq!(release_tag("/opt/Cadence/IC231/doc/basicLib"), Some("IC231"));
        assert_eq!(release_tag("/opt/Cadence/IC231/doc/rflibrary"), Some("IC231"));
        assert_eq!(
            cache_file_name("/opt/Cadence/IC231/doc/basicLib", "basicLib.html"),
            "IC231__basicLib.html"
        );
    }

    #[test]
    fn only_documentation_files_count_as_cache_content() {
        assert!(is_doc_file(std::path::Path::new("IC231__appA.html")));
        assert!(is_doc_file(std::path::Path::new("IC231__analoglibref.json")));
        assert!(!is_doc_file(std::path::Path::new("IC231__skdfref.fnd")));
        assert!(!is_doc_file(std::path::Path::new("notes.txt")));
    }

    // =========================================================================
    // Multi-library loading
    // =========================================================================

    /// A cache root: one subdirectory per library, as `sync_from_remote` writes it.
    fn two_library_cache() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();

        let a = root.path().join("analoglib");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(
            a.join("IC231__analoglibref.json"),
            r##"{"core":{"data":[
              {"id":"c1","parent":"#","text":"Passive Components"},
              {"id":"s1","parent":"c1","text":"Symbol: cap","href":"passives.html#pgfId-100"}
            ]}}"##,
        )
        .unwrap();
        std::fs::write(
            a.join("IC231__passives.html"),
            r#"<a name="pgfId-100"></a><h2>Symbol: cap</h2>
               <p>Two Terminal Capacitor</p>
               <table><tr><td>CDF Parameter Label</td><td>CDF Parameter</td><td>spectre</td></tr>
                      <tr><td>Capacitance</td><td>c</td><td>c</td></tr></table>
               <a name="pgfId-999"></a><h2>Symbol: next</h2>"#,
        )
        .unwrap();

        let b = root.path().join("basic");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(
            b.join("IC231__basiclib.json"),
            r##"{"core":{"data":[
              {"id":"n0","text":"Virtuoso Basic Library Reference","parent":"#","href":"TOC.html"},
              {"id":"n1","text":"Basic Library Categories","parent":"n0","href":"basicLib.html#top"},
              {"id":"n2","text":"Pins","parent":"n1","href":"basicLib.html#cat-pins"},
              {"id":"n3","text":"ipin","parent":"n2","href":"basicLib.html#pgfId-1"}
            ]}}"##,
        )
        .unwrap();
        std::fs::write(
            b.join("IC231__basicLib.html"),
            r#"<h2><a id="cat-pins"></a>Pins</h2>
               <h3><a id="pgfId-1"></a>ipi<a id="ipin"></a>n</h3>
               <h4>Description</h4><p>An input pin.</p>"#,
        )
        .unwrap();

        root
    }

    #[test]
    fn a_cache_root_loads_every_library_that_has_a_parser() {
        let root = two_library_cache();
        let mut f = LibRefFinder::new();
        f.load(root.path()).unwrap();

        assert_eq!(f.get_in("analogLib", "cap").unwrap().params.len(), 1);
        assert_eq!(
            f.get_in("basic", "ipin").unwrap().description,
            "An input pin."
        );
        assert_eq!(f.symbols_in("basic").count(), 1);
        assert!(f.report().unresolved.is_empty(), "{:?}", f.report());
    }

    /// Every registered library gets a status line, including the four with no
    /// symbols. A library that is simply absent from the answer is how "we did
    /// not read it" gets read as "it has nothing".
    #[test]
    fn every_registered_library_is_accounted_for_after_a_load() {
        let root = two_library_cache();
        let mut f = LibRefFinder::new();
        f.load(root.path()).unwrap();

        assert_eq!(f.libraries().len(), registry::LIBRARIES.len());
        for status in f.libraries() {
            match status.state {
                LibraryState::Loaded => assert!(status.symbols > 0),
                _ => {
                    assert_eq!(status.symbols, 0);
                    assert!(
                        status.state.note().is_some(),
                        "{} yields no symbols and says nothing about why",
                        status.lib
                    );
                }
            }
        }
        assert_eq!(f.library("analogLib").unwrap().state, LibraryState::Loaded);
        assert_eq!(f.library("basic").unwrap().state, LibraryState::Loaded);
        assert!(matches!(
            f.library("rfLib").unwrap().state,
            LibraryState::NoParser(_)
        ));
        assert!(matches!(
            f.library("ahdlLib").unwrap().state,
            LibraryState::NoManual(_)
        ));
    }

    /// A parsed library with nothing on disk reports `docs_missing`, which says
    /// "the sync did not happen" — not "this library documents no cells".
    #[test]
    fn a_parsed_library_with_no_files_says_the_docs_are_missing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("analoglib")).unwrap();
        let mut f = LibRefFinder::new();
        f.load(root.path()).unwrap();

        let s = f.library("analogLib").unwrap();
        assert_eq!(s.state, LibraryState::DocsMissing);
        assert!(s.state.note().unwrap().contains("missing sync"));
        assert!(f.is_empty());
    }

    /// The other layout: a Cadence release root, with `doc/<manual>/` under it.
    #[test]
    fn a_release_root_is_a_valid_load_target_too() {
        let root = tempfile::tempdir().unwrap();
        let doc = root.path().join("doc/basicLib");
        std::fs::create_dir_all(&doc).unwrap();
        std::fs::write(
            doc.join("basiclib.json"),
            r##"{"core":{"data":[
              {"id":"n0","text":"R","parent":"#"},
              {"id":"n1","text":"C","parent":"n0"},
              {"id":"n2","text":"Pins","parent":"n1"},
              {"id":"n3","text":"opin","parent":"n2","href":"basicLib.html#pgfId-1"}
            ]}}"##,
        )
        .unwrap();
        std::fs::write(
            doc.join("basicLib.html"),
            r#"<h3><a id="pgfId-1"></a>opin</h3><h4>Description</h4><p>An output pin.</p>"#,
        )
        .unwrap();

        let mut f = LibRefFinder::new();
        f.load(root.path()).unwrap();
        assert_eq!(f.get_in("basic", "opin").unwrap().description, "An output pin.");
    }

    /// A directory that exists but holds no index is not this library's manual.
    #[test]
    fn an_empty_directory_is_not_mistaken_for_a_manual() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("analoglib")).unwrap();
        let doc = registry::lookup("analogLib").unwrap();
        assert!(library_dir(root.path(), doc).is_none());
        assert!(!holds_index(&root.path().join("analoglib"), "analoglibref.json"));
    }

    /// `vdd` is a cell in more than one library. `get` answers with one of
    /// them; only `get_all` can tell the caller they are different symbols.
    #[test]
    fn a_name_carried_by_two_libraries_is_disambiguated_by_get_all() {
        let f = LibRefFinder::for_test(vec![
            LibSymbol {
                lib: "analogLib".into(),
                name: "vdd".into(),
                ..Default::default()
            },
            LibSymbol {
                lib: "basic".into(),
                name: "vdd".into(),
                ..Default::default()
            },
        ]);
        assert_eq!(f.get_all("vdd").len(), 2);
        assert_eq!(f.get_in("basic", "vdd").unwrap().lib, "basic");
        assert!(f.get_in("rfLib", "vdd").is_none());
        assert_eq!(f.get("VDD").unwrap().lib, "analogLib", "registry order");
    }

    #[test]
    fn library_cache_dirs_are_per_library() {
        let doc = registry::lookup("basic").unwrap();
        let root = cache_dir("somehost").unwrap();
        let lib = library_cache_dir("somehost", doc).unwrap();
        assert_eq!(lib.parent().unwrap(), root);
        assert_eq!(lib.file_name().unwrap(), "basic");
        assert!(root.ends_with("libref/somehost"));
    }
}
