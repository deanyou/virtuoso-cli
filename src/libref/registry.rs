//! Which Virtuoso libraries have a manual, where it lives, and who parses it.
//!
//! `libref` started as an analogLib-only lookup. It is not an analogLib-only
//! problem: every schematic on this project also places `basic/ipin`,
//! `basic/opin` and `basic/gnd`, and those are documented in a different
//! directory with a different HTML shape.
//!
//! # The rule this table exists to enforce
//!
//! A library that is not parsed must **say so**. The failure mode being
//! designed out is the one `skill.info` shipped with (defect L): the tool's own
//! missing data reported as a fact about the thing asked for — *"no such
//! function"* when the truth was *"39 of 41 databases never synced"*. So
//! `rfLib` answers "the manual is on the box, nothing here reads it yet" and
//! `ahdlLib` answers "IC23.1 ships no manual for this library at all", and
//! neither one is allowed to come back as an empty result set.
//!
//! # What is in the table, and what is not
//!
//! The table describes **documentation**, not installed libraries — it is
//! compiled in, and whether a library exists is a property of the Virtuoso
//! install, discovered at runtime by `ddGetLibList()`. On the reference box the
//! two do not agree in either direction: `pcLib` and `fBlockLib` ship manuals
//! but are not installed, and `ahdlLib` is installed with no manual. The probe
//! looks for whatever directories are actually there, so an entry whose
//! `doc_subdir` is absent simply reports as not found.

/// How a library's manual is laid out, and therefore which parser reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocFlavor {
    /// `doc/analoglibref/`: a jstree index of `Symbol: <name>` nodes spread
    /// over ~17 chapter files, each section carrying a CDF parameter table
    /// plus a `spectre -h <primitive>` hint.
    AnalogLib,
    /// `doc/basicLib/`: one HTML file, a jstree index of bare cell names, each
    /// section an `<h3>` with a prose *Description* and **no** parameter table.
    BasicLib,
    /// The manual is installed and this table knows where, but no parser here
    /// reads its shape. Queries answer with `note`, never with silence.
    Unparsed { note: &'static str },
    /// The library exists in Virtuoso but the release ships no manual for it.
    /// There is nothing to probe for, so `doc_subdir` is empty.
    NoManual { note: &'static str },
}

/// One library's documentation set.
#[derive(Debug, Clone, Copy)]
pub struct LibraryDoc {
    /// Virtuoso library name, spelled as `ddGetLibList()` reports it —
    /// `analogLib`, not `analoglib`. Lookups are case-insensitive; the display
    /// spelling matters because it is what goes into `<lib>/<cell>` paths.
    pub lib: &'static str,
    /// Directory holding the manual, relative to a Cadence release root.
    /// Empty for [`DocFlavor::NoManual`].
    pub doc_subdir: &'static str,
    /// Basename of the jstree index that drives the parse.
    pub index_basename: &'static str,
    pub flavor: DocFlavor,
}

impl LibraryDoc {
    /// Whether a parser here can turn this library's manual into symbols.
    pub fn is_parsed(&self) -> bool {
        matches!(self.flavor, DocFlavor::AnalogLib | DocFlavor::BasicLib)
    }

    /// Whether there is a directory worth probing the remote host for.
    pub fn has_manual(&self) -> bool {
        !self.doc_subdir.is_empty()
    }

    /// The one-line explanation for a library that yields no symbols.
    pub fn note(&self) -> Option<&'static str> {
        match self.flavor {
            DocFlavor::Unparsed { note } | DocFlavor::NoManual { note } => Some(note),
            _ => None,
        }
    }

    /// Subdirectory name for this library inside the local cache.
    ///
    /// Lower-cased so the cache path does not depend on the filesystem being
    /// case-sensitive — `analogLib` and `analoglib` must not become two caches.
    pub fn cache_key(&self) -> String {
        self.lib.to_ascii_lowercase()
    }
}

/// Every library `libref` knows about.
///
/// Ordered by how often a schematic here touches them: `analogLib` (sources,
/// passives), `basic` (pins, supplies), then the rest.
pub const LIBRARIES: &[LibraryDoc] = &[
    LibraryDoc {
        lib: "analogLib",
        doc_subdir: "doc/analoglibref",
        index_basename: "analoglibref.json",
        flavor: DocFlavor::AnalogLib,
    },
    LibraryDoc {
        lib: "basic",
        doc_subdir: "doc/basicLib",
        index_basename: "basiclib.json",
        flavor: DocFlavor::BasicLib,
    },
    LibraryDoc {
        lib: "rfLib",
        doc_subdir: "doc/rflibrary",
        index_basename: "rflibrary.json",
        flavor: DocFlavor::Unparsed {
            note: "The rfLib manual is installed (doc/rflibrary, one file per \
                   component: 113 `chap1_re_*.html`), but no parser here reads \
                   that shape yet — neither the analogLib chapter layout nor \
                   the single-file basicLib layout fits it. Until one is \
                   written, use schematic.list_cdf_params on a placed instance \
                   for parameter names, and read the HTML directly for prose.",
        },
    },
    LibraryDoc {
        lib: "fBlockLib",
        doc_subdir: "doc/fblocklibref",
        index_basename: "fblocklibref.json",
        flavor: DocFlavor::Unparsed {
            note: "The fBlockLib manual is installed (doc/fblocklibref) but no \
                   parser here reads it, and the library itself is not in \
                   ddGetLibList() on this install — check that the cell you are \
                   after really comes from fBlockLib before chasing its docs.",
        },
    },
    LibraryDoc {
        lib: "pcLib",
        doc_subdir: "doc/pclib",
        index_basename: "pclib.json",
        flavor: DocFlavor::Unparsed {
            note: "The pcLib manual is installed (doc/pclib) but no parser here \
                   reads it, and the library itself is not in ddGetLibList() on \
                   this install.",
        },
    },
    LibraryDoc {
        lib: "ahdlLib",
        doc_subdir: "",
        index_basename: "",
        flavor: DocFlavor::NoManual {
            note: "ahdlLib is installed in Virtuoso but IC23.1 ships no \
                   reference manual for it — there is no doc/ahdl* directory to \
                   read. Its cells are Verilog-A masters: place one and use \
                   schematic.list_cdf_params for the parameter names, or open \
                   the veriloga view to read the source.",
        },
    },
];

/// Look a library up by name, case-insensitively.
pub fn lookup(lib: &str) -> Option<&'static LibraryDoc> {
    LIBRARIES.iter().find(|l| l.lib.eq_ignore_ascii_case(lib))
}

/// The libraries a parser here can actually read.
pub fn parsed() -> impl Iterator<Item = &'static LibraryDoc> {
    LIBRARIES.iter().filter(|l| l.is_parsed())
}

/// Every registered library name, for error messages.
pub fn known_names() -> Vec<&'static str> {
    LIBRARIES.iter().map(|l| l.lib).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_insensitive_but_keeps_the_canonical_spelling() {
        assert_eq!(lookup("analoglib").unwrap().lib, "analogLib");
        assert_eq!(lookup("ANALOGLIB").unwrap().lib, "analogLib");
        assert_eq!(lookup("Basic").unwrap().lib, "basic");
        assert!(lookup("pdkLib").is_none());
    }

    #[test]
    fn the_two_libraries_every_schematic_uses_are_parsed() {
        assert!(lookup("analogLib").unwrap().is_parsed());
        assert!(lookup("basic").unwrap().is_parsed());
    }

    /// A library with no parser must carry a note saying what to do instead.
    /// Silence here is how "we did not read it" becomes "it does not exist".
    #[test]
    fn every_unparsed_library_explains_itself_and_offers_a_way_forward() {
        for l in LIBRARIES.iter().filter(|l| !l.is_parsed()) {
            let note = l.note().unwrap_or_else(|| panic!("{} has no note", l.lib));
            assert!(
                note.contains("list_cdf_params") || note.contains("ddGetLibList"),
                "{}'s note gives the caller nowhere to go: {note}",
                l.lib
            );
        }
    }

    #[test]
    fn a_parsed_library_never_carries_a_note() {
        for l in parsed() {
            assert!(l.note().is_none(), "{} is parsed but has a note", l.lib);
        }
    }

    /// `NoManual` means there is nothing on disk; probing for `""` would match
    /// the release root itself and sync the whole tree.
    #[test]
    fn only_libraries_with_a_manual_are_probed_for() {
        for l in LIBRARIES {
            let no_manual = matches!(l.flavor, DocFlavor::NoManual { .. });
            assert_eq!(
                l.doc_subdir.is_empty(),
                no_manual,
                "{}: doc_subdir and NoManual disagree",
                l.lib
            );
            assert_eq!(l.has_manual(), !no_manual);
        }
    }

    #[test]
    fn cache_keys_are_lowercase_and_unique() {
        let mut keys: Vec<String> = LIBRARIES.iter().map(|l| l.cache_key()).collect();
        keys.sort();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "two libraries share a cache directory");
        assert!(keys.iter().all(|k| k.chars().all(|c| !c.is_uppercase())));
    }

    #[test]
    fn every_manual_bearing_library_names_its_index() {
        for l in LIBRARIES.iter().filter(|l| l.has_manual()) {
            assert!(!l.index_basename.is_empty(), "{} has no index", l.lib);
            assert!(l.index_basename.ends_with(".json"), "{}", l.lib);
            assert!(l.doc_subdir.starts_with("doc/"), "{}", l.lib);
        }
    }
}
