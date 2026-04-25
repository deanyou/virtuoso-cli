use crate::error::{Result, VirtuosoError};

/// A SKILL s-expression value.
#[derive(Debug, PartialEq, Clone)]
pub enum SexpVal {
    Nil,
    Bool(bool),
    Str(String),
    Atom(String),
    List(Vec<SexpVal>),
}

/// Parse a SKILL s-expression from `input`.
pub fn parse_sexp(input: &str) -> Result<SexpVal> {
    let mut p = Parser::new(input.trim());
    p.parse_value()
}

/// Convert a `SexpVal::List` of values to a `Vec<Option<String>>`.
/// Returns `None` if `val` is not a list.
pub fn sexp_to_str_list(val: &SexpVal) -> Option<Vec<Option<String>>> {
    match val {
        SexpVal::List(items) => Some(items.iter().map(sexp_val_to_opt_str).collect()),
        _ => None,
    }
}

fn sexp_val_to_opt_str(val: &SexpVal) -> Option<String> {
    match val {
        SexpVal::Nil => None,
        SexpVal::Str(s) => Some(s.clone()),
        SexpVal::Bool(true) => Some("t".into()),
        SexpVal::Bool(false) => Some("nil".into()),
        SexpVal::Atom(s) => Some(s.clone()),
        SexpVal::List(_) => None,
    }
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            input: s.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn consume(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn parse_value(&mut self) -> Result<SexpVal> {
        self.skip_ws();
        match self.peek() {
            None => Err(VirtuosoError::Execution(
                "unexpected end of sexp input".into(),
            )),
            Some(b'(') => {
                self.pos += 1;
                self.parse_list()
            }
            Some(b'"') => {
                self.pos += 1;
                self.parse_string()
            }
            _ => self.parse_atom(),
        }
    }

    fn parse_list(&mut self) -> Result<SexpVal> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b')') => {
                    self.pos += 1;
                    return Ok(SexpVal::List(items));
                }
                None => return Err(VirtuosoError::Execution("unterminated list".into())),
                _ => items.push(self.parse_value()?),
            }
        }
    }

    fn parse_string(&mut self) -> Result<SexpVal> {
        let mut result = String::new();
        loop {
            match self.consume() {
                None => return Err(VirtuosoError::Execution("unterminated string".into())),
                Some(b'"') => return Ok(SexpVal::Str(result)),
                Some(b'\\') => match self.consume() {
                    Some(b'"') => result.push('"'),
                    Some(b'\\') => result.push('\\'),
                    Some(b'n') => result.push('\n'),
                    Some(b't') => result.push('\t'),
                    Some(c) => {
                        result.push('\\');
                        result.push(c as char);
                    }
                    None => return Err(VirtuosoError::Execution("unterminated escape".into())),
                },
                Some(c) => result.push(c as char),
            }
        }
    }

    fn parse_atom(&mut self) -> Result<SexpVal> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() || b == b')' || b == b'(' || b == b'"' {
                break;
            }
            self.pos += 1;
        }
        let atom = std::str::from_utf8(&self.input[start..self.pos]).unwrap_or("");
        Ok(match atom {
            "nil" => SexpVal::Nil,
            "t" => SexpVal::Bool(true),
            s => SexpVal::Atom(s.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nil() {
        assert_eq!(parse_sexp("nil").unwrap(), SexpVal::Nil);
    }

    #[test]
    fn parse_bool_true() {
        assert_eq!(parse_sexp("t").unwrap(), SexpVal::Bool(true));
    }

    #[test]
    fn parse_atom() {
        assert_eq!(
            parse_sexp("fnxSession0").unwrap(),
            SexpVal::Atom("fnxSession0".into())
        );
    }

    #[test]
    fn parse_simple_string() {
        assert_eq!(
            parse_sexp(r#""hello""#).unwrap(),
            SexpVal::Str("hello".into())
        );
    }

    #[test]
    fn parse_string_with_escape() {
        assert_eq!(
            parse_sexp(r#""hello \"world\"""#).unwrap(),
            SexpVal::Str(r#"hello "world""#.into())
        );
    }

    #[test]
    fn parse_string_with_backslash_n() {
        assert_eq!(
            parse_sexp(r#""line1\nline2""#).unwrap(),
            SexpVal::Str("line1\nline2".into())
        );
    }

    #[test]
    fn parse_empty_list() {
        assert_eq!(parse_sexp("(  )").unwrap(), SexpVal::List(vec![]));
    }

    #[test]
    fn parse_flat_list() {
        assert_eq!(
            parse_sexp(r#"("a" "b" "c")"#).unwrap(),
            SexpVal::List(vec![
                SexpVal::Str("a".into()),
                SexpVal::Str("b".into()),
                SexpVal::Str("c".into()),
            ])
        );
    }

    #[test]
    fn parse_nested_list() {
        assert_eq!(
            parse_sexp(r#"(("fnxSession0" "idle") ("fnxSession1" nil))"#).unwrap(),
            SexpVal::List(vec![
                SexpVal::List(vec![
                    SexpVal::Str("fnxSession0".into()),
                    SexpVal::Str("idle".into()),
                ]),
                SexpVal::List(vec![SexpVal::Str("fnxSession1".into()), SexpVal::Nil,]),
            ])
        );
    }

    #[test]
    fn parse_nil_in_list() {
        assert_eq!(
            parse_sexp("(nil t nil)").unwrap(),
            SexpVal::List(vec![SexpVal::Nil, SexpVal::Bool(true), SexpVal::Nil])
        );
    }

    #[test]
    fn sexp_to_str_list_basic() {
        let val = SexpVal::List(vec![
            SexpVal::Str("fnxSession0".into()),
            SexpVal::Nil,
            SexpVal::Str("idle".into()),
        ]);
        let result = sexp_to_str_list(&val).unwrap();
        assert_eq!(
            result,
            vec![
                Some("fnxSession0".to_string()),
                None,
                Some("idle".to_string()),
            ]
        );
    }

    #[test]
    fn sexp_to_str_list_nil_returns_none() {
        assert!(sexp_to_str_list(&SexpVal::Nil).is_none());
    }

    #[test]
    fn sexp_to_str_list_bool_true_becomes_t() {
        let val = SexpVal::List(vec![SexpVal::Bool(true)]);
        let result = sexp_to_str_list(&val).unwrap();
        assert_eq!(result, vec![Some("t".to_string())]);
    }

    #[test]
    fn parse_whitespace_tolerance() {
        assert_eq!(
            parse_sexp("  ( nil  t )  ").unwrap(),
            SexpVal::List(vec![SexpVal::Nil, SexpVal::Bool(true)])
        );
    }
}
