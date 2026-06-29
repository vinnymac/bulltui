//! Modal overlays: help, confirm, input form, redis stats, metrics, settings.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Sparkline, Wrap};
use ratatui::Frame;

use super::{centered_rect, centered_sized, modal_block};
use crate::app::App;
use crate::format;
use crate::state::Overlay;
use crate::theme;

pub fn draw_help(frame: &mut Frame, app: &App) {
    let area = centered_rect(70, 80, frame.area());
    let inner = modal_block(frame, area, "Help — keybindings", false);

    let h = |s: &str| Line::from(Span::styled(s.to_string(), theme::header()));
    let k = |key: &str, desc: &str| {
        Line::from(vec![
            Span::styled(format!("  {key:<12}"), theme::key_hint()),
            Span::raw(desc.to_string()),
        ])
    };
    // Built from the single keybinding registry (see `crate::keymap`) so help
    // and the status line stay in lockstep, and scoped to the active screen so
    // `?` shows only what's relevant here.
    let mut lines = Vec::new();
    for group in crate::keymap::help_groups(app.screen) {
        lines.push(h(group.title()));
        for b in crate::keymap::bindings_in(group) {
            lines.push(k(b.keys, b.desc));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "  press any key to close",
        theme::muted(),
    )));
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        inner,
    );
}

pub fn draw_confirm(frame: &mut Frame, app: &App) {
    let Overlay::Confirm(action) = &app.overlay else {
        return;
    };
    let danger = action.is_destructive();
    let area = centered_sized(64, 7, frame.area());
    let inner = modal_block(frame, area, "Confirm", danger);

    let desc_style = if danger {
        theme::danger()
    } else {
        Style::default()
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(action.describe(), desc_style)).centered(),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [y]", theme::key_hint()),
            Span::raw(" confirm    "),
            Span::styled("[n/Esc]", theme::key_hint()),
            Span::raw(" cancel"),
        ])
        .centered(),
    ];
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

pub fn draw_input(frame: &mut Frame, app: &App) {
    let Overlay::Input(form) = &app.overlay else {
        return;
    };
    let multiline = form.fields.iter().any(|f| f.multiline);
    // A single-line field is a bordered box: 1 content row + 2 border rows = 3.
    // The modal adds its own top/bottom border (2) and a trailing hint row (1).
    let height = if multiline {
        18
    } else {
        3 * form.fields.len() as u16 + 3
    };
    let area = centered_sized(72, height.min(frame.area().height), frame.area());
    let inner = modal_block(frame, area, &form.title, false);

    // Split: a region per field (taller for multiline) + a hint line.
    let mut constraints: Vec<Constraint> = form
        .fields
        .iter()
        .map(|f| {
            if f.multiline {
                Constraint::Min(3)
            } else {
                Constraint::Length(3)
            }
        })
        .collect();
    constraints.push(Constraint::Length(1));
    let chunks = Layout::vertical(constraints).split(inner);

    for (i, field) in form.fields.iter().enumerate() {
        let focused = i == form.focus;
        let label_style = if focused {
            theme::key_hint()
        } else {
            theme::muted()
        };
        let mut value = field.value.clone();
        if focused {
            value.push('▏'); // visible cursor
        }
        let border = if focused {
            theme::BORDER_FOCUS
        } else {
            theme::BORDER
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .title(Span::styled(field.label.clone(), label_style));

        if field.multiline {
            let para = Paragraph::new(value)
                .block(block)
                .wrap(Wrap { trim: false });
            frame.render_widget(para, chunks[i]);
        } else {
            // Single-line: show the tail so the cursor stays visible even when
            // the value is longer than the box.
            let text_w = chunks[i].width.saturating_sub(2) as usize;
            let display = tail_fit(&value, text_w);
            frame.render_widget(Paragraph::new(display).block(block), chunks[i]);
        }
    }

    let mut hint = vec![
        Span::styled("Tab", theme::key_hint()),
        Span::raw(" next  "),
        Span::styled("Enter", theme::key_hint()),
        Span::raw(if multiline { " newline  " } else { " submit  " }),
    ];
    if multiline {
        hint.push(Span::styled("Ctrl+Enter", theme::key_hint()));
        hint.push(Span::raw(" submit  "));
    }
    hint.push(Span::styled("Ctrl+U", theme::key_hint()));
    hint.push(Span::raw(" clear  "));
    hint.push(Span::styled("Esc", theme::key_hint()));
    hint.push(Span::raw(" cancel"));
    frame.render_widget(Paragraph::new(Line::from(hint)), chunks[form.fields.len()]);
}

/// Keep the last `width` characters of `s` so the end (and cursor) stay visible.
fn tail_fit(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        s.to_string()
    } else {
        chars[chars.len() - width..].iter().collect()
    }
}

pub fn draw_redis_stats(frame: &mut Frame, app: &App) {
    let area = centered_sized(60, 21, frame.area());
    let inner = modal_block(frame, area, "Redis", false);

    let Some(info) = &app.redis_info else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("Loading…", theme::muted()))),
            inner,
        );
        return;
    };

    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<22}"), theme::header()),
            Span::raw(v),
        ])
    };
    let mem = match info.memory_usage_fraction() {
        Some(f) => format!(
            "{:.1}%  ({} / {})",
            f * 100.0,
            format::bytes(info.used_memory().unwrap_or(0)),
            format::bytes(info.total_memory().unwrap_or(0)),
        ),
        None => format::bytes(info.used_memory().unwrap_or(0)),
    };
    let lines = vec![
        kv("Version", info.version().unwrap_or("?").to_string()),
        kv("Mode", info.mode().unwrap_or("standalone").to_string()),
        kv(
            "Port",
            info.tcp_port()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "?".into()),
        ),
        kv("OS", info.os().unwrap_or("?").to_string()),
        kv(
            "Uptime",
            format::human_duration(info.uptime_seconds().unwrap_or(0) * 1000),
        ),
        kv("Memory", mem),
        kv(
            "Peak memory",
            format::bytes(info.used_memory_peak().unwrap_or(0)),
        ),
        kv(
            "Fragmentation",
            info.mem_fragmentation_ratio()
                .map(|r| format!("{r:.2}"))
                .unwrap_or_else(|| "?".into()),
        ),
        kv(
            "Connected clients",
            info.connected_clients()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into()),
        ),
        kv(
            "Blocked clients",
            info.blocked_clients()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into()),
        ),
        kv(
            "Hit ratio",
            info.hit_ratio()
                .map(|r| format!("{:.1}%", r * 100.0))
                .unwrap_or_else(|| "?".into()),
        ),
        kv(
            "Ops/sec",
            info.instantaneous_ops_per_sec()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into()),
        ),
        kv(
            "Evicted keys",
            info.evicted_keys()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into()),
        ),
        Line::from(""),
        Line::from(Span::styled("press i/Esc to close", theme::muted())),
    ];
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

pub fn draw_metrics(frame: &mut Frame, app: &App) {
    let area = centered_rect(80, 60, frame.area());
    let queue = app.queue_name.clone().unwrap_or_default();
    let inner = modal_block(frame, area, &format!("Metrics — {queue}"), false);

    let Some((completed, failed)) = &app.metrics else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("Loading…", theme::muted()))),
            inner,
        );
        return;
    };

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(inner);

    let to_u64 = |d: &[i64]| d.iter().map(|v| (*v).max(0) as u64).collect::<Vec<u64>>();

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Completed/min  ",
                theme::state_style(bullmq::JobState::Completed),
            ),
            Span::styled(
                format!(
                    "total {} · {} points",
                    completed.count,
                    completed.data.len()
                ),
                theme::muted(),
            ),
        ])),
        rows[0],
    );
    let completed_data = to_u64(&completed.data);
    frame.render_widget(
        Sparkline::default()
            .data(&completed_data)
            .style(theme::state_style(bullmq::JobState::Completed)),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Failed/min  ", theme::state_style(bullmq::JobState::Failed)),
            Span::styled(
                format!("total {} · {} points", failed.count, failed.data.len()),
                theme::muted(),
            ),
        ])),
        rows[2],
    );
    let failed_data = to_u64(&failed.data);
    frame.render_widget(
        Sparkline::default()
            .data(&failed_data)
            .style(theme::state_style(bullmq::JobState::Failed)),
        rows[3],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "press m/Esc to close",
            theme::muted(),
        ))),
        rows[4],
    );
}

pub fn draw_settings(frame: &mut Frame, app: &App) {
    let area = centered_sized(56, 9, frame.area());
    let inner = modal_block(frame, area, "Settings", false);

    let s = &app.settings;
    let poll = if s.poll_secs == 0 {
        "off".to_string()
    } else {
        format!("{}s", s.poll_secs)
    };
    let items = [
        ("Auto-refresh", poll),
        ("Jobs per page", s.jobs_per_page.to_string()),
        (
            "Confirm actions",
            if s.confirm_actions {
                "on".into()
            } else {
                "off".into()
            },
        ),
    ];
    let mut lines = vec![Line::from("")];
    for (i, (label, value)) in items.iter().enumerate() {
        let focused = i == s.focus;
        let marker = if focused { "▶ " } else { "  " };
        let style = if focused {
            theme::key_hint()
        } else {
            theme::muted()
        };
        lines.push(Line::from(vec![
            Span::styled(marker, theme::key_hint()),
            Span::styled(format!("{label:<18}"), style),
            Span::styled(
                format!("‹ {value} ›"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ select · ←→ change · Esc close",
        theme::muted(),
    )));
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

pub fn draw_palette(frame: &mut Frame, app: &App) {
    let Overlay::Palette(st) = &app.overlay else {
        return;
    };
    let area = centered_rect(60, 70, frame.area());
    let inner = modal_block(frame, area, "Command", false);

    let rows = Layout::vertical([
        Constraint::Length(1), // prompt
        Constraint::Min(1),    // results
        Constraint::Length(1), // hint
    ])
    .split(inner);

    let prompt = format!(":{}▏", st.buffer);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(prompt, theme::key_hint()))),
        rows[0],
    );

    let height = rows[1].height as usize;
    let total = st.filtered.len();
    let sel = st.selected.min(total.saturating_sub(1));
    let start = if height > 0 && sel >= height {
        sel + 1 - height
    } else {
        0
    };
    let mut lines = Vec::new();
    for (row, &i) in st.filtered.iter().enumerate().skip(start).take(height) {
        let item = &st.items[i];
        let (marker, style) = if row == sel {
            ("▌ ", theme::selected())
        } else {
            ("  ", Style::default())
        };
        lines.push(Line::from(vec![
            Span::styled(marker, theme::key_hint()),
            Span::styled(item.label.clone(), style),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("  no matches", theme::muted())));
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), rows[1]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "↑↓ select · ⏎ run · Esc cancel",
            theme::muted(),
        ))),
        rows[2],
    );
}

pub fn draw_filter(frame: &mut Frame, app: &App) {
    let Overlay::Filter(st) = &app.overlay else {
        return;
    };
    let title = match st.scope {
        crate::state::FilterScope::Overview => "Filter queues",
        crate::state::FilterScope::QueueJobs => "Filter jobs",
        crate::state::FilterScope::Events => "Filter events",
    };
    let area = centered_sized(60, 3, frame.area());
    let inner = modal_block(frame, area, title, false);
    let line = Line::from(vec![
        Span::styled(format!("/{}▏", st.buffer), theme::key_hint()),
        Span::styled("   ⏎ keep · Esc clear · ! negates", theme::muted()),
    ]);
    frame.render_widget(Paragraph::new(line), inner);
}
