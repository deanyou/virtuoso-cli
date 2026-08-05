/// Read-only library database queries.
#[derive(Default)]
pub struct LibraryOps;

impl LibraryOps {
    pub fn list(&self) -> String {
        r#"mapcar(lambda((lib) lib~>name) ddGetLibList())"#.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn list_is_read_only_and_uses_dd_get_lib_list() {
        let s = LibraryOps::default().list();
        assert_eq!(s, "mapcar(lambda((lib) lib~>name) ddGetLibList())");
        assert!(!s.contains("ddDelete"));
    }
}
