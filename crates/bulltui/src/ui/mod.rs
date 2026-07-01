//! Rendering: the draw dispatcher, shared chrome (title/status bars) and the
//! per-screen and overlay renderers.

mod events;
mod job;
mod overlay;
mod overview;
mod queue;
mod schedulers;
mod workers;

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::{App, Screen};
use crate::state::Overlay;
use crate::theme;

/// The three horizontal bands of the main screen: the header box, the body
/// stage, and the status line. Shared by [`draw`] and the animation pass
/// ([`crate::fx::Animations::process`]) so effects target the same regions the
/// renderer drew into, even across resizes.
pub fn regions(area: Rect) -> [Rect; 3] {
    let chunks = Layout::vertical([
        Constraint::Length(3), // header box (bordered)
        Constraint::Min(1),    // body
        Constraint::Length(1), // status
    ])
    .split(area);
    [chunks[0], chunks[1], chunks[2]]
}

/// Top-level draw entrypoint.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, status] = regions(frame.area());

    draw_header(frame, header, app);
    // Each body renderer records the geometry of the rows it drew into `hits`,
    // a pure function of state + layout; `App::on_mouse` consults the result to
    // map a click back to the row the keyboard would act on.
    let mut hits: Vec<crate::app::HitRegion> = Vec::new();
    match app.screen {
        Screen::Overview => overview::draw(frame, body, app, &mut hits),
        Screen::Queue => queue::draw(frame, body, app, &mut hits),
        // The job renderer also reports the detail body's scroll bounds, recorded
        // for the next input tick (like the hit map) so scrolling stays clamped.
        Screen::Job => app.detail_view = job::draw(frame, body, app, &mut hits),
        Screen::Schedulers => schedulers::draw(frame, body, app, &mut hits),
        Screen::Workers => workers::draw(frame, body, app, &mut hits),
        Screen::Events => events::draw(frame, body, app, &mut hits),
    }
    app.mouse_regions = hits;
    draw_status(frame, status, app);

    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => overlay::draw_help(frame, app),
        Overlay::Confirm(_) => overlay::draw_confirm(frame, app),
        Overlay::Input(_) => overlay::draw_input(frame, app),
        Overlay::RedisStats => overlay::draw_redis_stats(frame, app),
        Overlay::Metrics => overlay::draw_metrics(frame, app),
        Overlay::Settings => overlay::draw_settings(frame, app),
        Overlay::Palette(_) => overlay::draw_palette(frame, app),
        Overlay::Filter(_) => overlay::draw_filter(frame, app),
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    // The header is its own bordered box so the chrome stands apart from the
    // content stage below it. The wordmark and the connection LED live in the
    // box's top-border title; the breadcrumb and poll cadence sit on the inner
    // row.
    //
    // Connection LED: a filled green dot (pulsed by the animation pass while
    // connected) or a hollow red dot when Redis is unreachable. The dot is the
    // *only* cell coloured `theme::LIVE`, which is how the heartbeat targets it
    // and nothing else. Just colour + a dot — no text.
    let (dot, dot_color) = if app.connected {
        ("●", theme::LIVE)
    } else {
        ("○", theme::DANGER)
    };
    let title = Line::from(vec![
        // A plain (no-background) space sits between the LED and the wordmark
        // chip so the dot doesn't butt up against the chip's coloured block.
        Span::styled(format!(" {dot} "), Style::default().fg(dot_color)),
        Span::styled(
            " bulltui ",
            theme::title().bg(theme::ACCENT).fg(Color::Black),
        ),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_FOCUS))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    // Breadcrumb (left).
    let mut crumbs = vec![Span::raw(" ")];
    match app.screen {
        Screen::Overview => crumbs.push(Span::styled("Queues", theme::header())),
        Screen::Queue => {
            crumbs.push(Span::styled("Queues", theme::muted()));
            crumbs.push(Span::raw(" › "));
            crumbs.push(Span::styled(
                app.queue_name.clone().unwrap_or_default(),
                theme::header(),
            ));
        }
        Screen::Job => {
            crumbs.push(Span::styled("Queues", theme::muted()));
            crumbs.push(Span::raw(" › "));
            crumbs.push(Span::styled(
                app.queue_name.clone().unwrap_or_default(),
                theme::muted(),
            ));
            crumbs.push(Span::raw(" › "));
            let id = app.job.as_ref().map(|j| j.id.clone()).unwrap_or_default();
            crumbs.push(Span::styled(format!("job {id}"), theme::header()));
        }
        Screen::Schedulers => {
            crumbs.push(Span::styled("Queues", theme::muted()));
            crumbs.push(Span::raw(" › "));
            crumbs.push(Span::styled(
                app.queue_name.clone().unwrap_or_default(),
                theme::muted(),
            ));
            crumbs.push(Span::raw(" › "));
            crumbs.push(Span::styled("schedulers", theme::header()));
        }
        Screen::Workers => {
            crumbs.push(Span::styled("Queues", theme::muted()));
            crumbs.push(Span::raw(" › "));
            crumbs.push(Span::styled("workers", theme::header()));
        }
        Screen::Events => {
            crumbs.push(Span::styled("Queues", theme::muted()));
            crumbs.push(Span::raw(" › "));
            crumbs.push(Span::styled("events", theme::header()));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(crumbs)), inner);

    // Read-only badge + poll cadence (right).
    let mut right = Vec::new();
    if app.read_only() {
        right.push(Span::styled("[read-only]", theme::muted().fg(theme::WARN)));
        right.push(Span::raw("  "));
    }
    // Mouse capture is on by default, which suspends the terminal's native
    // text selection — so surface the escape hatch right here (best-practice
    // discoverable hint) rather than leaving it buried in `?` help. When capture
    // is off we flag that instead, since off is the non-default mode: the switch
    // is never silent in either direction.
    if app.mouse_capture {
        right.push(Span::styled("⇧/⌥-drag: select", theme::muted()));
    } else {
        right.push(Span::styled("mouse:off", theme::muted().fg(theme::WARN)));
    }
    right.push(Span::raw("  "));
    let poll = if app.settings.poll_secs == 0 {
        "poll:off".to_string()
    } else {
        format!("poll:{}s", app.settings.poll_secs)
    };
    right.push(Span::styled(poll, theme::muted()));
    right.push(Span::raw(" "));
    frame.render_widget(Paragraph::new(Line::from(right)).right_aligned(), inner);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    // Hints come from the single keybinding registry so they can't drift from
    // the `?` help overlay (see `crate::keymap`).
    let mut spans = vec![Span::raw(" ")];
    for hint in crate::keymap::status_hints(app.screen) {
        spans.push(Span::styled(hint.keys, theme::key_hint()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(hint.label, theme::muted()));
        spans.push(Span::raw("  "));
    }
    // Right side: status / error message.
    let msg = if let Some(err) = &app.last_error {
        Span::styled(format!("⚠ {err}"), theme::danger())
    } else {
        Span::styled(app.status.clone(), theme::muted())
    };
    let left = Line::from(spans);
    let right = Line::from(vec![msg]).right_aligned();
    frame.render_widget(Paragraph::new(left), area);
    frame.render_widget(Paragraph::new(right), area);
}

// -- shared helpers --------------------------------------------------------

/// A centered rectangle `pct_x` × `pct_y` percent of `area`.
pub fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Percentage(pct_y)]).flex(Flex::Center);
    let horizontal = Layout::horizontal([Constraint::Percentage(pct_x)]).flex(Flex::Center);
    let [area] = vertical.areas(area);
    let [area] = horizontal.areas(area);
    area
}

/// A centered rectangle of a fixed size (clamped to `area`).
pub fn centered_sized(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let vertical = Layout::vertical([Constraint::Length(h)]).flex(Flex::Center);
    let horizontal = Layout::horizontal([Constraint::Length(w)]).flex(Flex::Center);
    let [area] = vertical.areas(area);
    let [area] = horizontal.areas(area);
    area
}

/// Clear and draw a bordered modal block, returning the inner content area.
pub fn modal_block(frame: &mut Frame, area: Rect, title: &str, danger: bool) -> Rect {
    frame.render_widget(Clear, area);
    let border = if danger {
        theme::DANGER
    } else {
        theme::BORDER_FOCUS
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(ratatui::style::Style::default().fg(border))
        .title(Span::styled(format!(" {title} "), theme::header()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}
