#!/bin/bash
# PostToolUse hook: auto-check Rust files after Write/Edit.
# Runs rustfmt --check on the edited file, then cargo clippy if fmt is clean.
#
# Input: JSON via stdin with tool_name, tool_input.file_path

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

# Only process .rs files inside the project
[[ "$FILE" == *.rs ]] || exit 0
[[ "$FILE" == "$CLAUDE_PROJECT_DIR"/* ]] || exit 0

command -v rustfmt &>/dev/null || exit 0
command -v jq &>/dev/null || exit 0

# ── rustfmt check ─────────────────────────────────────────────────────────────
FMT_OUT=$(rustfmt --edition 2021 --check "$FILE" 2>&1)
FMT_STATUS=$?

if [[ $FMT_STATUS -ne 0 ]]; then
    jq -n \
        --arg file "$(basename "$FILE")" \
        --arg result "$FMT_OUT" \
        '{systemMessage: ("[rust_check] fmt errors in " + $file + " — run `cargo fmt`\n" + $result)}'
    exit 0
fi

# ── cargo clippy (only when fmt is clean) ─────────────────────────────────────
cd "$CLAUDE_PROJECT_DIR" || exit 0

CLIPPY_OUT=$(cargo clippy -q -- -D warnings 2>&1)
CLIPPY_STATUS=$?

if [[ $CLIPPY_STATUS -ne 0 ]]; then
    # Show only the error lines to keep systemMessage concise
    ERRORS=$(echo "$CLIPPY_OUT" | grep -E "^error(\[|:)" | head -10)
    HINT=$(echo "$CLIPPY_OUT" | grep -E "^\s+= help:" | head -5)
    jq -n \
        --arg file "$(basename "$FILE")" \
        --arg errors "$ERRORS" \
        --arg hint "$HINT" \
        '{systemMessage: ("[rust_check] clippy errors after editing " + $file + ":\n" + $errors + "\n" + $hint)}'
fi
