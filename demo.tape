# bulltui demo recording
#
# Prerequisites:
#   just demo           start Valkey and seed demo data
#   vhs demo.tape       record (brew install vhs)
#
# Or in one shot:
#   just record

Output "assets/demo.gif"

Set Shell "bash"
Set Width 1600
Set Height 900
Set FontSize 13
Set FontFamily "Fira Code"
Set Theme "Catppuccin Mocha"
Set Framerate 24
Set PlaybackSpeed 1.25

# ── startup: type at readable speed so viewers can see the command ────────────
Set TypingSpeed 20ms
Type "BULLTUI_REDIS_URL=redis://127.0.0.1:6380 ./target/release/bulltui --queue emails --queue notifications --queue media --queue orchestrator"
Enter
Set TypingSpeed 50ms

# Dot-matrix splash plays (~3.5 s minimum), then overview loads.
Sleep 8s

# ── overview: all queues ──────────────────────────────────────────────────────
# Table view — queue names + per-state job counts
# Alphabetical display order: emails(1) · media(2) · notifications(3) · orchestrator(4)
Sleep 2.5s

# Toggle to stacked-bar view (bull-board style)
Type "v"
Sleep 3s

# Back to table view
Type "v"
Sleep 1.5s

# ── emails queue: completed → failed ─────────────────────────────────────────
# emails is row 1 (already selected) — drill in
Enter
Sleep 1.5s

# Jump to Completed tab:
#   Latest(0)→Active(1)→Waiting(2)→WaitingChildren(3)→Prioritized(4)→Completed(5)
Tab
Sleep 200ms
Tab
Sleep 200ms
Tab
Sleep 200ms
Tab
Sleep 200ms
Tab
Sleep 1s

# Select the first completed job and open it
Down
Sleep 600ms
Enter
Sleep 1.5s

# Browse job detail: Data(default)→Options→Progress→Error→Logs→Timeline
Sleep 1.5s
Tab
Sleep 1.2s
Tab
Sleep 1.2s
Tab
Sleep 1.2s
Tab
Sleep 1.2s
Tab
Sleep 1.8s

# Return to the queue list
Escape
Sleep 700ms

# Switch to Failed tab: Completed(5)→Failed(6)
Tab
Sleep 1s

# Open the first failed job
Down
Sleep 600ms
Enter
Sleep 1.5s

# Navigate to Error tab: Data(0)→Options→Progress→Error(3)
Tab
Sleep 500ms
Tab
Sleep 500ms
Tab
Sleep 2.5s

# Copy error to clipboard via y (OSC-52 / SSH/tmux-safe)
Type "y"
Sleep 1s

# Back to queue, then overview
Escape
Sleep 600ms
Escape
Sleep 1.2s

# ── media queue: live active job ──────────────────────────────────────────────
# Cursor is on emails (row 1) — move down to media (row 2)
Down
Sleep 700ms
Enter
Sleep 1.5s

# Switch to Active tab: Latest(0)→Active(1)
Tab
Sleep 1.2s

# Select and open the active transcode job
Down
Sleep 600ms
Enter
Sleep 1.8s

# Back to queue, then overview
Escape
Sleep 600ms
Escape
Sleep 1.2s

# ── notifications queue: waiting, prioritized, schedulers ─────────────────────
# Cursor is on media (row 2) — move down to notifications (row 3)
Down
Sleep 700ms
Enter
Sleep 1.5s

# Jump to Waiting tab: Latest(0)→Active(1)→Waiting(2)
Tab
Sleep 300ms
Tab
Sleep 1.2s

# Jump to Prioritized tab: Waiting(2)→WaitingChildren(3)→Prioritized(4)
Tab
Sleep 300ms
Tab
Sleep 1.2s

# Open the Schedulers screen (cron + interval schedulers)
Type "S"
Sleep 3s

# Back to queue, then overview
Escape
Sleep 700ms
Escape
Sleep 1.2s

# ── orchestrator queue: flow view ─────────────────────────────────────────────
# Cursor is on notifications (row 3) — move down to orchestrator (row 4)
Down
Sleep 700ms
Enter
Sleep 1.5s

# Latest tab shows the aggregate (waiting-children) parent job
Down
Sleep 600ms
Enter
Sleep 1.5s

# Navigate to Flow tab: Data(0)→Options→Progress→Error→Logs→Timeline→Flow(6)
Tab
Sleep 250ms
Tab
Sleep 250ms
Tab
Sleep 250ms
Tab
Sleep 250ms
Tab
Sleep 250ms
Tab
Sleep 2.5s

# Browse the child nodes in the dependency tree
Down
Sleep 800ms
Down
Sleep 1.2s

# ── command palette ───────────────────────────────────────────────────────────
Escape
Sleep 600ms
Escape
Sleep 1s

Type ":"
Sleep 600ms
Type "email"
Sleep 1.8s
Escape
Sleep 700ms

# ── redis server stats ────────────────────────────────────────────────────────
Type "i"
Sleep 3s
Escape
Sleep 700ms

# ── help overlay ──────────────────────────────────────────────────────────────
Type "?"
Sleep 3.5s
Escape
Sleep 500ms

# ── quit ─────────────────────────────────────────────────────────────────────
Type "q"
