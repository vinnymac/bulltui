//! Job detail screen: header + tabbed detail (data/opts/progress/error/logs/
//! timeline/flow) with scrolling.

use bullmq::{FlowNode, Job};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Tabs, Wrap,
};
use ratatui::Frame;

use crate::app::{flatten_flow, App, DetailView, HitKind, HitRegion};
use crate::format;
use crate::state::JobTab;
use crate::theme;

/// Returns the scroll bounds of the detail body it drew, which the caller
/// records into [`App::detail_view`] for the next input tick (the same
/// render-records-a-layout-fact pattern as the [`HitRegion`] map).
pub fn draw(frame: &mut Frame, area: Rect, app: &App, hits: &mut Vec<HitRegion>) -> DetailView {
    let rows = Layout::vertical([
        Constraint::Length(2), // header
        Constraint::Length(1), // tabs
        Constraint::Min(1),    // content
    ])
    .split(area);

    let Some(job) = &app.job else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("No job loaded.", theme::muted()))),
            area,
        );
        return DetailView::default();
    };

    draw_header(frame, rows[0], job, app);
    draw_tabs(frame, rows[1], app);
    draw_content(frame, rows[2], job, app, hits)
}

fn draw_header(frame: &mut Frame, area: Rect, job: &Job, app: &App) {
    let state = job.state;
    let mut line1 = vec![
        Span::styled(format!("#{} ", job.id), theme::header()),
        Span::styled(job.name.clone(), theme::title()),
    ];
    if let Some(s) = state {
        line1.push(Span::raw("  "));
        line1.push(Span::styled(
            format!("[{}]", s.label()),
            theme::state_style(s),
        ));
    }
    if job.is_failed() {
        line1.push(Span::raw("  "));
        line1.push(Span::styled("FAILED", theme::danger()));
    }

    let mut line2 = vec![Span::styled(
        format!("attempts {} ", job.attempts_made),
        theme::muted(),
    )];
    if let Some(g) = job.group_id() {
        line2.push(Span::styled(format!(" group:{g} "), theme::muted()));
    }
    if let Some(by) = &job.processed_by {
        line2.push(Span::styled(format!(" by:{by} "), theme::muted()));
    }
    line2.push(Span::styled(
        format!(" added {}", format::relative(job.timestamp, app.now)),
        theme::muted(),
    ));

    frame.render_widget(
        Paragraph::new(Text::from(vec![Line::from(line1), Line::from(line2)])),
        area,
    );
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let tabs = JobTab::all();
    let titles: Vec<Line> = tabs.iter().map(|t| Line::from(t.label())).collect();
    let idx = tabs.iter().position(|t| *t == app.job_tab).unwrap_or(0);
    let widget = Tabs::new(titles)
        .select(idx)
        .highlight_style(theme::selected().fg(theme::ACCENT))
        .divider(Span::styled("│", theme::muted()));
    frame.render_widget(widget, area);
}

fn draw_content(
    frame: &mut Frame,
    area: Rect,
    job: &Job,
    app: &App,
    hits: &mut Vec<HitRegion>,
) -> DetailView {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER))
        .title(Span::styled(
            format!(" {} ", app.job_tab.label()),
            theme::header(),
        ));
    let inner = block.inner(area);
    let view_h = inner.height;

    // The Flow tab is a navigable list, not prose: render it unwrapped (one row
    // per node) so the cursor index maps exactly to a row, and derive the scroll
    // from the cursor so the selection is always on screen.
    if app.job_tab == JobTab::Flow {
        let content_h = app
            .job_flow
            .as_ref()
            .map(|r| flatten_flow(r).len())
            .unwrap_or(0) as u16;
        let offset = flow_scroll(app.flow_selected, view_h);
        let para = Paragraph::new(flow_text(app))
            .block(block)
            .scroll((offset, 0));
        frame.render_widget(para, area);
        render_vscrollbar(frame, area, content_h, offset, view_h);
        hits.push(HitRegion {
            kind: HitKind::FlowNode,
            area: inner,
            offset: offset as usize,
            count: content_h as usize,
        });
        return DetailView {
            max_scroll: content_h.saturating_sub(view_h),
            page: view_h,
        };
    }

    let text = match app.job_tab {
        JobTab::Data => data_text(job),
        JobTab::Options => Text::from(format::pretty_json(&job.opts)),
        JobTab::Progress => progress_text(job),
        JobTab::Error => error_text(job),
        JobTab::Logs => logs_text(app),
        JobTab::Timeline => timeline_text(job, app),
        JobTab::Flow => unreachable!("flow tab handled above"),
    };

    let wrap = Wrap { trim: false };
    // Measure the *wrapped* height at the exact render width so the clamp and
    // scrollbar match what's drawn. Capped to `u16::MAX` so a pathological
    // payload truncates rather than wrapping the offset around.
    let content_h = Paragraph::new(text.clone())
        .wrap(wrap)
        .line_count(inner.width.max(1))
        .min(u16::MAX as usize) as u16;
    let max_scroll = content_h.saturating_sub(view_h);
    let offset = app.detail_scroll.min(max_scroll);
    let para = Paragraph::new(text)
        .block(block)
        .wrap(wrap)
        .scroll((offset, 0));
    frame.render_widget(para, area);
    render_vscrollbar(frame, area, content_h, offset, view_h);
    DetailView {
        max_scroll,
        page: view_h,
    }
}

/// Draw a vertical scrollbar on `area`'s right border when `content_h` exceeds
/// `view_h`. No-op when content fits. Thumb rides the border column, no content
/// width consumed.
fn render_vscrollbar(frame: &mut Frame, area: Rect, content_h: u16, offset: u16, view_h: u16) {
    if content_h <= view_h {
        return;
    }
    let mut state = ScrollbarState::new(content_h as usize)
        .viewport_content_length(view_h as usize)
        .position(offset as usize);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .thumb_symbol("█")
        .track_style(theme::scrollbar_track())
        .thumb_style(theme::scrollbar_thumb());
    // Inset past the top/bottom borders; the bar draws in the right border column.
    frame.render_stateful_widget(
        bar,
        area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        }),
        &mut state,
    );
}

/// Vertical scroll offset that keeps the cursor row visible in a viewport
/// `inner_h` rows tall: 0 until the cursor passes the bottom edge, then enough
/// to pin it to the last visible row.
fn flow_scroll(selected: usize, inner_h: u16) -> u16 {
    let h = inner_h.max(1) as usize;
    let off = if selected >= h { selected - h + 1 } else { 0 };
    off.min(u16::MAX as usize) as u16
}

fn data_text(job: &Job) -> Text<'static> {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled("data:", theme::header())));
    for l in format::pretty_json_str(&job.data).lines() {
        lines.push(Line::from(l.to_string()));
    }
    if let Some(rv) = &job.return_value {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("returnValue:", theme::header())));
        for l in format::pretty_json(rv).lines() {
            lines.push(Line::from(l.to_string()));
        }
    }
    Text::from(lines)
}

fn progress_text(job: &Job) -> Text<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled("progress: ", theme::header()),
        Span::raw(format::progress(&job.progress)),
    ])];
    if let Some(p) = job.progress.as_f64() {
        if (0.0..=100.0).contains(&p) {
            let filled = ((p / 100.0) * 30.0).round() as usize;
            let bar: String = "█".repeat(filled) + &"░".repeat(30 - filled.min(30));
            lines.push(Line::from(Span::styled(
                bar,
                theme::state_style(bullmq::JobState::Active),
            )));
        }
    } else if job.progress.is_object() {
        lines.push(Line::from(""));
        for l in format::pretty_json(&job.progress).lines() {
            lines.push(Line::from(l.to_string()));
        }
    }
    Text::from(lines)
}

fn error_text(job: &Job) -> Text<'static> {
    if !job.is_failed() {
        return Text::from(Line::from(Span::styled("No errors.", theme::muted())));
    }
    let mut lines = Vec::new();
    if let Some(reason) = &job.failed_reason {
        lines.push(Line::from(Span::styled("failedReason:", theme::header())));
        lines.push(Line::from(Span::styled(reason.clone(), theme::danger())));
        lines.push(Line::from(""));
    }
    if !job.stacktrace.is_empty() {
        lines.push(Line::from(Span::styled("stacktrace:", theme::header())));
        for frame in &job.stacktrace {
            for l in frame.lines() {
                lines.push(Line::from(Span::styled(l.to_string(), theme::muted())));
            }
        }
    }
    Text::from(lines)
}

fn logs_text(app: &App) -> Text<'static> {
    if app.job_logs.is_empty() {
        return Text::from(Line::from(Span::styled("No logs.", theme::muted())));
    }
    Text::from(
        app.job_logs
            .iter()
            .map(|l| Line::from(l.clone()))
            .collect::<Vec<_>>(),
    )
}

fn timeline_text(job: &Job, app: &App) -> Text<'static> {
    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<16}"), theme::header()),
            Span::raw(v),
        ])
    };
    let run_at = job.timestamp.map(|t| t + job.delay);
    let mut lines = vec![
        kv("Added at", format::datetime(job.timestamp)),
        kv("Added", format::relative(job.timestamp, app.now)),
    ];
    if job.delay > 0 {
        lines.push(kv("Delay", format::human_duration(job.delay)));
        lines.push(kv("Will run at", format::datetime(run_at)));
    }
    lines.push(kv("Process started", format::datetime(job.processed_on)));
    if let Some(by) = &job.processed_by {
        lines.push(kv("Processed by", by.clone()));
    }
    lines.push(kv("Finished at", format::datetime(job.finished_on)));
    lines.push(kv(
        "Duration",
        format::duration_between(job.processed_on, job.finished_on),
    ));
    Text::from(lines)
}

fn flow_text(app: &App) -> Text<'static> {
    let Some(root) = &app.job_flow else {
        return Text::from(Line::from(Span::styled("No flow data.", theme::muted())));
    };
    // The focused job (`▶`) is identified by (queue, id); the cursor (selection
    // highlight) by its flat index. The two coincide on open, then diverge as
    // the cursor moves.
    let here_id = app.job.as_ref().map(|j| &j.id);
    let here_queue = app.queue_name.as_ref();

    let nodes = crate::app::flatten_flow(root);
    let mut lines: Vec<Line> = Vec::with_capacity(nodes.len() + 1);
    for (i, (depth, node)) in nodes.iter().enumerate() {
        let is_current = here_id == Some(&node.job.id) && here_queue == Some(&node.queue_name);
        let is_cursor = i == app.flow_selected;
        lines.push(flow_line(node, *depth, is_current, is_cursor));
    }
    if nodes.len() <= 1 {
        lines.push(Line::from(Span::styled(
            "(standalone job — no parent/children)",
            theme::muted(),
        )));
    }
    Text::from(lines)
}

/// One row of the flow tree: indent + marker + `[state] queue/id name`. The
/// focused job gets the `▶` marker and title styling; the cursor row gets the
/// selection background patched over every span.
fn flow_line(node: &FlowNode, depth: usize, is_current: bool, is_cursor: bool) -> Line<'static> {
    let indent = "  ".repeat(depth);
    let state_label = node.state.map(|s| s.label()).unwrap_or("?");
    let state_style = node
        .state
        .map(theme::state_style)
        .unwrap_or_else(theme::muted);
    let marker = if is_current { "▶ " } else { "• " };
    let name_style = if is_current {
        theme::title()
    } else {
        Style::default()
    };
    let mut spans = vec![
        Span::raw(indent),
        Span::styled(marker, theme::key_hint()),
        Span::styled(format!("[{state_label}] "), state_style),
        Span::styled(format!("{}/{} ", node.queue_name, node.job.id), name_style),
        Span::styled(node.job.name.clone(), theme::muted()),
    ];
    if is_cursor {
        for s in &mut spans {
            s.style = s.style.patch(theme::selected());
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_scroll_keeps_the_cursor_visible() {
        // Viewport five rows tall: no scroll until the cursor passes the bottom
        // edge, then just enough to pin it to the last visible row.
        assert_eq!(flow_scroll(0, 5), 0);
        assert_eq!(flow_scroll(4, 5), 0, "last visible row needs no scroll");
        assert_eq!(flow_scroll(5, 5), 1, "the 6th row scrolls one line");
        assert_eq!(flow_scroll(9, 5), 5);
        // A degenerate (zero-height) viewport must not panic.
        assert_eq!(flow_scroll(3, 0), 3);
    }
}
