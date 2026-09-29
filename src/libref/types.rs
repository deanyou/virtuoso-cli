//! The records every library parser produces.
//!
//! Kept out of the per-library parsers so `analoglib.rs` and `basiclib.rs`
//! answer in the same shape: a caller asking *"what is `basic/ipin`?"* and one
//! asking *"what is `analogLib/vsin`?"* should not have to know which manual
//! the answer came from to read it.

use serde::{Deserialize, Serialize};

/// One CDF parameter of one symbol.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdfParam {
    /// The label shown in the ADE / schematic property form, e.g. `Amplitude 1 (Vpk)`.
    pub label: String,
    /// The CDF parameter name — what `schematic.set_param` takes, e.g. `va`.
    pub name: String,
    /// The corresponding Spectre netlist parameter, when the table gives one.
    pub spectre: String,
    /// Description, from `appA.html`. Empty when the appendix has no prose for it.
    pub description: String,
    /// Default value. `-` means "no default" in Cadence's tables.
    pub default: String,
    /// Concrete names, when `name` is a range like `F1 - F50`. Empty otherwise.
    ///
    /// The manual compresses fifty rows into one; the live instance does not.
    /// Cross-checking `vsin` against `schematic.list_cdf_params` showed exactly
    /// this: 37 documented rows vs 135 live names, the whole gap being two
    /// ranges. Without the expansion, `name` is a string no caller can pass to
    /// `set_param` — the tool's own notation presented as the parameter's name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expands_to: Vec<String>,
}

/// One library cell, as documented.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibSymbol {
    /// Virtuoso library the symbol lives in, e.g. `analogLib` or `basic`.
    ///
    /// Names collide across libraries — `basic` has `gnd`, analogLib has `gnd!`
    /// and `vdd` — so an answer that omits the library is an answer a caller
    /// cannot turn into a `<lib>/<cell>` to place.
    #[serde(default)]
    pub lib: String,
    /// Symbol name as it appears in the library, e.g. `vsin`.
    pub name: String,
    /// Chapter/category the symbol is filed under, e.g. `Sources - Independent`.
    pub category: String,
    /// Human title, e.g. `Independent Sinusoidal Voltage Source`.
    /// Empty for libraries whose headings are just the cell name.
    pub title: String,
    /// Prose describing what the cell is for.
    ///
    /// `analogLib` documents its symbols through the CDF table and leaves this
    /// empty; `basicLib` is the other way round — no parameter table anywhere,
    /// a *Description* paragraph for every one of its 41 cells. Neither is a
    /// parse failure, which is why both fields exist rather than one being
    /// forced to stand in for the other.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Spectre primitives named by the section's `spectre -h <x>` hints.
    /// `nmos4` lists three (`mos0`, `mos1`, `ekv`) and has no CDF table at all.
    pub primitives: Vec<String>,
    /// The symbol's CDF parameters.
    pub params: Vec<CdfParam>,
    /// Cadence release the entry came from, e.g. `IC231`. Empty if unprefixed.
    pub release: String,
    /// Chapter file the entry was parsed from, e.g. `independent.html`.
    pub source_file: String,
    /// Other chapters that document this same symbol. `ports.html` carries
    /// cross-reference stubs — *"This component is the same as psin described
    /// in Chapter 8"* — for the seven port sources, with no parameter table of
    /// their own. Those stubs are folded into the real entry rather than left
    /// to shadow it, and recorded here so the merge is visible.
    pub also_documented_in: Vec<String>,
}

impl LibSymbol {
    /// One-line summary for list output.
    pub fn summary(&self) -> String {
        format!(
            "{:<10} {:<14} {:<26} {} param(s)",
            self.lib,
            self.name,
            self.category,
            self.params.len()
        )
    }

    /// `analogLib/vsin` — what a caller passes to `schematic.place`.
    pub fn qualified_name(&self) -> String {
        if self.lib.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.lib, self.name)
        }
    }

    pub fn param(&self, name: &str) -> Option<&CdfParam> {
        self.params
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// Outcome of parsing one directory, including what could not be parsed.
#[derive(Debug, Clone, Default)]
pub struct ParseReport {
    /// Symbols named by the index.
    pub indexed: usize,
    /// Symbols whose section anchor was found and parsed.
    pub parsed: usize,
    /// Index entries whose anchor was missing — `"<name> (<file>#<anchor>)"`.
    /// Must stay empty; a non-empty list means the docs changed shape and the
    /// lookup is quietly incomplete.
    pub unresolved: Vec<String>,
    /// Rows dropped because they belonged to a non-parameter table.
    pub skipped_rows: usize,
}

impl ParseReport {
    /// Fold another report into this one.
    pub fn absorb(&mut self, other: ParseReport) {
        self.indexed += other.indexed;
        self.parsed += other.parsed;
        self.skipped_rows += other.skipped_rows;
        self.unresolved.extend(other.unresolved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_name_is_what_schematic_place_takes() {
        let s = LibSymbol {
            lib: "analogLib".into(),
            name: "vsin".into(),
            ..Default::default()
        };
        assert_eq!(s.qualified_name(), "analogLib/vsin");
    }

    /// A symbol from a hand-built fixture has no library; it must not come out
    /// as `/vsin`, which is a path to nothing.
    #[test]
    fn a_symbol_without_a_library_degrades_to_the_bare_name() {
        let s = LibSymbol {
            name: "vsin".into(),
            ..Default::default()
        };
        assert_eq!(s.qualified_name(), "vsin");
    }

    #[test]
    fn absorb_sums_the_counts_and_concatenates_the_failures() {
        let mut a = ParseReport {
            indexed: 156,
            parsed: 156,
            unresolved: vec!["x".into()],
            skipped_rows: 3,
        };
        a.absorb(ParseReport {
            indexed: 41,
            parsed: 41,
            unresolved: vec!["y".into()],
            skipped_rows: 2,
        });
        assert_eq!((a.indexed, a.parsed, a.skipped_rows), (197, 197, 5));
        assert_eq!(a.unresolved, vec!["x", "y"]);
    }

    #[test]
    fn param_lookup_is_case_insensitive() {
        let s = LibSymbol {
            params: vec![CdfParam {
                name: "va".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(s.param("VA").is_some());
        assert!(s.param("ampl").is_none());
    }
}
