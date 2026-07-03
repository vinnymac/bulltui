//! Command-line interface.

use clap::Parser;

pub use crate::clipboard::ClipboardMode;

/// bulltui: a terminal UI for BullMQ with feature parity with bull-board.
#[derive(Debug, Clone, Parser)]
#[command(name = "bulltui", version, about, long_about = None)]
pub struct Args {
    /// Redis/Valkey connection URL. Use `rediss://` for TLS (managed brokers:
    /// ElastiCache, Upstash, Redis Cloud, Azure).
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

    /// Skip TLS certificate verification for `rediss://` connections. Exposes
    /// the connection to MITM; use only on trusted networks with self-signed or
    /// private-CA certs. Errors on a plaintext `redis://` URL.
    #[arg(long)]
    pub insecure: bool,

    /// Clipboard backend for copy (`y`). `auto` uses OSC 52 over SSH (so copy
    /// reaches *your* clipboard) and the OS-native clipboard locally.
    #[arg(long, value_enum, default_value = "auto")]
    pub clipboard: ClipboardMode,

    /// Start with mouse capture disabled. Capture is on by default (click to
    /// select; click again to open; wheel scrolls). While captured, `Shift`/`⌥`
    /// drag does native text selection; `Ctrl+O` drops capture; `y` copies via
    /// OSC 52 regardless of capture state.
    #[arg(long)]
    pub no_mouse: bool,

    /// Skip the `BULLTUI` startup splash. A slow or TLS broker still shows
    /// the connecting screen; a fast local connect goes straight to the queues.
    #[arg(long)]
    pub no_splash: bool,

    /// Preview the startup splash and hold it on screen (no connection).
    /// Press any key to exit.
    #[arg(long)]
    pub splash_preview: bool,

    /// Render a single frame of the overview to stdout and exit (no TTY needed).
    #[arg(long)]
    pub snapshot: bool,
}
