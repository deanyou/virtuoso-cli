//! Guards generated SKILL against unbalanced parentheses.
//!
//! A missing `)` in a SKILL builder fails neither the build nor the unit
//! tests that assert on substrings: the string is handed to Virtuoso's
//! reader, which silently auto-closes it, and the expression stops meaning
//! what it was written to mean. `shape_grid_audit_skill` (PR #122) lost one
//! this way — the audit was absorbed into the unclosed `procedure(` body,
//! never ran, `dbClose` never fired, and the command returned nil with no
//! error at all.
//!
//! This walks the crate's own sources and balances every raw string that is
//! not inside a `#[cfg(test)]` module, so the defect is caught before review.

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Files whose raw strings are regexes or output parsers, not SKILL.
const EXCLUDED: &[&str] = &["src/spectre/parsers.rs", "src/skill_finder/parser.rs"];

struct RawString {
    /// Offset of the opening `r`, used for line numbers and region tests.
    start: usize,
    /// Span of the literal's contents, excluding the `r#"` / `"#` delimiters.
    content: Range<usize>,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_char(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// End offset of the char literal at `i`, or `None` if it is a lifetime.
fn char_literal_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if j >= b.len() {
        return None;
    }
    if b[j] == b'\\' {
        j += 2;
        while j < b.len() && is_ident_char(b[j]) {
            j += 1;
        }
    } else if b[j].is_ascii() {
        j += 1;
    } else {
        return None;
    }
    (j < b.len() && b[j] == b'\'').then_some(j + 1)
}

/// Collect every raw string and every `#[cfg(test)]` span in `text`.
///
/// Byte-wise is safe here: every delimiter is ASCII, and UTF-8 continuation
/// bytes never collide with them.
fn scan_source(text: &str, whole_file_is_test: bool) -> (Vec<RawString>, Vec<Range<usize>>) {
    let b = text.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    let mut raw = Vec::new();
    let mut test_spans = Vec::new();
    if whole_file_is_test {
        test_spans.push(0..n);
    }
    let mut pending_test: Option<usize> = None;
    let mut stack: Vec<Option<usize>> = Vec::new();

    while i < n {
        let c = b[i];

        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut level = 1usize;
            i += 2;
            while i < n && level > 0 {
                if b.get(i) == Some(&b'/') && b.get(i + 1) == Some(&b'*') {
                    level += 1;
                    i += 2;
                } else if b.get(i) == Some(&b'*') && b.get(i + 1) == Some(&b'/') {
                    level -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }

        // Raw string: r"...", r#"..."#, r##"..."##. Record contents only —
        // handing the delimiters to strip_skill_strings would make the
        // opening `"` look like a SKILL string quote and blank the body.
        if c == b'r' && matches!(b.get(i + 1), Some(b'#') | Some(b'"')) {
            let mut j = i + 1;
            let mut hashes = 0usize;
            while j < n && b[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if b.get(j) == Some(&b'"') {
                let content_start = j + 1;
                let mut k = content_start;
                while k < n {
                    if b[k] == b'"' && b[k + 1..].iter().take(hashes).all(|&h| h == b'#') {
                        break;
                    }
                    k += 1;
                }
                let end = if k < n { k + 1 + hashes } else { n };
                raw.push(RawString {
                    start: i,
                    content: content_start..k,
                });
                i = end;
                continue;
            }
        }

        if c == b'b' && b.get(i + 1) == Some(&b'"') {
            i += 1;
            continue;
        }
        if c == b'"' {
            i += 1;
            while i < n {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == b'\'' {
            if let Some(end) = char_literal_end(b, i) {
                i = end;
                continue;
            }
        }

        // Byte slices: `i` walks raw bytes and may sit inside a multi-byte
        // UTF-8 character, so slicing the &str would panic.
        if b[i..].starts_with(b"#[cfg(test)]") {
            pending_test = Some(i);
            i += b"#[cfg(test)]".len();
            continue;
        }
        if c == b'm' && b[i..].starts_with(b"mod") && (i == 0 || !is_ident_char(b[i - 1])) {
            let mut j = i + 3;
            while j < n && b[j] == b' ' {
                j += 1;
            }
            if j < n && is_ident_start(b[j]) {
                let mut k = j;
                while k < n && is_ident_char(b[k]) {
                    k += 1;
                }
                if b[j..k].starts_with(b"test") {
                    pending_test = Some(i);
                }
            }
        }

        if c == b'{' {
            stack.push(pending_test.take());
        } else if c == b'}' {
            if let Some(Some(start)) = stack.pop() {
                test_spans.push(start..i + 1);
            }
        }
        i += 1;
    }

    (raw, test_spans)
}

/// Blank out SKILL `"..."` literals and `;` comments; parens inside them are
/// data, not structure.
fn strip_skill_strings(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            i += 2;
            continue;
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(b' ');
            continue;
        }
        if b[i] == b';' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            out.push(b' ');
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// `(net depth, minimum depth reached)` over a literal's skeleton.
///
/// `format!` placeholders like `{lib}` contain no parentheses, so they are
/// counted as-is without needing the substituted value.
fn balance(bytes: &[u8]) -> (i32, i32) {
    let mut depth = 0i32;
    let mut lowest = 0i32;
    for &c in bytes {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                lowest = lowest.min(depth);
            }
            _ => {}
        }
    }
    (depth, lowest)
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Unbalanced SKILL parens in every builder outside the test modules.
#[test]
fn generated_skill_parens_are_balanced() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    let mut checked = 0usize;

    for path in rust_files(&root.join("src")) {
        let rel = path
            .strip_prefix(&root)
            .expect("under manifest dir")
            .to_string_lossy()
            .replace('\\', "/");
        if EXCLUDED.contains(&rel.as_str()) {
            continue;
        }
        let text = fs::read_to_string(&path).expect("source is readable");
        let (raw, tests) = scan_source(&text, false);
        for r in &raw {
            if tests.iter().any(|t| t.contains(&r.start)) {
                continue;
            }
            checked += 1;
            let (depth, lowest) = balance(&strip_skill_strings(&text[r.content.clone()]));
            if depth != 0 || lowest < 0 {
                let line = text[..r.start].lines().count();
                let kind = if lowest < 0 { "EXTRA )" } else { "MISSING )" };
                offenders.push(format!("{rel}:{line}  net={depth:+}  {kind}"));
            }
        }
    }

    assert!(
        checked > 50,
        "only {checked} raw strings checked — scanner is broken"
    );
    assert!(
        offenders.is_empty(),
        "unbalanced parens in generated SKILL:\n{}",
        offenders.join("\n")
    );
}

// A guard that cannot fail guards nothing. These pin the scanner's behaviour
// on the cases that matter.
//
// Fixtures use r##"..."## because they embed r#"..."# source literals; a
// single-hash outer string would terminate on the inner closing delimiter.

#[test]
fn scanner_flags_a_missing_closing_paren() {
    // The exact shape that shipped in PR #122: the final `result)` closes the
    // inner let, leaving procedure( open so the reader absorbs the rest.
    let src = r##"
        fn build() -> String {
            r#"procedure(checkGrid(cv) let((r) r = nil result)"#
        }
    "##;
    let (raw, _) = scan_source(src, false);
    assert_eq!(raw.len(), 1);
    let (depth, lowest) = balance(&strip_skill_strings(&src[raw[0].content.clone()]));
    assert_eq!((depth, lowest), (1, 0), "must report one unclosed paren");
}

#[test]
fn scanner_flags_an_extra_closing_paren() {
    let src = r##"fn f() -> String { r#"let((a) a = 1))"# }"##;
    let (raw, _) = scan_source(src, false);
    let (depth, lowest) = balance(&strip_skill_strings(&src[raw[0].content.clone()]));
    assert_eq!((depth, lowest), (-1, -1));
}

#[test]
fn scanner_ignores_parens_inside_skill_string_literals() {
    let src = r##"fn f() -> String { r#"member(t list("polygon" "path"))"# }"##;
    let (raw, _) = scan_source(src, false);
    let (depth, lowest) = balance(&strip_skill_strings(&src[raw[0].content.clone()]));
    assert_eq!(
        (depth, lowest),
        (0, 0),
        "parens inside a SKILL string are data"
    );
}

#[test]
fn scanner_ignores_raw_strings_inside_test_modules() {
    let src = r##"
        fn f() -> String { r#"let((a) a = 1)"# }
        #[cfg(test)]
        mod tests {
            #[test]
            fn t() {
                assert!(s.contains(r#"list(0 0) "R0")"#));
                assert!(s.contains(r#"cell"name"#));
            }
        }
    "##;
    let (raw, tests) = scan_source(src, false);
    let outside: Vec<_> = raw
        .iter()
        .filter(|r| !tests.iter().any(|t| t.contains(&r.start)))
        .collect();
    assert_eq!(outside.len(), 1, "both assertion literals must be filtered");
    let (depth, _) = balance(&strip_skill_strings(&src[outside[0].content.clone()]));
    assert_eq!(depth, 0);
}

#[test]
fn scanner_ignores_rust_delimiters_when_balancing() {
    // Regression guard: counting from the opening delimiter instead of the
    // content makes that quote blank the whole body and hide the defect.
    let src = r##"fn f() -> String { r#"procedure(checkGrid(cv) let((r) r = nil result)"# }"##;
    let (raw, _) = scan_source(src, false);
    let body = &src[raw[0].content.clone()];
    assert!(!body.starts_with('"') && !body.ends_with('"'));
    let (depth, _) = balance(&strip_skill_strings(body));
    assert_eq!(depth, 1, "the unclosed procedure paren must still be seen");
}
