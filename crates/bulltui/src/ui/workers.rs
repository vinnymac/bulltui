//! Workers / Busy view: active jobs with worker-lock health, and the roster of
//! connected workers (from `CLIENT LIST`).

use bullmq::JobState;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Tabs};
use ratatui::Frame;

use crate::app::{list_offset, App, HitKind, HitRegion};
use crate::format;
use crate::state::WorkersTab;
use crate::theme;

/// Assumed lock duration for lock-health color (matches BullMQ's default
/// `lockDuration` of 30s). The raw TTL is always shown alongside.
const ASSUMED_LOCK_MS: i64 = 30_000;
const AT_RISK_MS: i64 = 5_000;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, hits: &mut Vec<HitRegion>) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    draw_tabs(frame, rows[0], app);
    match app.workers_tab {
        WorkersTab::Busy => draw_busy(frame, rows[1], app, hits),
        WorkersTab::Roster => draw_roster(frame, rows[1], app, hits),
    }
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let titles: Vec<Line> = WorkersTab::all()
        .iter()
        .map(|t| {
            let n = match t {
                WorkersTab::Busy => app.active_locks.len(),
                WorkersTab::Roster => app.workers.len(),
            };
            Line::from(format!("{} {n}", t.label()))
        })
        .collect();
    let idx = WorkersTab::all()
        .iter()
        .position(|t| *t == app.workers_tab)
        .unwrap_or(0);
    let tabs = Tabs::new(titles)
        .select(idx)
        .highlight_style(theme::selected().fg(theme::ACCENT))
        .divider(Span::styled("│", theme::muted()));
    frame.render_widget(tabs, area);
}

fn scope_label(app: &App) -> String {
    app.workers_scope
        .clone()
        .unwrap_or_else(|| "all queues".into())
}

fn lock_health_style(ttl_ms: i64) -> Style {
    if ttl_ms == -2 || ttl_ms == 0 {
        theme::danger()
    } else if ttl_ms > 0 && ttl_ms <= AT_RISK_MS {
        theme::state_style(JobState::Waiting)
    } else {
        theme::state_style(JobState::Active)
    }
}

fn draw_busy(frame: &mut Frame, area: Rect, app: &App, hits: &mut Vec<HitRegion>) {
    let widths = [
        Constraint::Length(2),  // at-risk marker
        Constraint::Min(12),    // name
        Constraint::Length(14), // queue
        Constraint::Length(12), // active for
        Constraint::Length(12), // lock ttl
        Constraint::Length(5),  // ats
        Constraint::Length(5),  // stc
    ];
    let header = Row::new(
        ["", "Name", "Queue", "Active for", "Lock TTL", "ats", "stc"]
            .into_iter()
            .map(|h| Cell::from(h).style(theme::header()))
            .collect::<Vec<_>>(),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_FOCUS))
        .title(Span::styled(" Busy ", theme::header()))
        .title_bottom(
            Line::from(Span::styled(
                format!(
                    " scope: {} · lock health vs assumed {}s* ",
                    scope_label(app),
                    ASSUMED_LOCK_MS / 1000
                ),
                theme::muted(),
            ))
            .right_aligned(),
        );
    let inner = block.inner(area);

    if app.active_locks.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("No active jobs.", theme::muted())))
                .block(block),
            area,
        );
        return;
    }

    let rows: Vec<Row> = app
        .active_locks
        .iter()
        .map(|lock| {
            let marker = if lock.will_be_declared_stalled() {
                Cell::from("☢").style(theme::danger())
            } else if lock.is_at_risk(AT_RISK_MS) {
                Cell::from("⚠").style(theme::state_style(JobState::Failed))
            } else {
                Cell::from(" ")
            };
            let elapsed = lock
                .active_for_ms(app.now)
                .map(format::human_duration)
                .unwrap_or_else(|| "—".into());
            let lock_txt = match lock.lock_ttl_ms {
                -2 => "expired".to_string(),
                -1 => "no-expiry".to_string(),
                ms => format::human_duration(ms),
            };
            Row::new(vec![
                marker,
                Cell::from(format::one_line(&lock.job.name)),
                Cell::from(lock.queue.clone()),
                Cell::from(elapsed),
                Cell::from(lock_txt).style(lock_health_style(lock.lock_ttl_ms)),
                Cell::from(lock.job.attempts_started.to_string()),
                Cell::from(lock.job.stalled_counter.to_string()),
            ])
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(theme::selected())
        .highlight_symbol("▌");
    let sel = app
        .active_selected
        .min(app.active_locks.len().saturating_sub(1));
    let rows_h = inner.height.saturating_sub(1); // header occupies inner.y
    let offset = list_offset(sel, rows_h as usize, app.active_locks.len());
    let mut state = TableState::default().with_offset(offset);
    state.select(Some(sel));
    frame.render_stateful_widget(table, area, &mut state);
    hits.push(HitRegion {
        kind: HitKind::ActiveLock,
        area: Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: rows_h,
        },
        offset,
        count: app.active_locks.len(),
    });
}

fn draw_roster(frame: &mut Frame, area: Rect, app: &App, hits: &mut Vec<HitRegion>) {
    let widths = [
        Constraint::Length(20), // addr
        Constraint::Length(14), // queue
        Constraint::Min(12),    // worker
        Constraint::Length(8),  // age
        Constraint::Length(8),  // idle
        Constraint::Min(10),    // last cmd
    ];
    let header = Row::new(
        ["Addr", "Queue", "Worker", "Age", "Idle", "Last cmd"]
            .into_iter()
            .map(|h| Cell::from(h).style(theme::header()))
            .collect::<Vec<_>>(),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_FOCUS))
        .title(Span::styled(" Workers ", theme::header()));
    let inner = block.inner(area);

    if let Some(err) = &app.workers_error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("CLIENT LIST: {err}"),
                theme::danger(),
            )))
            .block(block),
            area,
        );
        return;
    }
    if app.workers.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No connected workers.",
                theme::muted(),
            )))
            .block(block),
            area,
        );
        return;
    }

    let rows: Vec<Row> = app
        .workers
        .iter()
        .map(|w| {
            Row::new(vec![
                Cell::from(w.addr.clone()),
                Cell::from(w.queue.clone().unwrap_or_else(|| "—".into())),
                Cell::from(w.worker_name.clone().unwrap_or_else(|| "(unnamed)".into())),
                Cell::from(format!("{}s", w.age_secs)),
                Cell::from(format!("{}s", w.idle_secs)),
                Cell::from(w.last_cmd.clone()),
            ])
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(theme::selected())
        .highlight_symbol("▌");
    let sel = app.worker_selected.min(app.workers.len().saturating_sub(1));
    let rows_h = inner.height.saturating_sub(1); // header occupies inner.y
    let offset = list_offset(sel, rows_h as usize, app.workers.len());
    let mut state = TableState::default().with_offset(offset);
    state.select(Some(sel));
    frame.render_stateful_widget(table, area, &mut state);
    hits.push(HitRegion {
        kind: HitKind::Worker,
        area: Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: rows_h,
        },
        offset,
        count: app.workers.len(),
    });
}
