//! Schedulers screen: a table of a queue's job schedulers (cron / repeatable),
//! with the authoritative next-run countdown from the `repeat` ZSET score.

use bullmq::JobState;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::app::{list_offset, App, HitKind, HitRegion};
use crate::format;
use crate::theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, hits: &mut Vec<HitRegion>) {
    let widths = [
        Constraint::Length(20), // id
        Constraint::Min(12),    // name
        Constraint::Length(18), // schedule
        Constraint::Length(16), // tz
        Constraint::Length(18), // next run
        Constraint::Length(12), // in
        Constraint::Length(6),  // iter
        Constraint::Length(6),  // limit
    ];
    let header = Row::new(
        [
            "ID", "Name", "Schedule", "Timezone", "Next run", "In", "Iter", "Limit",
        ]
        .into_iter()
        .map(|h| Cell::from(h).style(theme::header()))
        .collect::<Vec<_>>(),
    );

    let title = format!(" Schedulers ({}) ", app.schedulers.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_FOCUS))
        .title(Span::styled(title, theme::header()))
        .title_bottom(
            Line::from(Span::styled(
                " t trigger · d remove · esc back ",
                theme::muted(),
            ))
            .right_aligned(),
        );
    let inner = block.inner(area);

    if app.schedulers.is_empty() {
        let msg = Paragraph::new(Line::from(Span::styled(
            "No job schedulers in this queue.",
            theme::muted(),
        )))
        .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let dash = || "—".to_string();
    let rows: Vec<Row> = app
        .schedulers
        .iter()
        .map(|s| {
            let name = match (&s.name, s.is_new_style()) {
                (Some(n), true) => n.clone(),
                (Some(n), false) => format!("{n} [legacy]"),
                (None, true) => dash(),
                (None, false) => "[legacy]".to_string(),
            };
            Row::new(vec![
                Cell::from(s.id.clone()),
                Cell::from(name),
                Cell::from(s.schedule_label()),
                Cell::from(s.tz.clone().unwrap_or_else(dash)),
                Cell::from(format::datetime(s.next_run_ms)),
                Cell::from(format::countdown(s.next_run_ms, app.now))
                    .style(theme::state_style(JobState::Delayed)),
                Cell::from(
                    s.iteration_count
                        .map(|n| n.to_string())
                        .unwrap_or_else(dash),
                ),
                Cell::from(s.limit.map(|n| n.to_string()).unwrap_or_else(dash)),
            ])
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(theme::selected())
        .highlight_symbol("▌");

    let sel = app
        .scheduler_selected
        .min(app.schedulers.len().saturating_sub(1));
    let rows_h = inner.height.saturating_sub(1); // header occupies inner.y
    let offset = list_offset(sel, rows_h as usize, app.schedulers.len());
    let mut state = TableState::default().with_offset(offset);
    state.select(Some(sel));
    frame.render_stateful_widget(table, area, &mut state);
    hits.push(HitRegion {
        kind: HitKind::Scheduler,
        area: Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: rows_h,
        },
        offset,
        count: app.schedulers.len(),
    });
}
