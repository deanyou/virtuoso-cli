#!/usr/bin/env bash
set -euo pipefail

# setup-virtuoso-tmux.sh
# Creates a tmux session for Virtuoso TUI workflow
# Left pane: SSH to Docker with X11 forwarding for Virtuoso
# Right pane: Local vcli commands (port 36539 already tunneled)

SESSION="${VIRTUOSO_TMUX_SESSION:-virtuoso-tui}"
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o BatchMode=yes"
SSH_KEY="${VIRTUOSO_SSH_KEY:-$HOME/.ssh/id_rsa}"
SSH_HOST="${VIRTUOSO_SSH_HOST:-localhost}"
SSH_USER="${VIRTUOSO_SSH_USER:-user}"
SSH_PORT="${VIRTUOSO_SSH_PORT:-2222}"
CADENCE_ENV="${VIRTUOSO_CADENCE_ENV:-/opt/cadence_env.sh}"
REMOTE_DISPLAY="${VIRTUOSO_DISPLAY:-localhost:11.0}"
BRIDGE_PORT="${VB_PORT:-36539}"
BRIDGE_TIMEOUT="${VB_TIMEOUT:-60}"
CLI_DIR="${VIRTUOSO_CLI_DIR:-$HOME/git/virtuoso-cli}"

# Kill existing session
tmux kill-session -t "$SESSION" 2>/dev/null || true

# Capture pane IDs instead of assuming the user's base-index/pane-base-index.
LEFT_PANE="$(tmux new-session -d -P -F '#{pane_id}' -s "$SESSION" -x 180 -y 45)"
RIGHT_PANE="$(tmux split-window -d -h -P -F '#{pane_id}' -t "$LEFT_PANE")"

# Left pane: SSH to container with X11 forwarding
tmux send-keys -t "$LEFT_PANE" "ssh -X -p $SSH_PORT -i $SSH_KEY $SSH_OPTS $SSH_USER@$SSH_HOST" Enter
sleep 3

# Source cadence environment
tmux send-keys -t "$LEFT_PANE" "source $CADENCE_ENV" Enter
sleep 1
tmux send-keys -t "$LEFT_PANE" "export DISPLAY=$REMOTE_DISPLAY" Enter
sleep 1

# Right pane: Local shell for vcli
tmux send-keys -t "$RIGHT_PANE" "export VB_PORT=$BRIDGE_PORT" Enter
tmux send-keys -t "$RIGHT_PANE" "export VB_TIMEOUT=$BRIDGE_TIMEOUT" Enter
tmux send-keys -t "$RIGHT_PANE" "cd $CLI_DIR" Enter
tmux send-keys -t "$RIGHT_PANE" "echo \"VB_PORT=$BRIDGE_PORT ready for vcli\"" Enter

echo ""
echo "Tmux session '$SESSION' created."
echo "Run: tmux attach -t $SESSION"
echo ""
echo "Layout:"
echo "  Left pane:  SSH to Docker (run 'virtuoso &' to start Virtuoso)"
echo "  Right pane: Local shell with VB_PORT=$BRIDGE_PORT for vcli commands"
echo ""
echo "Note: Make sure the SSH tunnel for port $BRIDGE_PORT is active:"
echo "  ssh -p $SSH_PORT -i $SSH_KEY $SSH_USER@$SSH_HOST -L $BRIDGE_PORT:127.0.0.1:$BRIDGE_PORT -f -N"
