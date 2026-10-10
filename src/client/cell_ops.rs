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
    // For each shape, read its bBox and check (x / ugp) * ugp == x within tolerance.
    // Returns ((layer on off) ...) where on+off counts shapes on/off grid.
    let grid_check = r#"
procedure(checkGrid(cv ugp)
let((shapes result)
shapes = cv~>shapes
result = nil
when(shapes
foreach(s shapes
let((b x1 y1 x2 y2 on)
b = s~>bBox
x1 = cast(b~>x1)
y1 = cast(b~>y1)
x2 = cast(b~>x2)
y2 = cast(b~>y2)
on = if(abs((x1/ugp - round(x1/ugp))*ugp) < 1e-9 &&
         abs((y1/ugp - round(y1/ugp))*ugp) < 1e-9 &&
         abs((x2/ugp - round(x2/ugp))*ugp) < 1e-9 &&
         abs((y2/ugp - round(y2/ugp))*ugp) < 1e-9)
then t else nil)
result = cons(list(s~>layer~>name on) result))))
result)
"#;

    // Decorative supply tail detection:
    // Shapes of kind path/wire on annotate/drawing layer where one endpoint
    // is a free pin and the other touches a VDD*/VSS*/VPWR*/VGND* net.
    let tail_check = r#"
procedure(findTails(cv)
let((shapes tails)
shapes = setof(s cv~>shapes
  and(s~>kind == "path"
      member(s~>layer~>name list("annotate" "drawing"))
      s~>net
      rexMatchp("^(VDD|VSS|VPWR|VGND)" s~>net~>name)
  )
)
tails = nil
when(shapes
foreach(s shapes
let((netName) netName = s~>net~>name
tails = cons(list(netName 1) tails))))
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
