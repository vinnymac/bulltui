//! The live events feed: a scrolling, colour-by-kind tail of the queue event
//! streams, with follow/pause, filtering and a hidden-count indicator.

use bullmq::JobState;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::format;
use crate::theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    draw_status(frame, rows[0], app);
    draw_list(frame, rows[1], app);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("scope:{} ", app.events_scope.label()),
            theme::muted(),
        ),
    ];
    if app.events_paused {
        spans.push(Span::styled(" ⏸ PAUSED ", theme::danger()));
    } else if app.events_follow {
        spans.push(Span::styled(
            " ● FOLLOW ",
            theme::state_style(JobState::Completed),
        ));
    } else {
        spans.push(Span::styled(" ○ scroll ", theme::muted()));
    }
    if let Some(f) = &app.events_filter {
        spans.push(Span::styled(format!(" /{f} "), theme::key_hint()));
    }
    let hidden = app.hidden_event_count();
    if hidden > 0 {
        spans.push(Span::styled(format!(" {hidden} hidden "), theme::muted()));
    }
    spans.push(Span::styled(
        format!(" {} total ", app.events_total),
        theme::muted(),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_FOCUS))
        .title(Span::styled(" Events ", theme::header()))
        .title_bottom(
            Line::from(Span::styled(
                " f follow · p pause · / filter · s scope · n next-fail · ⏎ job ",
                theme::muted(),
            ))
            .right_aligned(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let filtered = app.filtered_events();
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Waiting for events…",
                theme::muted(),
            ))),
            inner,
        );
        return;
    }

    let height = inner.height as usize;
    let n = filtered.len();
    let sel = app.events_selected.min(n - 1);
    let start = if sel >= height { sel + 1 - height } else { 0 };

    let mut lines = Vec::new();
    for (i, ev) in filtered.iter().enumerate().skip(start).take(height) {
        let selected = i == sel;
        let marker = if selected { "▌" } else { " " };
        let mut spans = vec![
            Span::styled(format!("{marker} "), theme::key_hint()),
            Span::styled(format!("{} ", format::time_only(ev.ts)), theme::muted()),
            Span::styled(format!("{:<14} ", ev.queue), theme::muted()),
            Span::styled(
                format!("{:<16} ", ev.kind.label()),
                Style::default().fg(theme::event_color(ev.kind)),
            ),
        ];
        if let Some(id) = &ev.job_id {
            spans.push(Span::styled(format!("job {id} "), theme::muted()));
        }
        let summary = format::truncate(&format::one_line(&ev.summary()), 48);
        spans.push(Span::raw(summary));

        let row_style = if selected {
            theme::selected()
        } else {
            Style::default()
        };
        lines.push(Line::from(spans).style(row_style));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}
