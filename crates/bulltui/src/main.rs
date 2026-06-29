use bulltui::app::{self, App};
use bulltui::{cli, ui};
use clap::Parser;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    let client = bullmq::BullClient::connect(&args.url, args.prefix.clone())
        .await
        .map_err(|e| anyhow::anyhow!("failed to connect to redis at {}: {e}", args.url))?;

    if args.snapshot {
        return snapshot(client, args).await;
    }

    let mut terminal = ratatui::init();
    let result = app::run(&mut terminal, client, args).await;
    ratatui::restore();
    result
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
