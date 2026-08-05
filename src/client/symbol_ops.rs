use crate::client::bridge::escape_skill_string;

/// Builders for read-only symbol inspection and native schematic-to-symbol generation.
#[derive(Default)]
pub struct SymbolOps;

impl SymbolOps {
    pub fn new() -> Self {
        Self
    }

    pub fn inspect(&self, lib: &str, cell: &str, view: &str, view_type: &str) -> String {
        let (lib, cell, view, view_type) = (
            escape_skill_string(lib),
            escape_skill_string(cell),
            escape_skill_string(view),
            escape_skill_string(view_type),
        );
        format!(
            r#"let((cv out) out=nil cv=errset(dbOpenCellViewByType("{lib}" "{cell}" "{view}" "{view_type}" "r") nil) if(!cv list("notFound" "symbol view not found") then(progn(cv=car(cv) out=unwindProtect(progn(list("success" cv~>bBox mapcar(lambda((t) list(t~>name if(t~>direction t~>direction "inputOutput") if(t~>numBits t~>numBits 1))) cv~>terminals) schGetPinOrder(cv) cv~>portOrder cv~>termOrder)) dbClose(cv))) out)))"#
        )
    }

    pub fn generate(
        &self,
        lib: &str,
        cell: &str,
        schematic_view: &str,
        symbol_view: &str,
        sort_pins: Option<&str>,
    ) -> String {
        assert_ne!(
            schematic_view, symbol_view,
            "source and target views must differ"
        );
        if let Some(sort) = sort_pins {
            assert!(
                matches!(sort, "alphanumeric" | "geometric"),
                "invalid pin sort"
            );
        }
        let (lib, cell, src, dst) = (
            escape_skill_string(lib),
            escape_skill_string(cell),
            escape_skill_string(schematic_view),
            escape_skill_string(symbol_view),
        );
        let temp = format!("__vcli_symbol_{:x}", uuid::Uuid::new_v4().as_u128());
        let temp_e = escape_skill_string(&temp);
        let sort_expr = sort_pins.map(|s| format!("vbOldSort=schGetEnv(\"ssgSortPins\") vbChanged=schSetEnv(\"ssgSortPins\" \"{}\") ", escape_skill_string(s))).unwrap_or_default();
        format!(
            r#"let((src pinList gen tmp target expected actual old changed) src=nil tmp=nil target=nil old=nil changed=nil {sort_expr} unwindProtect(progn(src=dbOpenCellViewByType("{lib}" "{cell}" "{src}" "schematic" "r") unless(src list("notFound" "source schematic not found")) when(ddGetObj("{lib}" "{cell}" "{dst}") list("conflict" "target symbol exists")) pinList=schSchemToPinList("{lib}" "{cell}" "{src}") unless(pinList list("failed" "schematic to pin list failed")) gen=schPinListToSymbol("{lib}" "{cell}" "{temp_e}" pinList) unless(gen list("failed" "symbol generation failed")) tmp=dbOpenCellViewByType("{lib}" "{cell}" "{temp_e}" "schematicSymbol" "r") unless(tmp list("failed" "temporary symbol open failed")) expected=schGetPinOrder(src) actual=schGetPinOrder(tmp) unless(equal(expected actual) list("failed" "generated symbol pin order mismatch")) target=dbCopyCellView(tmp "{lib}" "{cell}" "{dst}" nil nil nil) unless(target list("conflict" "target symbol appeared during generation")) list("success" "{lib}" "{cell}" "{src}" "{dst}" actual)) progn(when(changed schSetEnv("ssgSortPins" old)) when(src dbClose(src)) when(tmp dbClose(tmp)) when(target dbClose(target)) when(ddGetObj("{lib}" "{cell}" "{temp_e}") ddDeleteObj(ddGetObj("{lib}" "{cell}" "{temp_e}")))))"#
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspect_quotes_inputs() {
        let s = SymbolOps::new().inspect("a\"b", "c", "symbol", "schematicSymbol");
        assert!(s.contains("a\\\"b"));
    }
    #[test]
    fn generate_uses_native_pipeline_and_no_overwrite() {
        let s = SymbolOps::new().generate("L", "C", "schematic", "symbol", None);
        assert!(s.contains("schSchemToPinList"));
        assert!(s.contains("schPinListToSymbol"));
        assert!(s.contains("dbCopyCellView"));
        assert!(s.contains("target symbol exists"));
    }
    #[test]
    fn generate_rejects_same_view() {
        let r = std::panic::catch_unwind(|| {
            SymbolOps::new().generate("L", "C", "symbol", "symbol", None)
        });
        assert!(r.is_err());
    }
}
