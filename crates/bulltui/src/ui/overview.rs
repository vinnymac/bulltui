//! Overview screen: all queues with per-state counts.
//!
//! Two interchangeable presentations (toggle with `v`):
//! - [`OverviewView::Table`]: numeric columns, one per state.
//! - [`OverviewView::Bars`]: a bull-board-style stacked, color-segmented bar
//!   per queue, with an on-screen legend and counts drawn inside each segment.

use bullmq::{JobCounts, JobState, QueueSummary};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::app::{list_offset, App, HitKind, HitRegion, OverviewView};
use crate::theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, hits: &mut Vec<HitRegion>) {
    let queues = app.visible_queues();

    let mut title = format!(" Queues ({}) ", queues.len());
    if let Some(s) = &app.overview_search {
        title = format!(" Queues — search:\"{s}\" ({}) ", queues.len());
    }
    if let Some(state) = app.overview_status_filter {
        title = format!(" Queues — has:{} ({}) ", state.status_str(), queues.len());
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_FOCUS))
        .title(Span::styled(title, theme::header()))
        .title_bottom(Line::from(Span::styled(
            format!(
                " sort:{} · view:{} · [v] toggle view ",
                app.overview_sort.label(),
                app.overview_view.label(),
            ),
            theme::muted(),
        )));

    if queues.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            "No queues found. Is Redis seeded? Press 'r' to refresh.",
            theme::muted(),
        )))
        .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    match app.overview_view {
        OverviewView::Table => draw_table(frame, inner, &queues, app, hits),
        OverviewView::Bars => draw_bars(frame, inner, &queues, app, hits),
    }
}

// -- table view ------------------------------------------------------------

fn count_cell(state: JobState, n: i64) -> Cell<'static> {
    if n > 0 {
        Cell::from(n.to_string()).style(theme::state_style(state))
    } else {
        Cell::from("·").style(theme::muted())
    }
}

fn draw_table(
    frame: &mut Frame,
    area: Rect,
    queues: &[&QueueSummary],
    app: &App,
    hits: &mut Vec<HitRegion>,
) {
    // Full, readable headers (no cryptic abbreviations); widths are sized to
    // the header words since the counts themselves are short.
    let header = Row::new(vec![
        Cell::from("Queue").style(theme::header()),
        Cell::from("Active").style(theme::state_style(JobState::Active)),
        Cell::from("Waiting").style(theme::state_style(JobState::Waiting)),
        Cell::from("Prioritized").style(theme::state_style(JobState::Prioritized)),
        Cell::from("Completed").style(theme::state_style(JobState::Completed)),
        Cell::from("Failed").style(theme::state_style(JobState::Failed)),
        Cell::from("Delayed").style(theme::state_style(JobState::Delayed)),
        Cell::from("Children").style(theme::state_style(JobState::WaitingChildren)),
        Cell::from("Paused").style(theme::state_style(JobState::Paused)),
        Cell::from("Total").style(theme::header()),
        Cell::from("Concurrency").style(theme::muted()),
    ]);

    let rows: Vec<Row> = queues
        .iter()
        .map(|q: &&QueueSummary| {
            let name = if q.is_paused {
                Line::from(vec![
                    Span::styled("⏸ ", theme::state_style(JobState::Paused)),
                    Span::raw(q.name.clone()),
                ])
            } else {
                Line::from(format!("  {}", q.name))
            };
            let conc = q
                .global_concurrency
                .map(|c| c.to_string())
                .unwrap_or_else(|| "—".to_string());
            Row::new(vec![
                Cell::from(name),
                count_cell(JobState::Active, q.counts.active),
                count_cell(JobState::Waiting, q.counts.waiting),
                count_cell(JobState::Prioritized, q.counts.prioritized),
                count_cell(JobState::Completed, q.counts.completed),
                count_cell(JobState::Failed, q.counts.failed),
                count_cell(JobState::Delayed, q.counts.delayed),
                count_cell(JobState::WaitingChildren, q.counts.waiting_children),
                count_cell(JobState::Paused, q.counts.paused),
                Cell::from(q.total_jobs().to_string()).style(theme::header()),
                Cell::from(conc).style(theme::muted()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(16),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(11),
        Constraint::Length(9),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(11),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(theme::selected())
        .highlight_symbol("▌");

    // Data rows sit one row below the header; window them so the cursor stays
    // visible, and record that geometry for click hit-testing.
    let sel = app.overview_selected.min(queues.len() - 1);
    let rows_h = area.height.saturating_sub(1);
    let offset = list_offset(sel, rows_h as usize, queues.len());
    let mut state = TableState::default().with_offset(offset);
    state.select(Some(sel));
    frame.render_stateful_widget(table, area, &mut state);
    hits.push(HitRegion {
        kind: HitKind::OverviewQueue,
        area: Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: rows_h,
        },
        offset,
        count: queues.len(),
    });
}

// -- bar view --------------------------------------------------------------

const NAME_W: u16 = 22;
const TOTAL_W: u16 = 7;

fn draw_bars(
    frame: &mut Frame,
    area: Rect,
    queues: &[&QueueSummary],
    app: &App,
    hits: &mut Vec<HitRegion>,
) {
    // Legend (2 rows, wrapping) + the list of queue bars.
    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(area);
    draw_legend(frame, rows[0]);

    let list = rows[1];
    if list.height == 0 {
        return;
    }
    let rows_h = list.height as usize;
    let total = queues.len();
    let sel = app.overview_selected.min(total.saturating_sub(1));
    // Window the list so the selected row stays visible.
    let offset = list_offset(sel, rows_h, total);
    let end = (offset + rows_h).min(total);

    for (slot, idx) in (offset..end).enumerate() {
        let q = queues[idx];
        let row_rect = Rect {
            x: list.x,
            y: list.y + slot as u16,
            width: list.width,
            height: 1,
        };
        draw_bar_row(frame, row_rect, q, idx == sel);
    }
    hits.push(HitRegion {
        kind: HitKind::OverviewQueue,
        area: list,
        offset,
        count: total,
    });
}

fn draw_legend(frame: &mut Frame, area: Rect) {
    let mut spans = Vec::new();
    for state in JobState::ALL {
        spans.push(Span::styled(
            "██",
            Style::default().fg(theme::state_color(state)),
        ));
        spans.push(Span::styled(
            format!(" {}  ", state.label()),
            theme::muted(),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
}

fn draw_bar_row(frame: &mut Frame, area: Rect, q: &QueueSummary, selected: bool) {
    let cols = Layout::horizontal([
        Constraint::Length(1),       // selection marker
        Constraint::Length(NAME_W),  // name
        Constraint::Length(1),       // gap
        Constraint::Min(1),          // bar
        Constraint::Length(TOTAL_W), // total
    ])
    .split(area);

    // Marker.
    let marker = if selected { "▌" } else { " " };
    frame.render_widget(
        Paragraph::new(Span::styled(marker, theme::key_hint())),
        cols[0],
    );

    // Name (+ paused indicator).
    let mut name_style = if selected {
        theme::title()
    } else {
        Style::default()
    };
    let icon = if q.is_paused {
        name_style = name_style.fg(theme::state_color(JobState::Paused));
        "⏸ "
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{icon}{}", q.name),
            name_style,
        ))),
        cols[1],
    );

    // Bar.
    frame.render_widget(
        Paragraph::new(Line::from(bar_spans(&q.counts, cols[3].width as usize))),
        cols[3],
    );

    // Total, right-aligned.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            q.total_jobs().to_string(),
            theme::header(),
        )))
        .right_aligned(),
        cols[4],
    );
}

/// Build the colored, stacked segments for one queue's counts, filling exactly
/// `width` columns. Each segment is a run of cells in the state's color with
/// the count centered inside (when it fits).
fn bar_spans(counts: &JobCounts, width: usize) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let parts: Vec<(JobState, i64)> = JobState::ALL
        .iter()
        .map(|s| (*s, counts.get(*s)))
        .filter(|(_, n)| *n > 0)
        .collect();
    let total: i64 = parts.iter().map(|(_, n)| *n).sum();
    if total == 0 {
        return vec![Span::styled("·".repeat(width), theme::muted())];
    }

    let seg_w = segment_widths(&parts, total, width);
    let mut spans = Vec::with_capacity(parts.len());
    for ((state, n), w) in parts.iter().zip(seg_w) {
        if w == 0 {
            continue;
        }
        let label = n.to_string();
        let content = if w >= label.len() {
            let pad = w - label.len();
            let left = pad / 2;
            format!("{}{}{}", " ".repeat(left), label, " ".repeat(pad - left))
        } else {
            " ".repeat(w)
        };
        let fill = theme::state_color(*state);
        spans.push(Span::styled(
            content,
            Style::default()
                .bg(fill)
                .fg(theme::contrast_text(fill))
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans
}

/// Apportion `width` columns across `parts` proportionally to their counts,
/// using largest-remainder with a min-1 guarantee for every nonzero segment
/// when there's room. The returned widths sum to exactly `width`.
fn segment_widths(parts: &[(JobState, i64)], total: i64, width: usize) -> Vec<usize> {
    let n = parts.len();
    if n == 0 || width == 0 {
        return vec![0; n];
    }
    let w = width as f64;
    let t = total as f64;
    let exact: Vec<f64> = parts.iter().map(|(_, c)| (*c as f64 / t) * w).collect();
    let mut base: Vec<usize> = exact.iter().map(|e| e.floor() as usize).collect();

    // Guarantee a visible sliver for every nonzero segment, if it fits.
    if width >= n {
        for b in base.iter_mut() {
            if *b == 0 {
                *b = 1;
            }
        }
    }

    let used: usize = base.iter().sum();
    if used < width {
        // Hand out the remainder to the largest fractional parts.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            let fa = exact[a] - exact[a].floor();
            let fb = exact[b] - exact[b].floor();
            fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut extra = width - used;
        let mut k = 0;
        while extra > 0 {
            base[order[k % n]] += 1;
            extra -= 1;
            k += 1;
        }
    } else if used > width {
        // Over-allocated by the min-1 bumps; trim from the widest segments.
        let mut over = used - width;
        while over > 0 {
            let mut mi = None;
            let mut mv = 1usize;
            for (i, b) in base.iter().enumerate() {
                if *b > mv {
                    mv = *b;
                    mi = Some(i);
                }
            }
            match mi {
                Some(i) => {
                    base[i] -= 1;
                    over -= 1;
                }
                None => break,
            }
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(widths: &[(JobState, i64)]) -> (Vec<(JobState, i64)>, i64) {
        let parts: Vec<(JobState, i64)> = widths.to_vec();
        let total = parts.iter().map(|(_, n)| *n).sum();
        (parts, total)
    }

    #[test]
    fn segments_sum_to_width() {
        let (parts, total) = states(&[
            (JobState::Completed, 5),
            (JobState::Failed, 3),
            (JobState::Waiting, 2),
        ]);
        for width in [1usize, 3, 7, 10, 40, 100] {
            let seg = segment_widths(&parts, total, width);
            assert_eq!(seg.iter().sum::<usize>(), width, "width={width}");
        }
    }

    #[test]
    fn nonzero_segments_visible_when_room() {
        // 999 vs 1: the tiny segment still gets at least one column at width 10.
        let (parts, total) = states(&[(JobState::Completed, 999), (JobState::Failed, 1)]);
        let seg = segment_widths(&parts, total, 10);
        assert_eq!(seg.iter().sum::<usize>(), 10);
        assert!(seg.iter().all(|w| *w >= 1));
    }

    #[test]
    fn bar_fills_exact_width() {
        let counts = JobCounts {
            completed: 4,
            failed: 1,
            ..Default::default()
        };
        let spans = bar_spans(&counts, 20);
        let drawn: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(drawn, 20);
    }

    #[test]
    fn empty_queue_bar_is_placeholder() {
        let counts = JobCounts::default();
        let spans = bar_spans(&counts, 12);
        let drawn: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(drawn, 12);
    }
}
