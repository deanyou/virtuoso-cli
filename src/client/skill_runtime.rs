pub(crate) fn escape_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub(crate) fn string_literal(value: &str) -> String {
    format!("\"{}\"", escape_string(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_literal_escapes_skill_control_characters() {
        assert_eq!(
            string_literal("a\\b\"c\nd\re"),
            "\"a\\\\b\\\"c\\nd\\re\""
        );
    }

    #[test]
    fn string_literal_handles_empty_string() {
        assert_eq!(string_literal(""), "\"\"");
    }
}
