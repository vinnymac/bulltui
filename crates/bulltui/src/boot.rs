//! Animated startup: the terminal is initialised *first*, then the connection
//! runs as a background task while a splash (and, if the broker is slow to
//! answer, a "connecting" screen) plays — so a distant or TLS broker shows life
//! instead of a frozen terminal during the handshake.
//!
//! Flow: every launch gets a brief `BULLTUI` wordmark splash ([`SPLASH_MIN`],
//! skippable, or off entirely with `--no-splash`) that overlaps the connect +
//! first fetch, so it costs almost no added latency. If the connection is still
//! pending once the splash has had its moment, it hands off to an orbiting-comet
//! "connecting" card until the socket answers (or the user cancels). Only then
//! does [`crate::app::run`] take over.
//!
//! Like the rest of the UI the renderers here are pure; the tachyonfx effects
//! are advanced only by this real loop (never in tests), so the splash/connect
//! renderers can still be exercised deterministically with `TestBackend`.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use bullmq::{BullClient, ConnectOptions};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use tachyonfx::{fx, Duration as FxDuration, Effect, Interpolation};

use crate::cli::Args;
use crate::theme;

/// The `BULLTUI` wordmark, pre-rendered from Google's **Doto** (a dot-matrix
/// face) with each lit dot as a `●` glyph — a crisp LED-panel look that suits a
/// TUI and powers on via [`crate::fx::splash_reveal`].
const WORDMARK: &str = include_str!("splash_bulltui.txt");

/// How long the splash lingers before, if still unconnected, handing off to the
/// connecting screen. A fast (local) connect overlaps this entirely, so it's a
/// brand beat rather than dead time. Any key skips it.
const SPLASH_MIN: Duration = Duration::from_millis(3500);

/// How long the wordmark takes to power on. Kept short — a quick warm-up from
/// the dim glow to full cyan — so the living shimmer that follows dominates the
/// hold rather than arriving just as the splash ends.
const REVEAL: (u32, Interpolation) = (700, Interpolation::QuadOut);

/// With `--no-splash`, the grace before the connecting screen appears — long
/// enough that a fast local connect flashes nothing at all, short enough that a
/// stalled broker still shows life quickly.
const GRACE: Duration = Duration::from_millis(150);

/// Braille throbber frames, cycled by elapsed time on the connecting card.
const THROBBER: [char; 8] = ['⣾', '⣽', '⣻', '⢿', '⡿', '⣟', '⣯', '⣷'];

/// The wordmark art as individual rows (trailing blank line dropped).
fn wordmark_rows() -> Vec<&'static str> {
    WORDMARK.lines().collect()
}

/// The throbber glyph for a given elapsed time (≈11 fps spin).
fn throbber_frame(elapsed: Duration) -> char {
    let i = (elapsed.as_millis() / 90) as usize % THROBBER.len();
    THROBBER[i]
}

/// Connect to Redis behind an animated splash. Returns `Ok(Some(client))` once
/// connected, `Ok(None)` if the user cancels (Esc / `q` / Ctrl-C), or `Err` if
/// the connection fails (surfaced immediately, without waiting out the splash).
pub async fn splash_and_connect(
    terminal: &mut DefaultTerminal,
    args: &Args,
) -> Result<Option<BullClient>> {
    // Run the connect off the render loop so frames keep flowing during the
    // (possibly multi-second, bounded) handshake. Owned inputs so the task is
    // 'static.
    let (url, prefix, insecure) = (args.url.clone(), args.prefix.clone(), args.insecure);
    let mut handle = tokio::spawn(async move {
        BullClient::connect_with(&url, prefix, ConnectOptions { insecure }).await
    });

    // A one-shot power-on: the wordmark sweeps in from the void, then holds (a
    // completed effect is a no-op, and the renderer paints the settled colour
    // each frame).
    let mut splash_fx = crate::fx::splash_reveal(REVEAL);
    // Lazily armed when we cross into the connecting phase.
    let mut orbit_fx: Option<Effect> = None;

    let show_splash = !args.no_splash;
    // Once past `hold` an unfinished connection reveals the connecting card: the
    // full splash beat normally, or just a brief grace under `--no-splash`.
    let hold = if show_splash { SPLASH_MIN } else { GRACE };

    let mut events = EventStream::new();
    let start = Instant::now();
    let mut last_frame = Instant::now();
    let mut outcome: Option<Result<BullClient>> = None;
    let mut skipped = false;

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_frame);
        last_frame = now;
        let elapsed = start.elapsed();

        // Exit: a connection error surfaces at once; a success returns straight
        // away unless the splash is mid-beat (then hold so it isn't a flicker).
        if let Some(res) = outcome.take() {
            let holding_splash = show_splash && elapsed < SPLASH_MIN && !skipped;
            if res.is_err() || !holding_splash {
                return res.map(Some);
            }
            outcome = Some(res); // connected, but let the splash finish
        }

        let connecting = outcome.is_none() && (elapsed >= hold || skipped);
        if connecting && orbit_fx.is_none() {
            orbit_fx = Some(fx::repeating(crate::fx::orbit((
                1400,
                Interpolation::Linear,
            ))));
        }

        let fx_dur = FxDuration::from_millis(dt.as_millis().min(u32::MAX as u128) as u32);
        terminal.draw(|frame| {
            if connecting {
                let card = render_connecting(frame, &args.url, elapsed);
                if let Some(e) = orbit_fx.as_mut() {
                    e.process(fx_dur, frame.buffer_mut(), card);
                }
            } else if show_splash {
                let rect = render_splash(frame);
                splash_fx.process(fx_dur, frame.buffer_mut(), rect);
            }
            // else: `--no-splash` grace window — a blank frame until the connect
            // resolves or the connecting card takes over.
        })?;

        tokio::select! {
            res = &mut handle, if outcome.is_none() => {
                outcome = Some(match res {
                    Ok(Ok(client)) => Ok(client),
                    Ok(Err(e)) => Err(anyhow::Error::new(e)
                        .context(format!("failed to connect to redis at {}", args.url))),
                    Err(join) => Err(anyhow!("connection task failed: {join}")),
                });
            }
            _ = tokio::time::sleep(Duration::from_millis(33)) => {}
            maybe = events.next() => {
                if let Some(Ok(Event::Key(key))) = maybe {
                    if key.kind == KeyEventKind::Press {
                        let ctrl_c = key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        if ctrl_c || matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                            handle.abort();
                            return Ok(None);
                        }
                        skipped = true; // any other key fast-forwards the splash
                    }
                }
            }
        }
    }
}

/// Play the splash and then **hold it on screen** until any key is pressed —
/// no connection, no timeout, no hand-off. A dev aid for eyeballing the splash
/// (`--splash-preview`).
pub async fn preview_splash(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut splash_fx = crate::fx::splash_reveal(REVEAL);
    let mut events = EventStream::new();
    let mut last_frame = Instant::now();
    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_frame);
        last_frame = now;
        let fx_dur = FxDuration::from_millis(dt.as_millis().min(u32::MAX as u128) as u32);
        terminal.draw(|frame| {
            let rect = render_splash(frame);
            splash_fx.process(fx_dur, frame.buffer_mut(), rect);
        })?;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(33)) => {}
            maybe = events.next() => {
                if let Some(Ok(Event::Key(key))) = maybe {
                    if key.kind == KeyEventKind::Press {
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Draw the centered `BULLTUI` wordmark + tagline; returns the rect it occupies
/// so the caller can fade exactly that region. Pure.
pub fn render_splash(frame: &mut Frame) -> Rect {
    let area = frame.area();
    let rows = wordmark_rows();
    let art_w = rows.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let art_h = rows.len() as u16;

    let tagline = "a terminal UI for BullMQ";
    let block_w = art_w.max(tagline.chars().count() as u16);
    let block = crate::ui::centered_sized(block_w, art_h + 2, area);

    let [art_area, _gap, tag_area] = Layout::vertical([
        Constraint::Length(art_h),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(block);

    // The wordmark keeps a shared left origin (each row is left-aligned), so the
    // letters can't shear; the block itself is what's centered on screen.
    let art_lines: Vec<Line> = rows
        .iter()
        .map(|l| Line::from(Span::styled(*l, Style::default().fg(theme::SPLASH_DOT))))
        .collect();
    let art_rect = Rect {
        x: art_area.x + art_area.width.saturating_sub(art_w) / 2,
        y: art_area.y,
        width: art_w,
        height: art_h,
    };
    frame.render_widget(Paragraph::new(art_lines), art_rect);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(tagline, theme::muted())).centered()),
        tag_area,
    );
    block
}

/// Draw the "connecting" card (amber LED + orbiting-comet border + throbber +
/// status). Returns the card rect so the caller can trace the orbit over its
/// border ring. Pure.
pub fn render_connecting(frame: &mut Frame, url: &str, elapsed: Duration) -> Rect {
    let area = frame.area();
    let tls = url.starts_with("rediss://");
    let message = if tls {
        "Establishing secure connection…"
    } else {
        "Connecting…"
    };

    // Size the card to hold the widest line, clamped to the viewport.
    let widest = message.chars().count().max(url.chars().count()) + 8;
    let card_w = (widest as u16).clamp(40, area.width.saturating_sub(4).max(40));
    let card = crate::ui::centered_sized(card_w, 9, area);

    // Title: amber LED (the "connecting" state, before it settles green) +
    // wordmark chip, mirroring the running app's header.
    let title = Line::from(vec![
        Span::styled(" ● ", Style::default().fg(theme::CONNECTING)),
        Span::styled(
            " bulltui ",
            theme::title()
                .bg(theme::ACCENT)
                .fg(ratatui::style::Color::Black),
        ),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_FOCUS))
        .title(title);
    let inner = block.inner(card);
    frame.render_widget(block, card);
    if inner.height == 0 {
        return card;
    }

    let throbber = throbber_frame(elapsed);
    let secs = elapsed.as_secs();
    let dim = theme::muted().add_modifier(Modifier::DIM);
    let body = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("{throbber}  "),
                Style::default().fg(theme::CONNECTING),
            ),
            Span::styled(message, theme::header()),
        ])
        .centered(),
        Line::from(""),
        Line::from(Span::styled(url.to_string(), theme::muted())).centered(),
        Line::from(Span::styled(format!("{secs}s · Esc to cancel"), dim)).centered(),
    ];
    frame.render_widget(Paragraph::new(body), inner);
    card
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn buffer_text(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn wordmark_art_is_present_and_rectangular() {
        let rows = wordmark_rows();
        assert!(rows.len() >= 5, "wordmark has several rows");
        assert!(
            rows.iter().any(|l| l.chars().count() > 20),
            "wordmark is a wide banner"
        );
    }

    #[test]
    fn throbber_cycles_through_every_frame() {
        let seen: std::collections::HashSet<char> = (0..8)
            .map(|i| throbber_frame(Duration::from_millis(i * 90)))
            .collect();
        assert_eq!(seen.len(), THROBBER.len(), "each 90ms step is a new frame");
    }

    #[test]
    fn splash_renders_the_tagline() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            render_splash(f);
        })
        .unwrap();
        assert!(buffer_text(&term).contains("a terminal UI for BullMQ"));
    }

    #[test]
    fn connecting_card_names_a_secure_handshake_for_rediss() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            render_connecting(f, "rediss://cache.example.com:6380", Duration::from_secs(3));
        })
        .unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("Establishing secure connection"));
        assert!(text.contains("Esc to cancel"));
    }

    #[test]
    fn splash_warms_up_without_black_and_keeps_animating() {
        use ratatui::style::Color;
        use tachyonfx::Duration as FxDuration;

        let mut fx = crate::fx::splash_reveal((800u32, tachyonfx::Interpolation::QuadOut));
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        let step = |term: &mut Terminal<TestBackend>, fx: &mut tachyonfx::Effect| {
            term.draw(|f| {
                let rect = render_splash(f);
                fx.process(FxDuration::from_millis(33), f.buffer_mut(), rect);
            })
            .unwrap();
        };

        // ~200ms into the warm-up, sample a lit dot's colour.
        for _ in 0..6 {
            step(&mut term, &mut fx);
        }
        let buf = term.backend().buffer();
        let dot = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|(x, y)| &buf[(x, y)])
            .find(|c| c.symbol() == "●")
            .expect("a lit dot was drawn");
        match dot.fg {
            // A teal/cyan warm-up has real green+blue — never a harsh near-black.
            Color::Rgb(_, g, b) => assert!(
                g as u16 + b as u16 > 80,
                "dots warm up in teal/cyan, not from black: {:?}",
                dot.fg
            ),
            other => panic!("expected an RGB dot colour, got {other:?}"),
        }

        // Well past the warm-up: the breathing shimmer must keep it alive.
        for _ in 0..80 {
            step(&mut term, &mut fx);
        }
        assert!(
            fx.running(),
            "the splash keeps animating (breathing) instead of freezing on a static frame"
        );
    }

    #[test]
    fn connecting_card_is_plain_for_non_tls() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            render_connecting(f, "redis://127.0.0.1:6379", Duration::from_secs(1));
        })
        .unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("Connecting…"));
        assert!(!text.contains("secure"));
    }
}
