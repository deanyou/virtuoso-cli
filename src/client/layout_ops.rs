use crate::client::bridge::escape_skill_string;

#[derive(Default)]
pub struct LayoutOps;

impl LayoutOps {
    pub fn new() -> Self {
        Self
    }

    pub fn create_rect(&self, layer: &str, purpose: &str, bbox: &[(i64, i64); 4]) -> String {
        let layer = escape_skill_string(layer);
        let purpose = escape_skill_string(purpose);
        format!(r#"rodCreateRect(?layer "{layer}" ?purpose "{purpose}" ?bBox {bbox:?})"#)
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
