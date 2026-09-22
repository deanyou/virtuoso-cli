#!/bin/bash
# PostToolUse hook: auto-fix Rust files after Write/Edit.
# Runs cargo fmt (auto-fix), then cargo clippy if fmt is clean.
#
# Input: JSON via stdin with tool_name, tool_input.file_path

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

# Only process .rs files inside the project
[[ "$FILE" == *.rs ]] || exit 0
[[ "$FILE" == "$CLAUDE_PROJECT_DIR"/* ]] || exit 0

command -v rustfmt &>/dev/null || exit 0
command -v jq &>/dev/null || exit 0

cd "$CLAUDE_PROJECT_DIR" || exit 0

# ── cargo fmt (auto-fix, then report if anything changed) ─────────────────────
FMT_BEFORE=$(rustfmt --edition 2021 --check "$FILE" 2>&1; echo $?)
cargo fmt --quiet 2>/dev/null
FMT_AFTER=$(rustfmt --edition 2021 --check "$FILE" 2>&1; echo $?)

if [[ "${FMT_BEFORE##*$'\n'}" -ne 0 && "${FMT_AFTER##*$'\n'}" -eq 0 ]]; then
    jq -n --arg file "$(basename "$FILE")" \
        '{systemMessage: ("[rust_check] auto-applied cargo fmt to " + $file)}'
fi

# ── cargo clippy (only on clean fmt) ──────────────────────────────────────────
CLIPPY_OUT=$(cargo clippy -q -- -D warnings 2>&1)
CLIPPY_STATUS=$?

if [[ $CLIPPY_STATUS -ne 0 ]]; then
    ERRORS=$(echo "$CLIPPY_OUT" | grep -E "^error(\[|:)" | head -10)
    HINT=$(echo "$CLIPPY_OUT" | grep -E "^\s+= help:" | head -5)
    jq -n \
        --arg file "$(basename "$FILE")" \
        --arg errors "$ERRORS" \
        --arg hint "$HINT" \
        '{systemMessage: ("[rust_check] clippy errors after editing " + $file + ":\n" + $errors + "\n" + $hint)}'
fi
