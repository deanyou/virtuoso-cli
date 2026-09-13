use crate::client::bridge::escape_skill_string;

/// Read-only library database queries.
#[derive(Default)]
pub struct LibraryOps;

impl LibraryOps {
    pub fn list(&self) -> String {
        r#"mapcar(lambda((lib) lib~>name) ddGetLibList())"#.into()
    }

    /// Every cell in `lib` with its view names, as `(("cell" ("view" …)) …)`.
    ///
    /// `lib~>cells` / `cell~>views` / `~>name` are the traversal Cadence's own
    /// documentation uses (`doc/virtuosoKPNS/pcellKPNS.html`, `pcRecompileLibPcell`),
    /// so this walks the same path the PDK tooling does rather than shelling out
    /// to `ls` — the on-disk directory listing and the `data.dm` index are not
    /// the same thing.
    ///
    /// `pattern` is a SKILL regular expression matched against the cell name; a
    /// plain word therefore behaves as a substring filter. A library with no
    /// cells is empty, not an error — but a library that does not exist is.
    pub fn list_cells(&self, lib: &str, pattern: Option<&str>) -> String {
        let lib = escape_skill_string(lib);
        let filter = match pattern {
            Some(p) => {
                let p = escape_skill_string(p);
                format!(r#"cells = setof(c cells rexMatchp("{p}" c~>name)) "#)
            }
            None => String::new(),
        };
        format!(
            r#"let((lib cells out) lib = ddGetObj("{lib}") when(!lib error("library.list_cells: no such library '{lib}' — check 'library.list'")) cells = lib~>cells {filter}out = mapcar(lambda((c) list(c~>name sort(mapcar(lambda((v) v~>name) c~>views) nil))) cells) sort(out lambda((a b) alphalessp(car(a) car(b)))))"#
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn list_is_read_only_and_uses_dd_get_lib_list() {
        let s = LibraryOps.list();
        assert_eq!(s, "mapcar(lambda((lib) lib~>name) ddGetLibList())");
        assert!(!s.contains("ddDelete"));
    }

    #[test]
    fn list_cells_walks_the_documented_dd_attributes() {
        let s = LibraryOps.list_cells("DESIGN_LIB", None);
        assert!(s.contains(r#"ddGetObj("DESIGN_LIB")"#), "{s}");
        assert!(s.contains("lib~>cells"), "{s}");
        assert!(s.contains("c~>views"), "{s}");
        assert!(!s.contains("rexMatchp"), "no filter unless asked: {s}");
    }

    /// Listing is a read path. It must never reach for a delete or an edit —
    /// the delete wheels are a separate, confirmation-gated story.
    #[test]
    fn list_cells_is_read_only() {
        let s = LibraryOps.list_cells("DESIGN_LIB", Some("ota"));
        for forbidden in ["ddDelete", "dbOpenCellViewByType", "dbSave", "schDelete"] {
            assert!(!s.contains(forbidden), "{forbidden} has no business here: {s}");
        }
    }

    #[test]
    fn list_cells_filters_by_pattern() {
        let s = LibraryOps.list_cells("SIM_LIB", Some("amp"));
        assert!(s.contains(r#"setof(c cells rexMatchp("amp" c~>name))"#), "{s}");
    }

    #[test]
    fn list_cells_escapes_its_arguments() {
        let s = LibraryOps.list_cells("bad\"lib", Some("bad\"pat"));
        assert!(!s.contains("bad\"lib"), "unescaped lib name: {s}");
        assert!(!s.contains("bad\"pat"), "unescaped pattern: {s}");
    }
}
