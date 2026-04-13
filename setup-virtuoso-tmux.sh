#!/bin/bash
# setup-virtuoso-tmux.sh
# Creates a tmux session for Virtuoso TUI workflow
# Left pane: SSH to Docker with X11 forwarding for Virtuoso
# Right pane: Local vcli commands (port 36539 already tunneled)

SESSION="virtuoso-tui"
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o BatchMode=yes"
SSH_KEY="$HOME/.ssh/id_rsa"

# Kill existing session
tmux kill-session -t $SESSION 2>/dev/null

# Create new session with larger dimensions
# tmux starts windows at index 1, not 0
tmux new-session -d -s $SESSION -x 180 -y 45

# Split into left/right panes (window index is 1)
tmux split-window -h -t $SESSION:1

# Left pane (pane index 1): SSH to container with X11 forwarding
tmux send-keys -t $SESSION:1.1 "ssh -X -p 2222 -i $SSH_KEY $SSH_OPTS user@localhost" Enter
sleep 3

# Source cadence environment
tmux send-keys -t $SESSION:1.1 'source /opt/cadence_env.sh' Enter
sleep 1
tmux send-keys -t $SESSION:1.1 'export DISPLAY=localhost:11.0' Enter
sleep 1

# Right pane (pane index 2): Local shell for vcli
tmux send-keys -t $SESSION:1.2 'export VB_PORT=36539' Enter
tmux send-keys -t $SESSION:1.2 'export VB_TIMEOUT=60' Enter
tmux send-keys -t $SESSION:1.2 "cd $HOME/git/virtuoso-cli" Enter
tmux send-keys -t $SESSION:1.2 'echo "VB_PORT=$VB_PORT ready for vcli"' Enter

echo ""
echo "Tmux session '$SESSION' created."
echo "Run: tmux attach -t $SESSION"
echo ""
echo "Layout:"
echo "  Left pane:  SSH to Docker (run 'virtuoso &' to start Virtuoso)"
echo "  Right pane: Local shell with VB_PORT=36539 for vcli commands"
echo ""
echo "Note: Make sure SSH tunnel for port 36539 is active:"
echo "  ssh -p 2222 -i ~/.ssh/id_rsa user@localhost -L 36539:127.0.0.1:36539 -f -N"
