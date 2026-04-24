#![allow(dead_code)]

use crate::client::bridge::escape_skill_string;

#[cfg(test)]
mod tests {
    use super::*;

    fn ops() -> LayoutOps {
        LayoutOps::new()
    }

    #[test]
    fn create_rect_skill_format() {
        let s = ops().create_rect("M1", "drawing", &[(0, 0), (100, 200)]);
        assert_eq!(
            s,
            r#"rodCreateRect(?layer "M1" ?purpose "drawing" ?bBox ((0 0) (100 200)))"#
        );
    }

    #[test]
    fn create_rect_escapes_layer() {
        let s = ops().create_rect(r#"M"1"#, "drawing", &[(0, 0), (1, 1)]);
        assert!(s.contains(r#""M\"1""#), "layer must be escaped: {s}");
    }

    #[test]
    fn create_polygon_skill_format() {
        let pts = vec![(0, 0), (10, 0), (10, 10)];
        let s = ops().create_polygon("poly", "drawing", &pts);
        assert!(s.contains("rodCreatePolygon"), "{s}");
        assert!(s.contains("0 0"), "{s}");
        assert!(s.contains("10 10"), "{s}");
    }

    #[test]
    fn create_path_includes_width() {
        let pts = vec![(0, 0), (50, 0)];
        let s = ops().create_path("M2", "drawing", 5, &pts);
        assert!(s.contains("?width 5"), "{s}");
    }

    #[test]
    fn create_instance_orientation() {
        let s = ops().create_instance("tsmc", "nmos", "layout", (10, 20), "MY");
        assert!(s.contains("\"MY\""), "orient must appear: {s}");
        assert!(
            s.contains("10:20") || s.contains("10 20"),
            "origin must appear: {s}"
        );
    }
}

#[derive(Default)]
pub struct LayoutOps;

impl LayoutOps {
    pub fn new() -> Self {
        Self
    }

    /// `bbox`: `[(ll_x, ll_y), (ur_x, ur_y)]` — lower-left and upper-right corners.
    /// Formats to SKILL `?bBox ((ll_x ll_y) (ur_x ur_y))`.
    pub fn create_rect(&self, layer: &str, purpose: &str, bbox: &[(i64, i64); 2]) -> String {
        let layer = escape_skill_string(layer);
        let purpose = escape_skill_string(purpose);
        let ((x1, y1), (x2, y2)) = (bbox[0], bbox[1]);
        format!(
            r#"rodCreateRect(?layer "{layer}" ?purpose "{purpose}" ?bBox (({x1} {y1}) ({x2} {y2})))"#
        )
    }

    pub fn create_polygon(&self, layer: &str, purpose: &str, points: &[(i64, i64)]) -> String {
        let layer = escape_skill_string(layer);
        let purpose = escape_skill_string(purpose);
        let pts: String = points
            .iter()
            .map(|(x, y)| format!("{x} {y}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!(r#"rodCreatePolygon(?layer "{layer}" ?purpose "{purpose}" ?points list({pts}))"#)
    }

    pub fn create_path(
        &self,
        layer: &str,
        purpose: &str,
        width: i64,
        points: &[(i64, i64)],
    ) -> String {
        let layer = escape_skill_string(layer);
        let purpose = escape_skill_string(purpose);
        let pts: String = points
            .iter()
            .map(|(x, y)| format!("{x} {y}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            r#"rodCreatePath(?layer "{layer}" ?purpose "{purpose}" ?width {width} ?points list({pts}))"#
        )
    }

    pub fn create_via(&self, via_def: &str, origin: (i64, i64)) -> String {
        let via_def = escape_skill_string(via_def);
        let (x, y) = origin;
        format!(r#"rodCreateVia(?viaHeader "{via_def}" ?origin {x}:{y})"#)
    }

    pub fn create_label(
        &self,
        layer: &str,
        _purpose: &str,
        text: &str,
        origin: (i64, i64),
    ) -> String {
        let layer = escape_skill_string(layer);
        let text = escape_skill_string(text);
        let (x, y) = origin;
        format!(
            r#"dbCreateLabel(dbGetCurCellView() dbGetLayerByName(dbGetCurCellView() "{layer}") {x}:{y} "{text}" "centerCenter" "R0" "stick" 0.0625)"#
        )
    }

    pub fn create_instance(
        &self,
        lib: &str,
        cell: &str,
        view: &str,
        origin: (i64, i64),
        orient: &str,
    ) -> String {
        let lib = escape_skill_string(lib);
        let cell = escape_skill_string(cell);
        let view = escape_skill_string(view);
        let orient = escape_skill_string(orient);
        let (x, y) = origin;
        format!(
            r#"dbCreateInst(dbOpenCellViewByType("{lib}" "{cell}" "{view}" nil "r") nil nil {x}:{y} "{orient}" 1)"#
        )
    }

    pub fn set_active_lpp(&self, layer: &str, purpose: &str) -> String {
        let layer = escape_skill_string(layer);
        let purpose = escape_skill_string(purpose);
        format!(r#"leSetEntryLayer(list("{layer}" "{purpose}"))"#)
    }

    pub fn fit_view(&self) -> String {
        r#"hiRedraw() hiZoomBox(hiGetCurrentWindow() geGetWindowBox(hiGetCurrentWindow()) geGetEditCellView()~>bBox)"#.into()
    }

    pub fn read_summary(&self) -> String {
        r#"let((cv) cv = geGetEditCellView() list(cv~>libName cv~>cellName cv~>viewName cv~>bBox length(cv~>instances) length(cv~>nets)))"#.into()
    }

    pub fn read_geometry(&self, layer: &str, purpose: &str) -> String {
        let layer = escape_skill_string(layer);
        let purpose = escape_skill_string(purpose);
        format!(
            r#"let((cv shapes) cv = geGetEditCellView() shapes = nil foreach(shape cv~>shapes when(shape~>lpp == list("{layer}" "{purpose}") shapes = cons(shape~>bBox shapes))) shapes)"#
        )
    }

    pub fn delete_shapes_on_layer(&self, layer: &str, purpose: &str) -> String {
        let layer = escape_skill_string(layer);
        let purpose = escape_skill_string(purpose);
        format!(
            r#"let((cv) cv = geGetEditCellView() foreach(shape cv~>shapes when(shape~>lpp == list("{layer}" "{purpose}") dbDeleteObject(shape))))"#
        )
    }

    pub fn highlight_net(&self, net_name: &str) -> String {
        let net_name = escape_skill_string(net_name);
        format!(
            r#"let((cv net) cv = geGetEditCellView() net = dbFindNetByName(cv "{net_name}") when(net hiHighlight(net)))"#
        )
    }
}
