#!/bin/bash
# PostToolUse hook: capture EDA errors from vcli/virtuoso/spectre Bash commands.
# Classifies SFE-xxx, OSSHNL-xxx, bridge NAK, etc. into skill memory/.
#
# Input: JSON via stdin with tool_name, tool_input.command, tool_response.output

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# ── Filter: only vcli / virtuoso / spectre commands ──────────────────────────
if ! echo "$COMMAND" | grep -qE "(vcli|virtuoso|spectre)\b"; then
    exit 0
fi

command -v python3 &>/dev/null || exit 0
command -v jq &>/dev/null || exit 0

SAVER="${CLAUDE_PROJECT_DIR}/.claude/skills/_shared/scripts/eda_memory_saver.py"
[[ -f "$SAVER" ]] || exit 0

# ── Extract output ────────────────────────────────────────────────────────────
OUTPUT=$(echo "$INPUT" | jq -r '.tool_response.output // empty' 2>/dev/null)
[[ -n "$OUTPUT" ]] || exit 0

# ── Quick pre-filter: only run saver if output looks like an error ────────────
if ! echo "$OUTPUT" | grep -qiE "SFE-|OSSHNL-|terminated prematurely|ipcBeginProcess|\[NAK\]|not registered|createNetlist.*nil|connection refused|exit.*127"; then
    exit 0
fi

# ── Classify source ───────────────────────────────────────────────────────────
if echo "$COMMAND" | grep -q "spectre"; then
    SOURCE="spectre"
elif echo "$COMMAND" | grep -qE "vcli|virtuoso"; then
    SOURCE="vcli"
else
    SOURCE="unknown"
fi

# ── Run saver ─────────────────────────────────────────────────────────────────
RESULT=$(echo "$OUTPUT" | python3 "$SAVER" "$COMMAND" - "$SOURCE" 2>&1)
SAVE_STATUS=$?

if [[ $SAVE_STATUS -eq 0 ]]; then
    SKILL=$(echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('skill',''))" 2>/dev/null)
    PATTERN=$(echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('pattern',''))" 2>/dev/null)
    jq -n \
        --arg skill "$SKILL" \
        --arg pattern "$PATTERN" \
        '{systemMessage: ("[eda_capture] 已记录到 " + $skill + "/memory/: " + $pattern)}'
fi
