//! SKILL builders for cell verification (grid audit + decorative supply-rail
//! tail detection).
//!
//! Every API used here was verified against a live Virtuoso bridge. The
//! measured facts are recorded here so the next reader does not have to
//! re-derive them:
//!
//! * `dbOpenCellViewByType(lib cell view viewType mode)` — the `viewType`
//!   argument is REQUIRED, and passing it is what makes the call return a
//!   *cellview*. `dbOpenCellViewByType(lib cell list(view) mode)` returns a
//!   *window id* instead, and every `~>` accessor on a window id reads nil, so
//!   the audit silently reports "no drift" for every cellview.
//!   `axlOpenCellViewByMode` does not exist at all.
//! * The grid is `techGetMfgGridResolution(techGetTechFile(ddGetObj(lib)))`.
//!   There is no `axlGetTech` and no `userGridPrecision` — not in this IC
//!   version and not in the OA function database either.
//! * A shape's layer is `car(s~>lpp)`; `s~>lpp` is the `(layer purpose)` pair.
//!   There is no `s~>layer~>name`.
//! * A shape's bounding box is `s~>bBox` = `((x1 y1) (x2 y2))`. A box has no
//!   `x1`/`x2`/`y1`/`y2` accessors — the coordinates come out of the two
//!   points with `caar`/`cadar`/`caadr`/`cadadr`.
//! * `s~>points` is a list of `(x y)` coordinate PAIRS, and only shapes that
//!   carry a vertex list have one (a polygon does, a rectangle does not).
//!   There is no `s~>kind` (it is nil on real shapes) and no `s~>~>points`.
//! * `s~>net~>name` works on layout shapes — but a shape may have no net, so
//!   it must be guarded with `when(s~>net ...)`.
//! * `dbGetOverlaps(cv list(pt pt))` returns the shapes that touch a point.
//!   This is the repo's existing idiom for "what is connected here".
//!
//! SKILL gotchas that shaped the code below (all measured, not guessed):
//!
//! * `foreach` swallows every remaining form in its body — a `foreach` body
//!   must be wrapped in `progn(...)` when more than one statement has to run
//!   per element.
//! * `if` evaluates its condition as a *form*, so `if(s~>points ...)` fails
//!   to even parse. `when`/`unless` accept accessor conditions.
//! * Arithmetic is infix-only: `(/ a b)` is a syntax error, `a / b` is not.
//! * `rexMatchp` has no alternation — `^(VDD|VSS)` never matches. The supply
//!   net test below compares prefixes with `strcmp` instead.

use crate::client::bridge::escape_skill_string;

/// Key the grid-audit SKILL emits for its decorative-tail section.
///
/// The parser in `crate::commands::cell` looks for exactly this key, so the
/// two ends of the format cannot drift apart silently.
pub const TAILS_KEY: &str = "tails";

/// Map a view name to the OA `viewType` string `dbOpenCellViewByType` needs.
///
/// The names are Cadence-standard and version/PDK-independent: a layout view
/// is `"maskLayout"`, a symbol view is `"schematicSymbol"`. An unknown view
/// falls back to the view name itself, matching the `cell.open` convention.
fn view_type_for(view: &str) -> &str {
    match view {
        "layout" => "maskLayout",
        "symbol" => "schematicSymbol",
        other => other,
    }
}

/// SKILL that reads the technology's manufacturing grid resolution.
///
/// Returns the grid step in user units (0.005 on the `st02` test library), or
/// nil when the technology reports no resolution — there is no "user grid
/// precision" concept in the OpenAccess API this repo talks to.
#[allow(dead_code)]
pub fn tech_grid_precision_skill(lib: &str) -> String {
    let lib = escape_skill_string(lib);
    format!(
        r#"let((techId grid) techId = techGetTechFile(ddGetObj("{lib}")) grid = nil when(techId grid = techGetMfgGridResolution(techId)) grid)"#
    )
}

/// SKILL that audits every shape of every layer in a cellview for coordinate
/// grid drift, and reports decorative supply-rail tails.
///
/// Opens the cellview read-only, resolves the manufacturing grid, then walks
/// every shape. Each shape contributes to exactly one layer bucket:
///
/// * `on` — every coordinate the shape exposes is a multiple of the grid;
/// * `off` — at least one coordinate is off-grid.
///
/// The coordinates checked are the shape's bounding-box corners
/// (`s~>bBox` = `((x1 y1) (x2 y2))`) *and*, when the shape carries a vertex
/// list (`s~>points` — polygons do, rectangles do not), every vertex. A
/// polygon can drift on a vertex while its bounding box stays on-grid, so
/// checking the box alone under-reports drift.
///
/// Returns a list shaped like
///
/// ```text
/// ((layerName onCount offCount) ... (tails ((netName count) ...)))
/// ```
///
/// which is exactly what `crate::commands::cell::parse_grid_audit_output`
/// reads. The counts are per *layer* — the previous version of this builder
/// emitted one tuple per *shape*, which made the off-grid count
/// unreachable and the drift verdict impossible.
///
/// ## Decorative supply tails
///
/// A *decorative supply tail* is a shape that
///
/// * belongs to a supply net — its net name starts with `VDD`, `VSS`, `VPWR`
///   or `VGND` (compared with `strcmp`, because `rexMatchp` cannot do
///   alternation), and
/// * has a **free end** — a terminal point of the shape (its `~>points`
///   vertices, or the corners of its bounding box) where `dbGetOverlaps`
///   finds no other shape. A piece of rail that stops in mid-air is
///   decorative; one that reaches a device pin is electrical.
///
/// The original PR #175 rule was stricter: a *single straight segment* on the
/// `annotate` layer, anchored on a `route-anchor`. Neither half of that is
/// checkable with the accessors that exist — `s~>kind` is nil on real layout
/// shapes (their `objType` is only `"rect"` or `"polygon"`), a layout shape
/// exposes no route-anchor or pin dbobject to compare against, and the
/// `annotate` layer does not exist in every technology. This implementation
/// therefore reports broader set — every supply-net shape with a free end
/// — and the counts are an upper bound on the true tail count.
///
/// ## What was verified against a live bridge, and what was not
///
/// The grid audit was run end-to-end against a live Virtuoso
/// (`st02/qvp20/layout`, 497 shapes, grid 0.005) and returned
/// `(("TO" 3 0) ("SP" 2 0) ("A1" 3 0) ("BJT" 1 0) ("W1" 487 0) ("TB" 1 0)
/// ("tails" nil))` — per-layer counts in the format the parser reads.
///
/// The tails branch could **not** be run end-to-end there: no layout
/// cellview reachable from that host carries a `VDD*`/`VSS*`/`VPWR*`/`VGND*`
/// net, so no shape can take the supply-net branch. Its two halves were
/// each verified against the live tool instead — the prefix test
/// (`substring` + `strcmp`) and the free-end test (`dbGetOverlaps` over a
/// degenerate bbox, excluding the shape itself). The end-to-end path over a
/// real supply net remains unexercised.
///
/// The function is read-only: it opens the cellview with mode `"r"` and
/// closes it again. The close is the cleanup form of an `unwindProtect`, so
/// it runs even when the audit raises.
pub fn shape_grid_audit_skill(lib: &str, cell: &str, view: &str) -> String {
    let view_type = escape_skill_string(view_type_for(view));
    let lib = escape_skill_string(lib);
    let cell = escape_skill_string(cell);
    let view = escape_skill_string(view);

    format!(
        r#"let((cv techId gridRes pg
     layerEntries tailEntries
     shp lyr on off ent newEntries entName found
     bbox x1 y1 x2 y2 pts p0 pt vx vy
     netName supply pre3 pre4 cmp
     terms others free res)
cv = dbOpenCellViewByType("{lib}" "{cell}" "{view}" "{view_type}" "r")
unless(cv error("cell.verify-native: cannot open {lib}/{cell}/{view}"))
techId = techGetTechFile(ddGetObj("{lib}"))
gridRes = nil
when(techId gridRes = techGetMfgGridResolution(techId))
unless(gridRes
  pg = dbGetPlacementGrid(cv)
  when(pg gridRes = cadr(car(pg))))
unless(gridRes gridRes = 1.0)
layerEntries = nil
tailEntries = nil
res = unwindProtect(progn(
  foreach(shp cv~>shapes
    progn(
      lyr = nil
      when(shp~>lpp lyr = car(shp~>lpp))
      unless(lyr lyr = "unknown")
      on = 1
      bbox = shp~>bBox
      when(bbox
        x1 = caar(bbox)
        y1 = cadar(bbox)
        x2 = caadr(bbox)
        y2 = cadadr(bbox)
        foreach(pt list(list(x1 y1) list(x2 y2) list(x2 y1) list(x1 y2))
          progn(
            vx = car(pt)
            vy = cadr(pt)
            unless(abs(vx / gridRes - round(vx / gridRes)) < 0.001 on = 0)
            unless(abs(vy / gridRes - round(vy / gridRes)) < 0.001 on = 0))))
      pts = shp~>points
      when(pts
        p0 = car(pts)
        when(listp(p0)
          foreach(pt pts
            progn(
              vx = car(pt)
              vy = cadr(pt)
              unless(abs(vx / gridRes - round(vx / gridRes)) < 0.001 on = 0)
              unless(abs(vy / gridRes - round(vy / gridRes)) < 0.001 on = 0)))))
      off = 1
      when(on == 1 off = 0)
      newEntries = nil
      found = nil
      foreach(ent layerEntries
        progn(
          entName = car(ent)
          when(entName == lyr
            found = t
            newEntries = cons(list(lyr (cadr(ent) + on) (caddr(ent) + off)) newEntries))
          unless(entName == lyr
            newEntries = cons(ent newEntries))))
      unless(found
        newEntries = cons(list(lyr on off) newEntries))
      layerEntries = newEntries
      netName = nil
      supply = nil
      when(shp~>net
        progn(
          netName = shp~>net~>name
          when(netName
            progn(
              pre3 = substring(netName 1 3)
              pre4 = substring(netName 1 4)
              cmp = strcmp(pre3 "VDD")
              when(cmp == 0 supply = 1)
              cmp = strcmp(pre3 "VSS")
              when(cmp == 0 supply = 1)
              cmp = strcmp(pre4 "VPWR")
              when(cmp == 0 supply = 1)
              cmp = strcmp(pre4 "VGND")
              when(cmp == 0 supply = 1)))))
      when(supply
        progn(
          terms = nil
          pts = shp~>points
          when(pts
            p0 = car(pts)
            when(listp(p0) terms = pts))
          unless(terms
            bbox = shp~>bBox
            when(bbox
              x1 = caar(bbox)
              y1 = cadar(bbox)
              x2 = caadr(bbox)
              y2 = cadadr(bbox)
              terms = list(list(x1 y1) list(x2 y2) list(x2 y1) list(x1 y2))))
          free = nil
          foreach(pt terms
            progn(
              others = nil
              foreach(ent dbGetOverlaps(cv list(pt pt))
                unless(ent == shp others = cons(ent others)))
              unless(others free = 1)))
          when(free
            newEntries = nil
            found = nil
            foreach(ent tailEntries
              progn(
                entName = car(ent)
                when(entName == netName
                  found = t
                  newEntries = cons(list(netName (cadr(ent) + 1)) newEntries))
                unless(entName == netName
                  newEntries = cons(ent newEntries))))
            unless(found
              newEntries = cons(list(netName 1) newEntries))
            tailEntries = newEntries)))))
  append(layerEntries list(list("{TAILS_KEY}" tailEntries))))
 dbClose(cv))
res)"#
    )
}

/// SKILL that opens a cellview read-only and returns basic cell metadata.
///
/// Returns: `(lib cell view shapeCount)`, or nil when the cellview does not
/// exist. The cellview is closed again before the value is returned.
#[allow(dead_code)]
pub fn cell_info_skill(lib: &str, cell: &str, view: &str) -> String {
    let view_type = escape_skill_string(view_type_for(view));
    let lib = escape_skill_string(lib);
    let cell = escape_skill_string(cell);
    let view = escape_skill_string(view);
    format!(
        r#"let((cv info) cv = dbOpenCellViewByType("{lib}" "{cell}" "{view}" "{view_type}" "r") when(cv info = list("{lib}" "{cell}" "{view}" length(cv~>shapes)) dbClose(cv)) info)"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Count the parentheses of a SKILL string, ignoring the ones inside
    /// double-quoted SKILL strings.
    fn paren_balance(skill: &str) -> i32 {
        let mut depth = 0i32;
        let mut in_string = false;
        let mut chars = skill.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if in_string => {
                    chars.next();
                }
                '"' => in_string = !in_string,
                '(' if !in_string => depth += 1,
                ')' if !in_string => depth -= 1,
                _ => {}
            }
        }
        depth
    }

    #[test]
    fn tech_grid_precision_escapes() {
        let s = tech_grid_precision_skill(r#"lib"x"#);
        assert!(s.contains(r#"lib\"x"#), "{s}");
    }

    #[test]
    fn tech_grid_precision_reads_the_mfg_grid() {
        let s = tech_grid_precision_skill("myLib");
        assert!(s.contains("techGetMfgGridResolution"), "{s}");
        assert!(s.contains("techGetTechFile(ddGetObj("), "{s}");
        assert!(!s.contains("userGridPrecision"), "{s}");
        assert!(!s.contains("axlGetTech"), "{s}");
    }

    #[test]
    fn shape_grid_audit_is_read_only() {
        let s = shape_grid_audit_skill("lib", "cell", "layout");
        assert!(s.contains(r#""r""#), "must open read-only: {s}");
    }

    #[test]
    fn shape_grid_audit_opens_the_cellview_with_a_view_type() {
        // The 4-argument form returns a window id, not a cellview, and every
        // accessor on it reads nil.
        let s = shape_grid_audit_skill("st02", "qvp20", "layout");
        assert!(
            s.contains(r#"dbOpenCellViewByType("st02" "qvp20" "layout" "maskLayout" "r")"#),
            "{s}"
        );
        assert!(!s.contains("axlOpenCellViewByMode"), "{s}");
    }

    #[test]
    fn shape_grid_audit_checks_b_box_corners_and_vertices() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        assert!(s.contains("bBox"), "{s}");
        assert!(s.contains("caar(bbox)"), "{s}");
        assert!(s.contains("caadr(bbox)"), "{s}");
        assert!(s.contains("shp~>points"), "{s}");
        assert!(!s.contains("~>x1"), "{s}");
        assert!(!s.contains("~>kind"), "{s}");
        assert!(!s.contains("~>~>points"), "{s}");
    }

    #[test]
    fn shape_grid_audit_checks_grid_precision() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        assert!(s.contains("techGetMfgGridResolution"), "{s}");
        assert!(!s.contains("userGridPrecision"), "{s}");
    }

    #[test]
    fn shape_grid_audit_aggregates_per_layer() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        // One (layer on off) entry per layer, not one per shape.
        assert!(s.contains("cons(list(lyr on off) newEntries)"), "{s}");
        assert!(s.contains("(cadr(ent) + on)"), "{s}");
        assert!(s.contains("(caddr(ent) + off)"), "{s}");
    }

    #[test]
    fn shape_grid_audit_emits_the_tails_marker() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        assert!(
            s.contains(&format!(r#"list("{TAILS_KEY}" tailEntries)"#)),
            "{s}"
        );
    }

    #[test]
    fn shape_grid_audit_detects_vdd_vss_nets() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        for prefix in ["VDD", "VSS", "VPWR", "VGND"] {
            assert!(s.contains(prefix), "must check {prefix}: {s}");
        }
    }

    #[test]
    fn shape_grid_audit_finds_free_ends_with_real_accessors() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        assert!(s.contains("shp~>net~>name"), "{s}");
        assert!(s.contains("dbGetOverlaps(cv list(pt pt))"), "{s}");
        // The old endpoint analysis used accessors that can never evaluate.
        assert!(!s.contains("s~>kind"), "{s}");
        assert!(!s.contains("~>objType == \"pin\""), "{s}");
        assert!(!s.contains("~>layer~>name"), "{s}");
    }

    #[test]
    fn shape_grid_audit_closes_the_cellview_on_every_path() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        assert!(s.contains("dbClose(cv)"), "must close cellview: {s}");
        // The close is the cleanup form of unwindProtect, so it also runs
        // when the audit raises.
        assert!(s.contains("unwindProtect("), "{s}");
    }

    #[test]
    fn shape_grid_audit_parentheses_are_balanced() {
        let s = shape_grid_audit_skill("st02", "qvp20", "layout");
        assert_eq!(paren_balance(&s), 0, "unbalanced SKILL parens: {s}");
    }

    #[test]
    fn cell_info_escapes() {
        let s = cell_info_skill("lib", r#"cell"name"#, "schematic");
        assert!(s.contains(r#"cell\"name"#), "{s}");
    }

    #[test]
    fn cell_info_opens_read_only() {
        let s = cell_info_skill("lib", "cell", "view");
        assert!(s.contains(r#""r""#), "{s}");
        assert!(s.contains("dbClose(cv)"), "must close cellview: {s}");
    }

    #[test]
    fn tmp_dump_skills() {
        std::fs::write(
            "/tmp/skill_audit.il",
            shape_grid_audit_skill("st02", "qvp20", "layout"),
        )
        .unwrap();
        std::fs::write(
            "/tmp/skill_info.il",
            cell_info_skill("st02", "qvp20", "layout"),
        )
        .unwrap();
        std::fs::write("/tmp/skill_grid.il", tech_grid_precision_skill("st02")).unwrap();
    }
}
