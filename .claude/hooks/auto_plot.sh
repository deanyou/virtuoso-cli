#!/bin/bash
# PostToolUse hook: auto-plot simulation results after vcli/virtuoso sim commands.
# Triggers on: sim sweep / sim measure / process char — when output is JSON.
# Saves PNG to plots/ and injects the path as systemMessage.
#
# Supported commands:
#   vcli sim sweep --format json        → line plot
#   vcli sim measure --format json      → horizontal bar chart
#   vcli process char --format json     → gm/Id lookup table curves
#
# Input: JSON via stdin with tool_name, tool_input.command, tool_response.output

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# ── Filter: only sim sweep / sim measure / process char ───────────────────────
if ! echo "$COMMAND" | grep -qE "(vcli|virtuoso) (sim (sweep|measure)|process char)"; then
    exit 0
fi

# Require --format json (or format=json) — other formats don't produce plottable JSON
if ! echo "$COMMAND" | grep -qE "\-\-format[= ]json"; then
    exit 0
fi

command -v python3 &>/dev/null || exit 0
command -v jq &>/dev/null || exit 0

PLOT_SCRIPT="${CLAUDE_PROJECT_DIR}/.claude/skills/sim-plot/scripts/plot_sim.py"
[[ -f "$PLOT_SCRIPT" ]] || exit 0

# ── Extract JSON from tool response ───────────────────────────────────────────
OUTPUT=$(echo "$INPUT" | jq -r '.tool_response.output // empty' 2>/dev/null)
[[ -n "$OUTPUT" ]] || exit 0

# Validate it's parseable JSON
echo "$OUTPUT" | python3 -c "import sys, json; json.load(sys.stdin)" 2>/dev/null || exit 0

# ── Determine plot prefix from command type ────────────────────────────────────
if echo "$COMMAND" | grep -q "sim sweep"; then
    PREFIX="sweep"
elif echo "$COMMAND" | grep -q "sim measure"; then
    PREFIX="measure"
elif echo "$COMMAND" | grep -q "process char"; then
    PREFIX="gmid"
else
    PREFIX="sim"
fi

# ── Auto-generate output path under plots/ ────────────────────────────────────
PLOTS_DIR="${CLAUDE_PROJECT_DIR}/plots"
mkdir -p "$PLOTS_DIR"
PLOT_FILE="${PLOTS_DIR}/${PREFIX}_$(date +%Y%m%d_%H%M%S).png"

# ── Run plot_sim.py ────────────────────────────────────────────────────────────
RESULT=$(echo "$OUTPUT" | python3 "$PLOT_SCRIPT" --output "$PLOT_FILE" 2>&1)
PLOT_STATUS=$?

if [[ $PLOT_STATUS -eq 0 ]]; then
    REL_PATH="${PLOT_FILE#"$CLAUDE_PROJECT_DIR/"}"
    jq -n \
        --arg path "$REL_PATH" \
        --arg abs "$PLOT_FILE" \
        '{systemMessage: ("[auto_plot] Chart saved → " + $path + " (open with: xdg-open " + $abs + ")")}'
else
    # Silent failure — don't spam if data format isn't plottable
    exit 0
fi
