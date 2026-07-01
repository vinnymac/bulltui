//! Command-line interface.

use clap::Parser;

pub use crate::clipboard::ClipboardMode;

/// bulltui — a terminal UI for BullMQ (feature parity with bull-board).
#[derive(Debug, Clone, Parser)]
#[command(name = "bulltui", version, about, long_about = None)]
pub struct Args {
    /// Redis/Valkey connection URL. Use `rediss://` to connect over TLS (for
    /// managed brokers — ElastiCache, Upstash, Redis Cloud, Azure, …).
    #[arg(
        short,
        long,
        env = "BULLTUI_REDIS_URL",
        default_value = "redis://127.0.0.1:6379"
    )]
    pub url: String,

    /// BullMQ key prefix.
    #[arg(short, long, env = "BULLTUI_PREFIX", default_value = "bull")]
    pub prefix: String,

    /// Restrict to these queues (repeatable). If omitted, queues are
    /// auto-discovered by scanning for `{prefix}:*:meta` keys.
    #[arg(short, long = "queue")]
    pub queues: Vec<String>,

    /// Auto-refresh interval in seconds (0 disables auto-refresh).
    #[arg(long, default_value_t = 5)]
    pub poll: u64,

    /// Number of jobs per page in the queue view.
    #[arg(long, default_value_t = 10)]
    pub jobs_per_page: usize,

    /// Read-only mode: disable all write/admin operations.
    #[arg(long)]
    pub read_only: bool,

    /// Skip confirmation prompts for destructive actions.
    #[arg(long)]
    pub no_confirm: bool,

    /// Skip TLS certificate verification for `rediss://` connections. Insecure
    /// (exposes the connection to man-in-the-middle) — only for self-signed or
    /// private-CA brokers on a trusted network. Errors on a plaintext `redis://`
    /// URL rather than silently doing nothing.
    #[arg(long)]
    pub insecure: bool,

    /// Clipboard backend for copy (`y`). `auto` uses OSC 52 over SSH (so copy
    /// reaches *your* clipboard) and the OS-native clipboard locally.
    #[arg(long, value_enum, default_value = "auto")]
    pub clipboard: ClipboardMode,

    /// Disable mouse navigation at launch. Mouse capture is **on by default**
    /// (the prevailing TUI posture — htop, lazygit, zellij, neovim): click a row
    /// to select, click it again to open; the wheel scrolls. While captured, hold
    /// `Shift` (or `⌥`/Option on macOS terminals) and drag to select text
    /// natively, or press `Ctrl+O` to drop capture entirely; `y` copies the
    /// focused pane straight to your clipboard (OSC 52 over SSH/tmux) regardless.
    /// Pass `--no-mouse` to start with capture off. The keyboard always works.
    #[arg(long)]
    pub no_mouse: bool,

    /// Skip the `BULLTUI` startup splash. A slow/TLS broker still shows a
    /// "connecting" screen (so the terminal never just freezes), but a fast
    /// local connect goes straight to the queues with no brand beat.
    #[arg(long)]
    pub no_splash: bool,

    /// Preview the startup splash and hold it on screen (no connection). Powers
    /// the wordmark on, then waits — press any key to exit. Handy for tuning it.
    #[arg(long)]
    pub splash_preview: bool,

    /// Render a single frame of the overview to stdout and exit (no TTY needed).
    #[arg(long)]
    pub snapshot: bool,
}
