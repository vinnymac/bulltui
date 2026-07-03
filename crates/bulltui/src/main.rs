use bulltui::app::{self, App};
use bulltui::{boot, cli, ui};
use clap::Parser;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    // Headless snapshot connects plainly (no TTY, no splash) and exits.
    if args.snapshot {
        let client = connect_headless(&args).await?;
        return snapshot(client, args).await;
    }

    // Splash preview: hold the splash on screen (no connection) until a keypress.
    if args.splash_preview {
        let mut terminal = ratatui::init();
        let result = boot::preview_splash(&mut terminal).await;
        ratatui::restore();
        return result;
    }

    // Interactive: the terminal starts before the connection. The connection runs
    // behind an animated splash / "connecting" screen (see `boot`), so a slow or
    // TLS broker does not freeze the terminal during the handshake.
    let mut terminal = ratatui::init();
    let result = match boot::splash_and_connect(&mut terminal, &args).await {
        Ok(Some(client)) => app::run(&mut terminal, client, args).await,
        Ok(None) => Ok(()), // user cancelled during connect
        Err(e) => Err(e),   // connection failed
    };
    // `ratatui::restore()` does not touch mouse capture (it never enabled it),
    // so disable it unconditionally here. Idempotent when already off.
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();
    result
}

/// Connect without any UI (for `--snapshot`).
async fn connect_headless(args: &cli::Args) -> anyhow::Result<bullmq::BullClient> {
    bullmq::BullClient::connect_with(
        &args.url,
        args.prefix.clone(),
        bullmq::ConnectOptions {
            insecure: args.insecure,
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("failed to connect to redis at {}: {e}", args.url))
}

/// Render a single overview frame to stdout (headless, for demos/CI).
async fn snapshot(client: bullmq::BullClient, args: cli::Args) -> anyhow::Result<()> {
    let mut app = App::new(client, args);
    app.refresh_overview().await;
    let (w, h) = (120u16, 30u16);
    let mut terminal = Terminal::new(TestBackend::new(w, h))?;
    terminal.draw(|f| ui::draw(f, &mut app))?;
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    print!("{out}");
    Ok(())
}
