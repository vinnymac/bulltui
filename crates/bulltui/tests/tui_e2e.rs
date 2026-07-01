//! End-to-end TUI tests: drive the real `App` against a seeded Valkey/Redis
//! and assert on what ratatui actually renders (via `TestBackend`) and on the
//! resulting state after simulated keypresses.

use std::collections::HashMap;
use std::time::Duration;

use bullmq::{EventKind, JobState, QueueEvent};
use bulltui::app::{App, HitKind, Screen};
use bulltui::cli::Args;
use bulltui::state::{EventScope, JobTab, StatusTab, WorkersTab};
use bulltui::ui;
use clap::Parser;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};

async fn start_redis() -> (ContainerAsync<GenericImage>, String) {
    std::env::set_var("TESTCONTAINERS_RYUK_DISABLED", "true");
    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .expect("start redis");
    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(6379.tcp()).await.unwrap();
    (container, format!("redis://{host}:{port}"))
}

async fn run_seeder(url: &str) {
    let seeder = format!("{}/../../e2e/seeder/seed.mjs", env!("CARGO_MANIFEST_DIR"));
    let output = tokio::process::Command::new("node")
        .arg(&seeder)
        .arg(url)
        .output()
        .await
        .expect("spawn seeder");
    assert!(
        output.status.success(),
        "seeder failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn buffer_text(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

async fn seeded_app() -> (ContainerAsync<GenericImage>, App) {
    let (container, url) = start_redis().await;
    run_seeder(&url).await;
    let client = bullmq::BullClient::connect(&url, "bull")
        .await
        .expect("connect");
    let args = Args::parse_from(["bulltui", "--url", &url, "--poll", "0"]);
    let mut app = App::new(client, args);
    app.refresh_overview().await;
    (container, app)
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    buffer_text(terminal.backend().buffer())
}

async fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE)).await;
}

async fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        press(app, KeyCode::Char(c)).await;
    }
}

/// Inject a synthetic left-click at `(col, row)` — the mouse analogue of
/// `press`. Hit-testing reads the regions recorded by the previous `render`, so
/// callers render first (exactly as the run loop draws before reading events).
async fn click(app: &mut App, col: u16, row: u16) {
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    })
    .await;
}

async fn wheel(app: &mut App, down: bool) {
    app.on_mouse(MouseEvent {
        kind: if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        },
        column: 1,
        row: 1,
        modifiers: KeyModifiers::NONE,
    })
    .await;
}

async fn tab_to(app: &mut App, state: JobState) {
    for _ in 0..StatusTab::all().len() {
        if app.status_tab == StatusTab::State(state) {
            return;
        }
        press(app, KeyCode::Tab).await;
    }
    panic!("never reached {state:?} tab");
}

async fn job_tab_to(app: &mut App, tab: JobTab) {
    for _ in 0..JobTab::all().len() {
        if app.job_tab == tab {
            return;
        }
        press(app, KeyCode::Tab).await;
    }
    panic!("never reached {tab:?} job tab");
}

#[tokio::test]
async fn overview_renders_seeded_queues() {
    let (_c, mut app) = seeded_app().await;
    let text = render(&mut app, 120, 30);
    assert!(text.contains("bulltui"), "title rendered");
    assert!(text.contains("emails"), "emails row:\n{text}");
    assert!(text.contains("notifications"));
    assert!(text.contains("Queues"));
    assert!(text.contains('⏸'), "paused indicator (reports):\n{text}");
}

#[tokio::test]
async fn navigates_overview_to_queue_to_job() {
    let (_c, mut app) = seeded_app().await;

    // emails sorts first alphabetically; open it.
    press(&mut app, KeyCode::Enter).await;
    assert_eq!(app.screen, Screen::Queue);
    assert_eq!(app.queue_name.as_deref(), Some("emails"));

    // Tab across to the Completed status tab and confirm jobs load + render.
    for _ in 0..5 {
        press(&mut app, KeyCode::Tab).await; // Latest→Active→Waiting→WChildren→Prioritized→Completed
    }
    assert!(!app.jobs.is_empty(), "completed jobs loaded");
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Completed"), "completed tab visible:\n{text}");

    // Open the first job's detail view.
    press(&mut app, KeyCode::Enter).await;
    assert_eq!(app.screen, Screen::Job);
    let job_id = app.job.as_ref().unwrap().id.clone();
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Data"), "job tabs visible:\n{text}");
    assert!(text.contains(&job_id), "job id in header:\n{text}");

    // Switch to the Logs tab; the seeder logged "processing"/"done" lines.
    for _ in 0..4 {
        press(&mut app, KeyCode::Tab).await; // Data→Options→Progress→Error→Logs
    }
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Logs"), "logs tab:\n{text}");
}

#[tokio::test]
async fn flow_tab_drills_into_a_child_job() {
    let (_c, mut app) = seeded_app().await;

    // Reach the flow parent: orchestrator queue → waiting-children → open it.
    // The seeder builds `orchestrator/aggregate` with three `workers/collect`
    // children, so this is a genuine cross-queue flow.
    press(&mut app, KeyCode::Char(':')).await;
    type_str(&mut app, "orchestr").await;
    press(&mut app, KeyCode::Enter).await;
    assert_eq!(app.queue_name.as_deref(), Some("orchestrator"));

    tab_to(&mut app, JobState::WaitingChildren).await;
    assert!(
        !app.jobs.is_empty(),
        "the aggregate parent waits on its children"
    );
    press(&mut app, KeyCode::Enter).await; // open the parent
    assert_eq!(app.screen, Screen::Job);
    assert_eq!(app.queue_name.as_deref(), Some("orchestrator"));
    let parent_id = app.job.as_ref().unwrap().id.clone();

    // The flow tree loaded and the cursor starts on the focused (root) node.
    assert!(app.job_flow.is_some(), "flow tree fetched");
    assert_eq!(app.flow_selected, 0, "cursor starts on the focused root");

    job_tab_to(&mut app, JobTab::Flow).await;
    let text = render(&mut app, 120, 30);
    assert!(text.contains("aggregate"), "root rendered:\n{text}");
    assert!(text.contains("collect"), "children rendered:\n{text}");
    assert!(
        text.contains("workers/"),
        "cross-queue child labelled:\n{text}"
    );

    // Move the cursor onto the first child and drill in.
    press(&mut app, KeyCode::Down).await;
    assert_eq!(app.flow_selected, 1, "cursor moved to the first child");
    press(&mut app, KeyCode::Enter).await;

    // We jumped across queues into a child job, still on the Flow tab.
    assert_eq!(app.screen, Screen::Job);
    assert_eq!(
        app.queue_name.as_deref(),
        Some("workers"),
        "jumped to the child's queue"
    );
    assert_eq!(
        app.job_tab,
        JobTab::Flow,
        "stays on the Flow tab while drilling"
    );
    let child = app.job.as_ref().unwrap();
    assert_eq!(child.name, "collect", "opened a collect child");
    assert_ne!(child.id, parent_id, "now viewing a different job");
}

#[tokio::test]
async fn job_detail_body_scrolls_within_bounds_with_a_scrollbar() {
    let (_c, mut app) = seeded_app().await;

    // emails has permanently-failed `fail` jobs (attempts: 1) whose worker throws
    // `boom for <id>` — a real multi-line Node stacktrace on the Error tab.
    press(&mut app, KeyCode::Enter).await; // open emails (sorts first)
    assert_eq!(app.queue_name.as_deref(), Some("emails"));
    tab_to(&mut app, JobState::Failed).await;
    assert!(!app.jobs.is_empty(), "failed jobs present");
    press(&mut app, KeyCode::Enter).await; // open a failed job
    assert_eq!(app.screen, Screen::Job);
    job_tab_to(&mut app, JobTab::Error).await;

    // A short viewport guarantees the stacktrace overflows. The render records
    // the exact (wrapped) scroll bounds into `detail_view`.
    let _ = render(&mut app, 100, 14);
    assert!(
        app.detail_view.max_scroll > 0,
        "the error content overflows the viewport (max_scroll {})",
        app.detail_view.max_scroll
    );
    assert_eq!(app.detail_scroll, 0, "opens at the top");

    // Hammering Down past the end clamps to the last line — never into the void.
    for _ in 0..100 {
        press(&mut app, KeyCode::Down).await;
    }
    let max = app.detail_view.max_scroll;
    assert_eq!(app.detail_scroll, max, "clamped at the bottom, not past it");

    // Home returns to the top; End jumps back to the bottom.
    press(&mut app, KeyCode::Home).await;
    assert_eq!(app.detail_scroll, 0, "Home → top");
    press(&mut app, KeyCode::End).await;
    assert_eq!(app.detail_scroll, max, "End → bottom");

    // PageUp from the bottom steps back by a viewport-worth (never negative).
    press(&mut app, KeyCode::PageUp).await;
    assert!(
        app.detail_scroll < max,
        "PageUp steps back up from the bottom"
    );

    // A scrollbar thumb (█) is drawn on the right border when content overflows —
    // and only then; the Error tab has no other █ glyph.
    let text = render(&mut app, 100, 14);
    assert!(
        text.contains('█'),
        "a scrollbar thumb renders while the body overflows:\n{text}"
    );

    // Switching tabs resets the scroll (cycle_job_tab), so a short tab can't
    // inherit a tall tab's offset and strand its content off-screen.
    press(&mut app, KeyCode::Tab).await; // Error → Logs
    assert_eq!(app.detail_scroll, 0, "tab switch resets scroll to the top");
}

#[tokio::test]
async fn pause_action_via_keys_updates_state() {
    let (_c, mut app) = seeded_app().await;

    press(&mut app, KeyCode::Enter).await; // open emails
    assert_eq!(app.queue_name.as_deref(), Some("emails"));
    assert!(
        !app.queue_summary.as_ref().unwrap().is_paused,
        "starts running"
    );

    press(&mut app, KeyCode::Char('p')).await; // request pause -> confirm overlay
    press(&mut app, KeyCode::Char('y')).await; // confirm

    // The queue is paused both per the client and in the reloaded summary.
    assert!(
        app.client.is_paused("emails").await.unwrap(),
        "paused in redis"
    );
    assert!(
        app.queue_summary.as_ref().unwrap().is_paused,
        "summary shows paused"
    );
}

#[tokio::test]
async fn command_palette_navigates_to_queue() {
    let (_c, mut app) = seeded_app().await;
    // Open the palette and fuzzy-type a queue name.
    press(&mut app, KeyCode::Char(':')).await;
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Command"), "palette modal open:\n{text}");
    type_str(&mut app, "notif").await;
    let text = render(&mut app, 120, 30);
    assert!(text.contains("notifications"), "fuzzy match shown:\n{text}");
    press(&mut app, KeyCode::Enter).await;
    assert_eq!(app.screen, Screen::Queue);
    assert_eq!(app.queue_name.as_deref(), Some("notifications"));
}

#[tokio::test]
async fn live_filter_narrows_jobs() {
    let (_c, mut app) = seeded_app().await;
    press(&mut app, KeyCode::Enter).await; // open emails (Latest tab)
    let total = app.visible_jobs().len();
    assert!(total >= 2, "need several jobs on Latest, got {total}");

    press(&mut app, KeyCode::Char('/')).await; // open the live filter
    type_str(&mut app, "failed").await; // match on the state label
    let filtered = app.visible_jobs().len();
    assert!(
        filtered > 0 && filtered < total,
        "filtered {filtered} of {total}"
    );
    assert!(
        app.visible_jobs()
            .iter()
            .all(|j| j.state == Some(JobState::Failed)),
        "only failed jobs remain"
    );

    press(&mut app, KeyCode::Esc).await; // clears the filter
    assert!(app.job_filter.is_none(), "filter cleared");
    assert_eq!(app.visible_jobs().len(), total, "list restored");
}

#[tokio::test]
async fn multi_select_bulk_retry() {
    let (_c, mut app) = seeded_app().await;
    press(&mut app, KeyCode::Enter).await; // open emails
    tab_to(&mut app, JobState::Failed).await;
    let failed_before = app.queue_summary.as_ref().unwrap().counts.failed;
    assert!(failed_before >= 2, "need ≥2 failed, got {failed_before}");

    // Select two rows with Space.
    press(&mut app, KeyCode::Char(' ')).await;
    press(&mut app, KeyCode::Down).await;
    press(&mut app, KeyCode::Char(' ')).await;
    assert_eq!(app.job_selection.len(), 2, "two jobs selected");

    // Bulk retry → confirm dialog describes the count.
    press(&mut app, KeyCode::Char('r')).await;
    let text = render(&mut app, 120, 30);
    assert!(
        text.contains("Retry 2 selected"),
        "bulk confirm text:\n{text}"
    );
    press(&mut app, KeyCode::Char('y')).await;

    assert!(app.job_selection.is_empty(), "selection cleared after bulk");
    let failed_after = app
        .client
        .queue_summary("emails")
        .await
        .unwrap()
        .counts
        .failed;
    assert_eq!(
        failed_after,
        failed_before - 2,
        "two failed jobs retried (was {failed_before})"
    );
}

async fn open_notifications(app: &mut App) {
    press(app, KeyCode::Char(':')).await;
    type_str(app, "notif").await;
    press(app, KeyCode::Enter).await;
    assert_eq!(app.queue_name.as_deref(), Some("notifications"));
}

#[tokio::test]
async fn schedulers_screen_renders() {
    let (_c, mut app) = seeded_app().await;
    open_notifications(&mut app).await;
    press(&mut app, KeyCode::Char('S')).await;
    assert_eq!(app.screen, Screen::Schedulers);
    assert!(
        app.schedulers.len() >= 2,
        "two schedulers seeded, got {}",
        app.schedulers.len()
    );
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Schedulers"), "title:\n{text}");
    assert!(text.contains("digest-cron"), "cron scheduler id:\n{text}");
    assert!(
        text.contains("poll-every"),
        "interval scheduler id:\n{text}"
    );
}

#[tokio::test]
async fn delayed_split_marks_scheduled() {
    let (_c, mut app) = seeded_app().await;
    open_notifications(&mut app).await;
    tab_to(&mut app, JobState::Delayed).await;
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Kind"), "kind column:\n{text}");
    // Scheduler-produced delayed jobs classify as "scheduled".
    assert!(text.contains("scheduled"), "scheduled badge:\n{text}");
}

#[tokio::test]
async fn reprioritize_via_keys() {
    let (_c, mut app) = seeded_app().await;
    open_notifications(&mut app).await;
    tab_to(&mut app, JobState::Prioritized).await;
    assert!(!app.jobs.is_empty(), "prioritized jobs seeded");
    let before = app
        .client
        .queue_summary("notifications")
        .await
        .unwrap()
        .counts
        .prioritized;
    let id = app.selected_job().unwrap().id.clone();

    press(&mut app, KeyCode::Char('#')).await; // open re-prioritize form
    app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .await; // Ctrl+U clear
    type_str(&mut app, "9").await;
    press(&mut app, KeyCode::Enter).await; // submit

    let job = app
        .client
        .get_job("notifications", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.priority, 9, "priority updated");
    let after = app
        .client
        .queue_summary("notifications")
        .await
        .unwrap()
        .counts
        .prioritized;
    assert_eq!(after, before, "job stays in prioritized (count unchanged)");
}

#[tokio::test]
async fn workers_busy_view_shows_active_job() {
    let (_c, mut app) = seeded_app().await;
    // `media` has an active "transcode" job (its worker exited mid-flight).
    press(&mut app, KeyCode::Char(':')).await;
    type_str(&mut app, "media").await;
    press(&mut app, KeyCode::Enter).await;
    assert_eq!(app.queue_name.as_deref(), Some("media"));

    press(&mut app, KeyCode::Char('w')).await;
    assert_eq!(app.screen, Screen::Workers);
    assert!(!app.active_locks.is_empty(), "media has an active job");
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Busy"), "busy tab:\n{text}");
    assert!(text.contains("transcode"), "active job name:\n{text}");

    press(&mut app, KeyCode::Tab).await; // → Roster
    assert_eq!(app.workers_tab, WorkersTab::Roster);
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Workers"), "roster tab title:\n{text}");
}

fn ev(ts: i64, kind: EventKind, job: &str) -> QueueEvent {
    QueueEvent {
        stream_id: format!("{ts}-0"),
        ts,
        queue: "emails".into(),
        kind,
        job_id: Some(job.into()),
        fields: HashMap::new(),
    }
}

#[tokio::test]
async fn events_feed_backfills_and_ingests() {
    let (_c, mut app) = seeded_app().await;

    // The bullmq events reader reads the seeded `emails` stream (added/…events).
    let backfilled = app.client.backfill_events("emails", 100).await.unwrap();
    assert!(!backfilled.is_empty(), "emails stream has events");

    app.open_events(EventScope::All).await;
    assert_eq!(app.screen, Screen::Events);

    // Inject synthetic events through the determinism seam (no socket).
    app.ingest_events(vec![
        ev(1, EventKind::Added, "1"),
        ev(2, EventKind::Completed, "1"),
        ev(3, EventKind::Failed, "2"),
    ]);
    assert_eq!(app.filtered_event_count(), 3);
    assert_eq!(app.events_selected, 2, "follow tracks the tail");
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Events"), "events screen:\n{text}");
    assert!(text.contains("completed"), "completed kind:\n{text}");
    assert!(text.contains("failed"), "failed kind:\n{text}");

    // Live filter narrows to failures, then Esc restores.
    press(&mut app, KeyCode::Char('/')).await;
    type_str(&mut app, "failed").await;
    assert_eq!(app.filtered_event_count(), 1);
    assert!(app.hidden_event_count() >= 2, "others hidden");
    press(&mut app, KeyCode::Esc).await;
    assert_eq!(app.filtered_event_count(), 3, "filter cleared");

    // Pause freezes selection but still buffers.
    app.events_selected = 0;
    app.events_follow = false;
    app.events_paused = true;
    app.ingest_events(vec![ev(4, EventKind::Completed, "3")]);
    assert_eq!(app.events_selected, 0, "paused: selection frozen");
    assert_eq!(app.filtered_event_count(), 4, "still buffered");
}

#[tokio::test]
async fn events_jump_to_failure() {
    let (_c, mut app) = seeded_app().await;
    app.open_events(EventScope::All).await;
    app.ingest_events(vec![
        ev(1, EventKind::Added, "1"),
        ev(2, EventKind::Completed, "1"),
        ev(3, EventKind::Failed, "2"),
    ]);
    app.events_selected = 0;
    app.events_follow = false;
    press(&mut app, KeyCode::Char('n')).await; // next failure
    assert_eq!(app.events_selected, 2, "jumped to the failed event");
}

#[tokio::test]
async fn event_stream_task_delivers_live() {
    let (_c, app) = seeded_app().await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let handle = bulltui::events::EventStreamHandle::spawn(
        app.client.clone(),
        vec!["emails".into()],
        100,
        tx,
    );

    // First batch is the backfill (read on the shared connection).
    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("backfill within 5s")
        .expect("a batch");
    assert!(!first.is_empty(), "backfill delivered");

    // Produce a live event; `pause` XADDs a `paused` event to the stream. The
    // dedicated blocking XREAD must deliver it (the ADR-0001 risk path).
    app.client.pause("emails").await.unwrap();
    let mut saw_paused = false;
    for _ in 0..5 {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(batch)) => {
                if batch.iter().any(|e| e.kind == EventKind::Paused) {
                    saw_paused = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(saw_paused, "live paused event delivered via XREAD");
    handle.shutdown().await;
}

#[tokio::test]
async fn redis_stats_overlay_renders() {
    let (_c, mut app) = seeded_app().await;
    press(&mut app, KeyCode::Char('i')).await;
    let text = render(&mut app, 120, 30);
    assert!(text.contains("Redis"), "redis modal title:\n{text}");
    assert!(text.contains("Version"), "version field:\n{text}");
    assert!(text.contains("Connected clients"), "clients field");
}

#[tokio::test]
async fn mouse_click_selects_then_opens_queue() {
    let (_c, mut app) = seeded_app().await;
    // Queue order is what the overview actually draws (sorted by name).
    let names: Vec<String> = app
        .visible_queues()
        .iter()
        .map(|q| q.name.clone())
        .collect();
    assert!(names.len() >= 2, "need ≥2 queues, got {}", names.len());

    // Render first so the click hit map reflects the drawn rows.
    let _ = render(&mut app, 120, 30);
    let region = *app
        .mouse_regions
        .iter()
        .find(|r| r.kind == HitKind::OverviewQueue)
        .expect("overview registers a clickable region");

    // The second visible row maps to queue index 1. A first click only moves the
    // cursor there (no double-click timer needed).
    let (x, y) = (region.area.x + 1, region.area.y + 1);
    click(&mut app, x, y).await;
    assert_eq!(app.overview_selected, 1, "click moved the cursor");
    assert_eq!(app.screen, Screen::Overview, "first click only selects");

    // A second click on the now-selected row opens it.
    let _ = render(&mut app, 120, 30);
    click(&mut app, x, y).await;
    assert_eq!(app.screen, Screen::Queue, "second click opened the queue");
    assert_eq!(
        app.queue_name.as_deref(),
        Some(names[1].as_str()),
        "opened the queue under the click"
    );
}

#[tokio::test]
async fn mouse_click_opens_a_job_row() {
    let (_c, mut app) = seeded_app().await;
    press(&mut app, KeyCode::Enter).await; // open emails (Latest tab has jobs)
    assert_eq!(app.screen, Screen::Queue);
    assert!(!app.visible_jobs().is_empty(), "emails has jobs");

    let _ = render(&mut app, 120, 30);
    let region = *app
        .mouse_regions
        .iter()
        .find(|r| r.kind == HitKind::Job)
        .expect("queue registers a clickable job region");

    // Row 0 is the default selection, so a single click on it opens the job —
    // this also exercises the bordered-table geometry (data rows start one row
    // below the in-block header).
    click(&mut app, region.area.x + 1, region.area.y).await;
    assert_eq!(
        app.screen,
        Screen::Job,
        "clicking the selected job row opened its detail"
    );
}

#[tokio::test]
async fn mouse_wheel_moves_overview_selection() {
    let (_c, mut app) = seeded_app().await;
    let _ = render(&mut app, 120, 30);
    assert_eq!(app.overview_selected, 0, "starts at the top");
    wheel(&mut app, true).await;
    assert_eq!(app.overview_selected, 1, "wheel down moves the cursor");
    wheel(&mut app, false).await;
    assert_eq!(app.overview_selected, 0, "wheel up moves back");
}

#[tokio::test]
async fn mouse_capture_defaults_on_and_toggles_in_header() {
    let (_c, mut app) = seeded_app().await;
    assert!(
        app.mouse_capture,
        "on by default (mainstream TUI posture); --no-mouse starts it off"
    );

    // Default-on suspends native selection, so the header surfaces the
    // Shift/⌥-drag escape hatch rather than a bare mode chip.
    let text = render(&mut app, 120, 30);
    assert!(
        text.contains("drag: select"),
        "header surfaces the text-selection escape hatch when captured:\n{text}"
    );
    assert!(
        !text.contains("mouse:off"),
        "no off-flag while captured:\n{text}"
    );

    app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL))
        .await;
    assert!(!app.mouse_capture, "Ctrl+O dropped capture");
    let text = render(&mut app, 120, 30);
    assert!(
        text.contains("mouse:off"),
        "header flags the non-default off mode — never silent:\n{text}"
    );
    assert!(
        !text.contains("drag: select"),
        "no selection hint once native selection is restored:\n{text}"
    );

    app.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL))
        .await;
    assert!(app.mouse_capture, "Ctrl+O turned capture back on");
    let text = render(&mut app, 120, 30);
    assert!(
        text.contains("drag: select"),
        "selection hint returns when captured again:\n{text}"
    );
}
