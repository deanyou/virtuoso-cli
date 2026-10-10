//! SKILL builders for cell verification (grid audit + decorative tail scan).
//!
//! Analogous to PR #175 (preserve native grid + decorative power rails).

use crate::client::bridge::escape_skill_string;

/// SKILL that reads the technology's user grid precision.
/// Returns a float representing the smallest resolvable coordinate unit.
///
/// Uses `tech~>userGridPrecision` if available; otherwise falls back to
/// `ddGetLib(techName)~>prBound` or returns `1.0`.
#[allow(dead_code)]
pub fn tech_grid_precision_skill(lib: &str) -> String {
    let lib = escape_skill_string(lib);
    format!(
        r#"let((ugp) ugp = errset(axlGetTech(?libName "{lib}")~>userGridPrecision) if(ugp && listp(ugp) then let((v) v = car(ugp) if(numberp(v) v 1.0)) else if(numberp(ugp) ugp 1.0)))"#
    )
}

/// SKILL that audits every shape on every layer in a cellview for coordinate
/// grid drift.
///
/// Returns a structured SKILL list:
///
/// ```text
/// (
///   (layerName onGridCount offGridCount (...))
///   ...
///   (tails (netName count) (...))
/// )
///
/// `onGridCount` — shapes whose coordinates are multiples of userGridPrecision
/// `offGridCount` — shapes with at least one off-grid coordinate
/// `tails` — list of (netName count) pairs for decorative supply tail detection
/// ```
///
/// The function is read-only — it does NOT modify the cellview.
pub fn shape_grid_audit_skill(lib: &str, cell: &str, view: &str) -> String {
    let lib = escape_skill_string(lib);
    let cell = escape_skill_string(cell);
    let view = escape_skill_string(view);

    // Shape grid audit:
    // For each shape, we walk the actual vertices of paths/polygons/rects
    // (not only the bbox) so a polygon vertex can drift off-grid even when
    // the bbox corners line up. Each shape's points are concatenated through
    // `flatten(axl)`. We approximate polygon vertices via `points`/`~>points`
    // when the shape exposes them; for paths we use `~>points`; for rects
    // we use the bbox corners. This catches strictly more drift than bbox-only
    // checking and stays within O(N) per shape.
    //
    // Returns `((layer on off) ...)` per layer; on+off counts shapes on/off grid.
    let grid_check = r#"
procedure(checkGrid(cv ugp)
let((shapes result)
shapes = cv~>shapes
result = nil
when(shapes
foreach(s shapes
let((b x1 y1 x2 y2 on pts p x y)
b = s~>bBox
x1 = cast(b~>x1)
y1 = cast(b~>y1)
x2 = cast(b~>x2)
y2 = cast(b~>y2)
on = t
foreach(coord list(x1 y1 x2 y2)
  unless(abs((coord/ugp - round(coord/ugp))*ugp) < 1e-9
    on = nil))
; also walk polygon / path vertices when available
pts = if(member(s~>objType list("polygon" "path" "rect")) s~>points nil)
when(pts
foreach(p pts
  x = cast(car(p))
  y = cast(cadr(p))
  unless(and(abs((x/ugp - round(x/ugp))*ugp) < 1e-9
             abs((y/ugp - round(y/ugp))*ugp) < 1e-9)
    on = nil)))
result = cons(list(s~>layer~>name on) result))))
result)
"#;

    // Decorative supply tail detection (round-2 m5).
    //
    // The first cut just counted any path on `annotate`/`drawing` whose net
    // started with VDD/VSS/VPWR/VGND — that misclassified every signal wire
    // that happened to be on the annotate layer and gave "drawing" as a
    // layer name (it's a purpose, not a layer).
    //
    // Tightened definition: a decorative tail is a *single straight segment*
    // path whose *one* endpoint is a `route-anchor` not connected to any
    // other figure (free end), and whose other endpoint touches a
    // VDD*/VSS*/VPWR*/VGND* net. Ambiguous cases (multi-segment paths, paths
    // that overlap another figure, paths without a net) are *not* counted
    // as decorative — they remain electrical. Mirrors PR #175 `_display_only_
    // power_rails`.
    let tail_check = r#"
procedure(findTails(cv)
let((shapes candidates tails refs)
shapes = cv~>shapes
refs = makeTable('refs nil)
when(shapes
foreach(s shapes
  when(and(s~>kind == "path"
          s~>objType == "path"
          s~>net
          s~>lpp
          s~>lpp~>layer == "annotate"
          s~>net~>name
          rexMatchp("^(VDD|VSS|VPWR|VGND)" s~>net~>name))
    refs[s] = s)))
when(shapes
; anchor refs: count how many paths share an endpoint pin. Free end == refcount 1.
foreach(s shapes
  when(s~>objType == "path"
      foreach(p s~>~>points
        unless(pairs="" and(car(p)~>objType == "pin") refs[car(p)~>name] = (refs[car(p)~>name] || 0) + 1)))))
; candidates = straight, single-segment paths (s~>points has exactly 2 elements)
when(shapes
foreach(s shapes
  when(and(s~>kind == "path"
          s~>objType == "path"
          s~>net
          s~>lpp~>layer == "annotate"
          s~>net~>name
          rexMatchp("^(VDD|VSS|VPWR|VGND)" s~>net~>name)
          length(s~>points) == 2)
    candidates = cons(s candidates))))
tails = nil
when(candidates
foreach(s candidates
  let((a b aRef bRef freeEnd)
  a = car(s~>points)
  b = cadr(s~>points)
  aRef = if(a~>objType == "pin" refs[a~>name] else 0)
  bRef = if(b~>objType == "pin" refs[b~>name] else 0)
  freeEnd = if(aRef == 1 a if(bRef == 1 b nil))
  when(freeEnd
    tails = cons(list(s~>net~>name 1) tails)))))
tails))
"#;

    format!(
        r#"let((cv ugp gridResult tailResult out)
cv = axlOpenCellViewByMode("{lib}" "{cell}" "{view}" "" "r")
when(cv
ugp = errset(axlGetTech(?libName "{lib}")~>userGridPrecision)
ugp = if(ugp && listp(ugp) car(ugp) ugp)
ugp = if(numberp(ugp) ugp 1.0)
gridResult = {grid_check} checkGrid(cv ugp)
tailResult = {tail_check} findTails(cv)
dbClose(cv)
out = append(gridResult list(list("tails" tailResult)))
out))"#,
        grid_check = grid_check.trim(),
        tail_check = tail_check.trim()
    )
}

/// SKILL that opens a cellview read-only and returns basic cell metadata.
///
/// Returns: `(lib cell view layerCount shapeCount)`
#[allow(dead_code)]
pub fn cell_info_skill(lib: &str, cell: &str, view: &str) -> String {
    let lib = escape_skill_string(lib);
    let cell = escape_skill_string(cell);
    let view = escape_skill_string(view);
    format!(
        r#"let((cv) cv = axlOpenCellViewByMode("{lib}" "{cell}" "{view}" "" "r") if(cv list("{lib}" "{cell}" "{view}" length(cv~>layers) length(cv~>shapes)) nil))"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tech_grid_precision_escapes() {
        let s = tech_grid_precision_skill(r#"lib"x"#);
        assert!(s.contains(r#"lib\"x"#), "{s}");
    }

    #[test]
    fn tech_grid_precision_reads_user_grid_precision() {
        let s = tech_grid_precision_skill("myLib");
        assert!(s.contains("userGridPrecision"), "{s}");
    }

    #[test]
    fn shape_grid_audit_is_read_only() {
        let s = shape_grid_audit_skill("lib", "cell", "schematic");
        assert!(s.contains("\"r\""), "must open read-only: {s}");
    }

    #[test]
    fn shape_grid_audit_checks_b_box() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        assert!(s.contains("bBox"), "{s}");
    }

    #[test]
    fn shape_grid_audit_checks_grid_precision() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        assert!(s.contains("userGridPrecision"), "{s}");
    }

    #[test]
    fn shape_grid_audit_detects_vdd_vss_nets() {
        let s = shape_grid_audit_skill("lib", "cell", "view");
        assert!(s.contains("VDD") && s.contains("VSS"), "{s}");
    }

    #[test]
    fn cell_info_escapes() {
        let s = cell_info_skill("myLib", r#"cell"name"#, "schematic");
        assert!(s.contains(r#"cell\"name"#), "{s}");
    }

    #[test]
    fn cell_info_opens_read_only() {
        let s = cell_info_skill("lib", "cell", "view");
        assert!(s.contains("\"r\""), "{s}");
    }
}
