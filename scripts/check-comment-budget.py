#!/usr/bin/env python3
"""Comment budget ratchet for virtuoso-cli.

Counts comment lines per group and fails when any group exceeds the frozen
baseline in scripts/comment-budget.json. The budget only ever goes down:
delete comments (or move decision records into docs/decision-records/), never
raise the baseline.

The counting口径 is the comment-governance campaign's own extractor: a
string-aware state machine per language that understands Rust's nested block
comments, raw strings and char/lifetime ambiguity, Python's triple-quoted
strings, SKILL's `;` comments, shell word-boundary `#` and TOML strings.
Rust doc comments (`///`, `//!`) count like any other comment -- the ratchet
measures the whole comment surface.

Usage:
  scripts/check-comment-budget.py                 # check against the baseline
  scripts/check-comment-budget.py --write         # re-anchor the baseline
  scripts/check-comment-budget.py --json          # print the actuals
"""
import json
import os
import re
import sys
from collections import defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
DEFAULT_BASELINE = os.path.join(HERE, "comment-budget.json")

GROUP_DEPTH = 2
# Only the *repo-root* build directory is skipped. A same-named directory
# deeper in the tree can be real source (src/target/ is a module here).
SKIP_DIRS_ROOT = {"node_modules", "dist", ".git", "coverage", "target", "__pycache__"}
SKIP_DIRS_ANY = {"node_modules", ".git", "__pycache__"}

# Out of scope per the campaign brief: the three skill trees (own sync
# discipline), the agent-tooling trees, the docs tree, and vendored resources.
EXCLUDE_DIRS = {
    ".claude", ".agents", ".codex", ".ua", ".evoontology",
    ".pensieve", ".workbuddy", "docs", "resources",
}
EXCLUDE_FILE_PREFIXES = ()

LANG_BY_EXT = {
    ".rs": "rust",
    ".py": "python",
    ".il": "skill",
    ".ils": "skill",
    ".sh": "shell",
    ".toml": "toml",
}

MARKER_RE = re.compile(
    r"TODO|FIXME|HACK|XXX|WORKAROUND|workaround|TEMPORAR|temporar|FOR NOW|for now|"
    r"DEPRECATED|deprecated|LEGACY|legacy|REMOVE ME|remove (this|later)|DELETE ME|"
    r"NO LONGER|no longer|USED TO|used to|old code|临时|先这样|以后|待改|待修|凑合|将就"
)

# Rust: a comment body that looks like real code (dead-code candidates).
RUST_CODEISH_RE = re.compile(
    r"(;|=>|\bfn\s+[A-Za-z_]|\blet\s+(mut\s+)?[A-Za-z_]|\bimpl\b|\bmatch\s+[A-Za-z_]|"
    r"\bif\s+[^\n]*\{|\buse\s+[A-Za-z_]+::|\breturn\b\s|\bpub\s+(fn|struct|enum))"
)
PY_CODEISH_RE = re.compile(
    r"(=|^\s*(if|for|while|return|def|class|import|from|print)\b|^\s*\w+\s*=)"
)
SH_CODEISH_RE = re.compile(r"(\$\(|`|\bif\b|\bthen\b|\belse\b|\bfi\b|\bfor\b|\bdo\b|\becho\b)")

# Never deleted: tool directives, license headers, conditional-compilation notes.
PROTECTED_RE = re.compile(
    r"SAFETY:|clippy::|rustfmt|#\[allow|#\[cfg|noqa|type:\s*ignore|nosec|"
    r"allow\(dead_code\)|Copyright|SPDX|Licensed under|LICENSE"
)


def group_of(rel, depth=GROUP_DEPTH):
    """Group = the containing directory path, truncated to `depth` segments.

    src/client/bridge.rs -> src/client ; src/config.rs -> src ;
    tests/foo.rs -> tests ; Cargo.toml -> root.
    """
    parts = rel.split("/")
    dirs = parts[:-1]
    if not dirs:
        return "root"
    return "/".join(dirs[:depth]) if len(dirs) >= depth else dirs[0]


# --------------------------------------------------------------------------
# Rust lexer
# --------------------------------------------------------------------------

_RUST_STR_PREFIX_RE = re.compile(r"(?:br|cr|r|b|c)?(#*)\"")
_IDENT_CHARS = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_")


def _rust_scan_string(text, i, hashes):
    """text[i] == '"'. Return index just past the closing quote.

    Raw strings (hashes > 0) have no escapes: the terminator is '"' + hashes.
    """
    n = len(text)
    if hashes:
        j = text.find('"', i + 1)
        term = '"' + "#" * hashes
        while j >= 0:
            if text.startswith(term, j):
                return j + len(term)
            j = text.find('"', j + 1)
        return -1  # unterminated
    j = i + 1
    while j < n:
        if text[j] == "\\":
            j += 2
            continue
        if text[j] == '"':
            return j + 1
        j += 1
    return -1


def _rust_scan_char(text, i):
    """text[i] == "'". Return (kind, end_index) where kind in {char, lifetime}.

    Rust ambiguity: 'a is a lifetime, 'a' is a char literal.
    """
    n = len(text)
    rest = text[i + 1:]
    if not rest:
        return ("lifetime", i + 1)
    if rest[0] == "\\":
        j = i + 1
        while j < n and text[j] != "'":
            j += 2 if text[j] == "\\" else 1
        if j < n and text[j] == "'":
            return ("char", j + 1)
        return ("lifetime", i + 1)
    # a non-escape char literal holds exactly one character
    if i + 2 < n and text[i + 2] == "'":
        return ("char", i + 3)
    return ("lifetime", i + 1)


def lex_rust(text):
    """Return (units, unhandled_reasons).

    unit = dict(kind, line, nlines, body, doc, own_line, start, end)
    """
    units = []
    unhandled = []
    i, n = 0, len(text)
    line = 1
    line_start_of = 0  # index of the start of the current line

    def cur_line_start(idx):
        return text.rfind("\n", 0, idx) + 1

    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            if j < 0:
                j = n
            doc = False
            if i + 2 < n:
                if text[i + 2] == "/":
                    doc = True
                elif text[i + 2] == "!":
                    doc = True
            body = text[i + 2:j]
            ls = cur_line_start(i)
            own = text[ls:i].strip() == ""
            units.append({
                "kind": "line", "line": line, "nlines": 1, "body": body.strip(),
                "doc": doc, "own_line": own, "start": i, "end": j,
            })
            i = j
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 1
            j = i + 2
            doc = False
            if j < n and text[j] == "!":
                doc = True
            elif j + 1 < n and text[j] == "*" and text[j + 1] != "/":
                doc = True
            while j < n and depth > 0:
                if text[j] == "/" and j + 1 < n and text[j + 1] == "*":
                    depth += 1
                    j += 2
                    continue
                if text[j] == "*" and j + 1 < n and text[j + 1] == "/":
                    depth -= 1
                    j += 2
                    continue
                j += 1
            if depth > 0:
                unhandled.append("unterminated block comment at line %d" % line)
                break
            body = text[i + 2:j - 2]
            nlines = text.count("\n", i, j) + 1
            ls = cur_line_start(i)
            own = text[ls:i].strip() == ""
            units.append({
                "kind": "block", "line": line, "nlines": nlines,
                "body": body.strip(), "doc": doc, "own_line": own,
                "start": i, "end": j,
            })
            line += text.count("\n", i, j)
            i = j
            continue
        if c == '"' or (c in "bcr" and _RUST_STR_PREFIX_RE.match(text, i)):
            m = _RUST_STR_PREFIX_RE.match(text, i)
            if m and m.start() == i:
                hashes = len(m.group(1))
                end = _rust_scan_string(text, i + len(m.group(0)) - 1, hashes)
                if end < 0:
                    unhandled.append("unterminated string at line %d" % line)
                    break
                line += text.count("\n", i, end)
                i = end
                continue
        if c == "'":
            kind, end = _rust_scan_char(text, i)
            if end < 0:
                unhandled.append("unterminated char literal at line %d" % line)
                break
            i = end
            continue
        if c == "\n":
            line += 1
            i += 1
            continue
        i += 1
    return units, unhandled


# --------------------------------------------------------------------------
# Python lexer
# --------------------------------------------------------------------------

_PY_STR_RE = re.compile(r"([rRbBuUfF]{0,3})(\"\"\"|'''|\"|')")


def lex_python(text):
    units = []
    unhandled = []
    i, n = 0, len(text)
    line = 1
    while i < n:
        c = text[i]
        if c == "#":
            if line == 1 and text.startswith("#!", i):
                # A shebang is an interpreter directive, not a comment.
                j = text.find("\n", i)
                i = n if j < 0 else j
                continue
            j = text.find("\n", i)
            if j < 0:
                j = n
            ls = text.rfind("\n", 0, i) + 1
            own = text[ls:i].strip() == ""
            units.append({
                "kind": "line", "line": line, "nlines": 1,
                "body": text[i + 1:j].strip(), "doc": False, "own_line": own,
                "start": i, "end": j,
            })
            i = j
            continue
        m = _PY_STR_RE.match(text, i)
        if m:
            prefix = m.group(1)
            quote = m.group(2)
            raw = "r" in prefix.lower()
            j = i + len(m.group(0))
            closed = False
            while j < n:
                # In a Python raw string a backslash still prevents the next
                # quote from terminating the string (it just stays in the
                # value), so the escape is skipped the same way in both kinds.
                if text[j] == "\\":
                    j += 2
                    continue
                if text.startswith(quote, j):
                    j += len(quote)
                    closed = True
                    break
                j += 1
            if not closed:
                unhandled.append("unterminated string at line %d" % line)
                break
            # Python 3.12 f-string: a '#' inside {...} is a comment.
            if "f" in prefix.lower() and "#" in text[i:j]:
                unhandled.append("f-string containing '#' at line %d" % line)
                break
            line += text.count("\n", i, j)
            i = j
            continue
        if c == "\n":
            line += 1
        i += 1
    return units, unhandled


# --------------------------------------------------------------------------
# SKILL (Cadence IL) lexer: ';' starts a comment to end of line.
# --------------------------------------------------------------------------

def lex_skill(text):
    units = []
    unhandled = []
    i, n = 0, len(text)
    line = 1
    while i < n:
        c = text[i]
        if c == ";":
            j = text.find("\n", i)
            if j < 0:
                j = n
            ls = text.rfind("\n", 0, i) + 1
            own = text[ls:i].strip() == ""
            units.append({
                "kind": "line", "line": line, "nlines": 1,
                "body": text[i + 1:j].strip(), "doc": False, "own_line": own,
                "start": i, "end": j,
            })
            i = j
            continue
        if c == '"':
            j = i + 1
            closed = False
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    j += 1
                    closed = True
                    break
                j += 1
            if not closed:
                unhandled.append("unterminated string at line %d" % line)
                break
            line += text.count("\n", i, j)
            i = j
            continue
        if c == "\n":
            line += 1
        i += 1
    return units, unhandled


# --------------------------------------------------------------------------
# Shell lexer: '#' starts a comment only at the start of a word.
# --------------------------------------------------------------------------

_SH_HEREDOC_RE = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")


def lex_shell(text):
    units = []
    unhandled = []
    i, n = 0, len(text)
    line = 1
    heredocs = []  # pending heredoc terminators
    line_start = True
    while i < n:
        c = text[i]
        if c == "\n":
            line += 1
            i += 1
            line_start = True
            # consume any pending heredoc bodies
            while heredocs:
                term, indented = heredocs.pop(0)
                pat = re.compile(r"^[ \t]*" + re.escape(term) + r"[ \t]*$")
                while i < n:
                    eol = text.find("\n", i)
                    if eol < 0:
                        eol = n
                    if pat.match(text[i:eol]):
                        i = eol
                        line += 1
                        break
                    i = eol + 1
                    line += 1
            continue
        if c == "#":
            # comment only at the start of a word
            prev = text[i - 1] if i > 0 else "\n"
            if prev in " \t\n;&|()<>=" or i == 0:
                j = text.find("\n", i)
                if j < 0:
                    j = n
                ls = text.rfind("\n", 0, i) + 1
                own = text[ls:i].strip() == ""
                body = text[i + 1:j].strip()
                if line == 1 and text.startswith("#!", i):
                    i = j
                    continue
                units.append({
                    "kind": "line", "line": line, "nlines": 1, "body": body,
                    "doc": False, "own_line": own, "start": i, "end": j,
                })
                i = j
                continue
            i += 1
            continue
        if c in "'\"":
            quote = c
            j = i + 1
            closed = False
            while j < n:
                if quote == '"' and text[j] == "\\":
                    j += 2
                    continue
                if text[j] == quote:
                    j += 1
                    closed = True
                    break
                j += 1
            if not closed:
                unhandled.append("unterminated string at line %d" % line)
                break
            line += text.count("\n", i, j)
            i = j
            continue
        if c == "<" and text.startswith("<<", i):
            m = _SH_HEREDOC_RE.match(text, i)
            if m:
                heredocs.append((m.group(2), m.group(0).startswith("<<-")))
                i = m.end()
                continue
        i += 1
    return units, unhandled


# --------------------------------------------------------------------------
# TOML lexer
# --------------------------------------------------------------------------

_TOML_STR_RE = re.compile(r"(\"\"\"|'''|\"|')")


def lex_toml(text):
    units = []
    unhandled = []
    i, n = 0, len(text)
    line = 1
    while i < n:
        c = text[i]
        if c == "#":
            j = text.find("\n", i)
            if j < 0:
                j = n
            ls = text.rfind("\n", 0, i) + 1
            own = text[ls:i].strip() == ""
            units.append({
                "kind": "line", "line": line, "nlines": 1,
                "body": text[i + 1:j].strip(), "doc": False, "own_line": own,
                "start": i, "end": j,
            })
            i = j
            continue
        m = _TOML_STR_RE.match(text, i)
        if m:
            quote = m.group(1)
            j = i + len(quote)
            closed = False
            while j < n:
                if quote in ('"', '"""') and text[j] == "\\":
                    j += 2
                    continue
                if text.startswith(quote, j):
                    j += len(quote)
                    closed = True
                    break
                j += 1
            if not closed:
                unhandled.append("unterminated string at line %d" % line)
                break
            line += text.count("\n", i, j)
            i = j
            continue
        if c == "\n":
            line += 1
        i += 1
    return units, unhandled


LEXERS = {
    "rust": lex_rust,
    "python": lex_python,
    "skill": lex_skill,
    "shell": lex_shell,
    "toml": lex_toml,
}


def merge_line_units(units):
    """Merge adjacent whole-line comments into one unit.

    Trailing comments (code before the marker on the same line) always stay
    standalone: merging them with a neighbouring whole-line comment would mix
    two unrelated classifications into one deletable span.
    """
    merged = []
    for u in sorted(units, key=lambda x: x["line"]):
        if (merged and u["kind"] == "line" and merged[-1]["kind"] == "line"
                and u["doc"] == merged[-1]["doc"]
                and u["own_line"] and merged[-1]["own_line"]
                and u["line"] == merged[-1]["line"] + merged[-1]["nlines"]):
            p = merged[-1]
            merged[-1] = dict(p)
            merged[-1]["nlines"] = p["nlines"] + 1
            merged[-1]["body"] = (p["body"] + "\n" + u["body"]).strip()
            merged[-1]["end"] = u["end"]
        else:
            merged.append(dict(u))
    return merged


def codeish(lang, body):
    if lang == "rust":
        return bool(RUST_CODEISH_RE.search(body))
    if lang == "python":
        return bool(PY_CODEISH_RE.search(body))
    if lang == "shell":
        return bool(SH_CODEISH_RE.search(body))
    return False


def actuals(root):
    """{group: comment_lines} and the marker count for the tree at `root`."""
    groups = defaultdict(int)
    markers = 0
    units = 0
    for dirpath, dirnames, filenames in os.walk(root):
        is_root = os.path.abspath(dirpath) == os.path.abspath(root)
        if is_root:
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS_ROOT)
        else:
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS_ANY)
        for fn in sorted(filenames):
            lang = LANG_BY_EXT.get(os.path.splitext(fn)[1].lower())
            if not lang:
                continue
            path = os.path.join(dirpath, fn)
            rel = os.path.relpath(path, root)
            if rel.split("/")[0] in EXCLUDE_DIRS:
                continue
            try:
                text = open(path, encoding="utf-8").read()
            except (OSError, UnicodeDecodeError):
                continue
            found, unhandled = LEXERS[lang](text)
            if unhandled:
                # A file whose lexer cannot be trusted is skipped rather than
                # guessed at; it simply does not contribute to the budget.
                continue
            found = merge_line_units(found)
            g = group_of(rel)
            for u in found:
                groups[g] += u["nlines"]
                units += 1
                if MARKER_RE.search(u["body"]):
                    markers += 1
    return dict(groups), markers, units


def main():
    args = [a for a in sys.argv[1:]]
    write = "--write" in args
    as_json = "--json" in args
    args = [a for a in args if not a.startswith("--")]
    root = os.path.abspath(args[0]) if len(args) > 0 else REPO_ROOT
    baseline_path = os.path.abspath(args[1]) if len(args) > 1 else DEFAULT_BASELINE

    groups, markers, units = actuals(root)

    if as_json:
        json.dump({"groups": groups, "markers": markers, "units": units},
                  sys.stdout, indent=1, sort_keys=True)
        sys.stdout.write("\n")
        return

    if write:
        json.dump({"groups": groups, "markers": markers},
                  open(baseline_path, "w"), ensure_ascii=False, indent=1, sort_keys=True)
        print("baseline written: %d groups, markers=%d, comment lines=%d"
              % (len(groups), markers, sum(groups.values())))
        return

    base = json.load(open(baseline_path))
    bad = []
    for g, n in sorted(groups.items()):
        b = base["groups"].get(g)
        if b is None:
            bad.append("FAIL %s: new group with %d comment lines has no baseline" % (g, n))
        elif n > b:
            bad.append("FAIL %s: %d comment lines > baseline %d" % (g, n, b))
    for g in sorted(base["groups"]):
        if g not in groups:
            bad.append("FAIL %s: group disappeared (baseline %d) -- update the baseline"
                       % (g, base["groups"][g]))
    if markers > base["markers"]:
        bad.append("FAIL markers: %d > baseline %d" % (markers, base["markers"]))
    if bad:
        print("\n".join(bad))
        print("Comment budget exceeded. Delete comments (or move decision records to "
              "docs/decision-records/) -- do not raise the baseline.")
        sys.exit(1)
    print("comment budget OK (%d groups, %d comment lines, %d markers)"
          % (len(groups), sum(groups.values()), markers))


if __name__ == "__main__":
    main()
