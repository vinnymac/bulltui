//! Queue screen: status tabs + a paginated job table with per-tab columns.

use bullmq::{DelayedKind, Job, JobState};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Tabs};
use ratatui::Frame;

use crate::app::App;
use crate::format;
use crate::state::StatusTab;
use crate::theme;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::vertical([
        Constraint::Length(1), // info
        Constraint::Length(1), // tabs
        Constraint::Min(1),    // table
    ])
    .split(area);

    draw_info(frame, rows[0], app);
    draw_tabs(frame, rows[1], app);
    draw_table(frame, rows[2], app);
}

fn draw_info(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = Vec::new();
    if let Some(summary) = &app.queue_summary {
        if summary.is_paused {
            spans.push(Span::styled(
                "⏸ PAUSED ",
                theme::state_style(JobState::Paused),
            ));
        } else {
            spans.push(Span::styled(
                "● running ",
                theme::state_style(JobState::Completed),
            ));
        }
        spans.push(Span::styled(
            format!("total {} ", summary.counts.total()),
            theme::muted(),
        ));
        if let Some(c) = summary.global_concurrency {
            spans.push(Span::styled(format!(" concurrency:{c} "), theme::muted()));
        }
    }
    if let Some(rl) = &app.rate_limit {
        if rl.is_throttled() {
            let txt = if rl.manual {
                " THROTTLED (manual) ".to_string()
            } else {
                format!(
                    " THROTTLED · resets in {} ",
                    format::human_duration(rl.ttl_ms)
                )
            };
            spans.push(Span::styled(txt, theme::danger()));
        }
        if let (Some(max), Some(dur)) = (rl.max, rl.duration_ms) {
            spans.push(Span::styled(
                format!(" limit {max}/{dur}ms "),
                theme::muted(),
            ));
        }
        if let Some(conc) = rl.concurrency {
            spans.push(Span::styled(
                format!(" active {}/{conc} ", rl.active),
                theme::muted(),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let tabs = StatusTab::all();
    let titles: Vec<Line> = tabs
        .iter()
        .map(|t| {
            let label = t.label();
            match t {
                StatusTab::Latest => Line::from(Span::raw(label)),
                StatusTab::State(s) => {
                    let n = app
                        .queue_summary
                        .as_ref()
                        .map(|sum| sum.counts.get(*s))
                        .unwrap_or(0);
                    Line::from(vec![
                        Span::raw(label),
                        Span::styled(format!(" {n}"), theme::state_style(*s)),
                    ])
                }
            }
        })
        .collect();
    let idx = tabs.iter().position(|t| *t == app.status_tab).unwrap_or(0);
    let tabs_widget = Tabs::new(titles)
        .select(idx)
        .highlight_style(theme::selected().fg(theme::ACCENT))
        .divider(Span::styled("│", theme::muted()));
    frame.render_widget(tabs_widget, area);
}

/// Header labels and column widths for a given tab.
fn columns(tab: StatusTab) -> (Vec<&'static str>, Vec<Constraint>) {
    use Constraint::*;
    match tab {
        StatusTab::Latest => (
            vec!["State", "ID", "Name", "Added", "Att"],
            vec![Length(12), Length(10), Min(16), Length(16), Length(4)],
        ),
        StatusTab::State(JobState::Active) => (
            vec!["ID", "Name", "Started", "Worker", "Att"],
            vec![Length(10), Min(16), Length(16), Length(18), Length(4)],
        ),
        StatusTab::State(JobState::Completed) => (
            vec!["ID", "Name", "Finished", "Duration", "Att"],
            vec![Length(10), Min(16), Length(16), Length(10), Length(4)],
        ),
        StatusTab::State(JobState::Failed) => (
            vec!["ID", "Name", "Finished", "Att", "Reason"],
            vec![Length(10), Min(14), Length(16), Length(4), Min(18)],
        ),
        StatusTab::State(JobState::Delayed) => (
            vec!["Kind", "ID", "Name", "Run at", "In", "Reason"],
            vec![
                Length(9),
                Length(10),
                Min(14),
                Length(18),
                Length(12),
                Min(12),
            ],
        ),
        StatusTab::State(JobState::Prioritized) => (
            vec!["ID", "Name", "Priority", "Added"],
            vec![Length(10), Min(16), Length(9), Length(16)],
        ),
        StatusTab::State(_) => (
            vec!["ID", "Name", "Added", "Att"],
            vec![Length(10), Min(16), Length(16), Length(4)],
        ),
    }
}

fn job_cells(tab: StatusTab, job: &Job, now: i64) -> Vec<Cell<'static>> {
    let id = Cell::from(job.id.clone());
    let name = Cell::from(format::one_line(&job.name));
    let att = Cell::from(job.attempts_made.to_string());
    match tab {
        StatusTab::Latest => {
            let state = job.state.unwrap_or(JobState::Waiting);
            vec![
                Cell::from(state.label()).style(theme::state_style(state)),
                id,
                name,
                Cell::from(format::relative(job.timestamp, now)),
                att,
            ]
        }
        StatusTab::State(JobState::Active) => vec![
            id,
            name,
            Cell::from(format::relative(job.processed_on, now)),
            Cell::from(job.processed_by.clone().unwrap_or_else(|| "—".into())),
            att,
        ],
        StatusTab::State(JobState::Completed) => vec![
            id,
            name,
            Cell::from(format::relative(job.finished_on, now)),
            Cell::from(format::duration_between(job.processed_on, job.finished_on)),
            att,
        ],
        StatusTab::State(JobState::Failed) => vec![
            id,
            name,
            Cell::from(format::relative(job.finished_on, now)),
            att,
            Cell::from(format::one_line(
                job.failed_reason.as_deref().unwrap_or("—"),
            ))
            .style(theme::state_style(JobState::Failed)),
        ],
        StatusTab::State(JobState::Delayed) => {
            let run_at = job.delayed_run_at();
            let kind = job.delayed_kind();
            let kind_style = match kind {
                DelayedKind::Scheduled => theme::state_style(JobState::Delayed),
                DelayedKind::RetryBackoff => theme::state_style(JobState::Failed),
                DelayedKind::Plain => theme::muted(),
            };
            let reason = match kind {
                DelayedKind::RetryBackoff => {
                    format::one_line(job.failed_reason.as_deref().unwrap_or("—"))
                }
                _ => "—".to_string(),
            };
            vec![
                Cell::from(kind.label()).style(kind_style),
                id,
                name,
                Cell::from(format::datetime(run_at)),
                Cell::from(format::countdown(run_at, now)),
                Cell::from(reason),
            ]
        }
        StatusTab::State(JobState::Prioritized) => vec![
            id,
            name,
            Cell::from(job.priority.to_string()).style(theme::state_style(JobState::Prioritized)),
            Cell::from(format::relative(job.timestamp, now)),
        ],
        StatusTab::State(_) => vec![
            id,
            name,
            Cell::from(format::relative(job.timestamp, now)),
            att,
        ],
    }
}

fn draw_table(frame: &mut Frame, area: Rect, app: &App) {
    let (mut headers, mut widths) = columns(app.status_tab);
    // A leading 2-col gutter for the multi-select check mark.
    headers.insert(0, "");
    widths.insert(0, Constraint::Length(2));
    let header = Row::new(
        headers
            .iter()
            .map(|h| Cell::from(*h).style(theme::header()))
            .collect::<Vec<_>>(),
    );

    let jobs = app.visible_jobs();
    let sel = app.effective_job_selection();

    let title = format!(" {} ", app.status_tab.label());
    let mut bottom = if matches!(app.status_tab, StatusTab::Latest) {
        format!(" {} shown ", jobs.len())
    } else {
        format!(
            " page {}/{} · {} jobs ",
            app.page + 1,
            app.page_count(),
            app.current_status_count()
        )
    };
    if let Some(f) = &app.job_filter {
        bottom = format!(" /{f} ·{bottom}");
    }
    if !sel.is_empty() {
        bottom = format!(" {} selected ·{bottom}", sel.len());
    }
    if app.range_anchor.is_some() {
        bottom = format!(" range ·{bottom}");
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::BORDER_FOCUS))
        .title(Span::styled(title, theme::header()))
        .title_bottom(Line::from(Span::styled(bottom, theme::muted())).right_aligned());

    if jobs.is_empty() {
        let empty = if app.job_filter.is_some() {
            "No jobs match the filter."
        } else {
            "No jobs in this state."
        };
        let msg = Paragraph::new(Line::from(Span::styled(empty, theme::muted()))).block(block);
        frame.render_widget(msg, area);
        return;
    }

    let rows: Vec<Row> = jobs
        .iter()
        .map(|j| {
            let mut cells = job_cells(app.status_tab, j, app.now);
            let mark = if sel.contains(&j.id) {
                Cell::from("✓").style(theme::select_mark())
            } else {
                Cell::from(" ")
            };
            cells.insert(0, mark);
            Row::new(cells)
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(theme::selected())
        .highlight_symbol("▌");

    let mut state = TableState::default();
    state.select(Some(app.job_selected.min(jobs.len().saturating_sub(1))));
    frame.render_stateful_widget(table, area, &mut state);
}
