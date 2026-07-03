//! Application state and the main event loop.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use anyhow::Result;
use bullmq::{
    ActiveJobLock, BullClient, FlowNode, Job, JobCounts, JobScheduler, JobState, Metrics,
    MetricsKind, QueueEvent, QueueSummary, RateLimitStatus, RedisInfo, WorkerInfo,
};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures_util::StreamExt;
use ratatui::layout::Rect;
use ratatui::DefaultTerminal;
use serde_json::Value;

use crate::cli::Args;
use crate::state::{
    BulkAction, Command, EventScope, Field, FilterScope, FilterState, InputForm, InputKind, JobTab,
    Overlay, PaletteItem, PaletteState, PendingAction, Settings, StatusTab, WorkersTab,
};
use crate::ui;

/// Top-level screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Queue,
    Job,
    Schedulers,
    Workers,
    Events,
}

/// How the overview renders each queue's per-state counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewView {
    /// Numeric columns, one per state (the classic table).
    Table,
    /// A bull-board-style stacked, color-segmented bar per queue.
    Bars,
}

impl OverviewView {
    fn toggled(self) -> Self {
        match self {
            OverviewView::Table => OverviewView::Bars,
            OverviewView::Bars => OverviewView::Table,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OverviewView::Table => "table",
            OverviewView::Bars => "bars",
        }
    }
}

/// Sort mode for the overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewSort {
    Name,
    Total,
    Active,
    Waiting,
    Completed,
    Failed,
    Delayed,
}

impl OverviewSort {
    pub fn label(self) -> &'static str {
        match self {
            OverviewSort::Name => "name",
            OverviewSort::Total => "total",
            OverviewSort::Active => "active",
            OverviewSort::Waiting => "waiting",
            OverviewSort::Completed => "completed",
            OverviewSort::Failed => "failed",
            OverviewSort::Delayed => "delayed",
        }
    }

    fn next(self) -> Self {
        match self {
            OverviewSort::Name => OverviewSort::Total,
            OverviewSort::Total => OverviewSort::Active,
            OverviewSort::Active => OverviewSort::Waiting,
            OverviewSort::Waiting => OverviewSort::Completed,
            OverviewSort::Completed => OverviewSort::Failed,
            OverviewSort::Failed => OverviewSort::Delayed,
            OverviewSort::Delayed => OverviewSort::Name,
        }
    }
}

/// Which ordered collection a mouse click maps onto. The resolved index is an
/// index into the *same* slice the keyboard and renderer use (`visible_queues`,
/// `visible_jobs`, `flatten_flow`, …), so a click can never land on a row the
/// keyboard couldn't reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    OverviewQueue,
    Job,
    FlowNode,
    Scheduler,
    ActiveLock,
    Worker,
    Event,
}

/// A band of 1-row list entries drawn this frame, with the scroll offset and
/// item count needed to map a click back to an item index. Rebuilt every frame
/// by [`crate::ui::draw`]; pure (no clock, no effects), so `TestBackend` output
/// is deterministic.
#[derive(Debug, Clone, Copy)]
pub struct HitRegion {
    pub kind: HitKind,
    /// The data-rows rectangle (header row and borders already excluded).
    pub area: Rect,
    /// Index of the first visible row (the renderer's scroll offset).
    pub offset: usize,
    /// Total items in the list (so off-the-end clicks resolve to nothing).
    pub count: usize,
}

/// Scroll bounds of the job-detail body, recorded by the last render.
/// `max_scroll` = content height minus viewport height; `page` = viewport height.
/// Both are `0` until first render or when content fits without scrolling.
#[derive(Debug, Clone, Copy, Default)]
pub struct DetailView {
    pub max_scroll: u16,
    pub page: u16,
}

/// Clamp a vertical scroll delta to `[0, max]`. Pure; unit-testable without `App`.
pub(crate) fn clamp_scroll(current: u16, delta: isize, max: u16) -> u16 {
    (current as isize + delta).clamp(0, max as isize) as u16
}

impl HitRegion {
    /// The item index at `(col, row)`, or `None` when the point is outside the
    /// band or past the last item.
    pub fn index_at(&self, col: u16, row: u16) -> Option<usize> {
        let in_x = col >= self.area.x && col < self.area.x.saturating_add(self.area.width);
        let in_y = row >= self.area.y && row < self.area.y.saturating_add(self.area.height);
        if !in_x || !in_y {
            return None;
        }
        let idx = self.offset + (row - self.area.y) as usize;
        (idx < self.count).then_some(idx)
    }
}

/// First visible index for a `height`-row viewport that keeps `selected` on
/// screen. Shared by every list renderer and the mouse hit map.
pub fn list_offset(selected: usize, height: usize, count: usize) -> usize {
    if height == 0 || count == 0 {
        return 0;
    }
    let sel = selected.min(count - 1);
    if sel >= height {
        sel + 1 - height
    } else {
        0
    }
}

/// Application state.
pub struct App {
    pub client: BullClient,
    pub args: Args,
    pub settings: Settings,
    pub should_quit: bool,
    pub now: i64,

    pub status: String,
    pub last_error: Option<String>,
    /// Whether the most recent data fetch reached Redis. Drives the connection
    /// indicator (pulsing green when true, static red when false).
    pub connected: bool,
    /// Animation state. Enqueued by state transitions; advanced only by the
    /// real run loop via [`App::render_effects`], never in tests.
    pub animations: crate::fx::Animations,

    pub screen: Screen,
    pub overlay: Overlay,

    // overview
    pub queues: Vec<QueueSummary>,
    pub overview_selected: usize,
    pub overview_sort: OverviewSort,
    pub overview_view: OverviewView,
    pub overview_search: Option<String>,
    /// When set, only queues with jobs in this state are shown.
    pub overview_status_filter: Option<JobState>,

    // queue view
    pub queue_name: Option<String>,
    pub queue_summary: Option<QueueSummary>,
    pub status_tab: StatusTab,
    pub jobs: Vec<Job>,
    pub job_selected: usize,
    pub page: usize,
    /// Live `/` fuzzy filter over the job list (id + name + state).
    pub job_filter: Option<String>,
    /// Multi-selected job ids in the current queue/tab (survives pagination).
    pub job_selection: HashSet<String>,
    /// When set, range-select mode is active, anchored at this `visible_jobs`
    /// index; the live range runs from here to the cursor.
    pub range_anchor: Option<usize>,

    // schedulers
    pub schedulers: Vec<JobScheduler>,
    pub scheduler_selected: usize,
    /// Screen to return to when leaving the schedulers view.
    pub scheduler_return: Screen,

    // workers / busy
    pub workers_tab: WorkersTab,
    pub active_locks: Vec<ActiveJobLock>,
    pub active_selected: usize,
    pub workers: Vec<WorkerInfo>,
    pub worker_selected: usize,
    /// None = all queues; Some = a single queue.
    pub workers_scope: Option<String>,
    pub workers_error: Option<String>,
    pub workers_return: Screen,
    /// Live rate-limit / concurrency for the open queue (for the badge).
    pub rate_limit: Option<RateLimitStatus>,

    // events feed
    pub events: VecDeque<QueueEvent>,
    pub events_cap: usize,
    pub events_follow: bool,
    pub events_paused: bool,
    pub events_selected: usize,
    pub events_filter: Option<String>,
    pub events_scope: EventScope,
    pub events_return: Screen,
    /// Set when the scope changes; the run loop tears down + respawns the task.
    pub events_resubscribe: bool,
    pub events_total: u64,

    // job detail
    pub job: Option<Job>,
    pub job_tab: JobTab,
    pub job_logs: Vec<String>,
    pub job_flow: Option<FlowNode>,
    pub detail_scroll: u16,
    /// Scroll bounds of the detail body recorded by the last render; see
    /// [`DetailView`]. Consulted by the scroll keys/wheel to clamp movement.
    pub detail_view: DetailView,
    /// Cursor into the flattened flow tree ([`flatten_flow`]) on the Flow tab.
    /// `↑↓` move it; `Enter` jumps to the node under it. Kept in sync with the
    /// focused job whenever a job opens (see [`App::sync_flow_selection`]).
    pub flow_selected: usize,

    // overlays' data
    pub redis_info: Option<RedisInfo>,
    pub metrics: Option<(Metrics, Metrics)>, // (completed, failed)

    // mouse navigation (on by default; strictly additive to the keyboard)
    /// Whether the terminal is capturing mouse events (on by default). When
    /// true, native click-drag selection is suspended; `Shift`/`⌥`-drag restores
    /// it, `Ctrl+O` drops capture entirely. Seeded from `--no-mouse`.
    pub mouse_capture: bool,
    /// Clickable list bands recorded by the last [`crate::ui::draw`]. Consulted
    /// by [`App::on_mouse`] to map a click to the row the keyboard would act on.
    pub mouse_regions: Vec<HitRegion>,
}

/// Flatten a flow tree into preorder (depth-first, children in stored order),
/// carrying each node's depth. This is the single canonical traversal shared by
/// flow navigation ([`App::move_flow`], [`App::jump_to_selected_flow_node`]) and
/// the Flow-tab renderer (`crate::ui::job`), so the cursor index and the
/// rendered rows can never drift apart.
pub(crate) fn flatten_flow(root: &FlowNode) -> Vec<(usize, &FlowNode)> {
    fn walk<'a>(node: &'a FlowNode, depth: usize, out: &mut Vec<(usize, &'a FlowNode)>) {
        out.push((depth, node));
        for child in &node.children {
            walk(child, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

impl App {
    pub fn new(client: BullClient, args: Args) -> Self {
        let settings = Settings {
            poll_secs: args.poll,
            jobs_per_page: args.jobs_per_page.max(1),
            confirm_actions: !args.no_confirm,
            focus: 0,
        };
        // Mouse capture is on by default; `--no-mouse` starts it off.
        let mouse_on = !args.no_mouse;
        Self {
            client,
            args,
            settings,
            should_quit: false,
            now: crate::format::now_ms(),
            status: "Welcome to bulltui".to_string(),
            last_error: None,
            connected: true,
            animations: crate::fx::Animations::new(),
            screen: Screen::Overview,
            overlay: Overlay::None,
            queues: Vec::new(),
            overview_selected: 0,
            overview_sort: OverviewSort::Name,
            overview_view: OverviewView::Table,
            overview_search: None,
            overview_status_filter: None,
            queue_name: None,
            queue_summary: None,
            status_tab: StatusTab::Latest,
            jobs: Vec::new(),
            job_selected: 0,
            page: 0,
            job_filter: None,
            job_selection: HashSet::new(),
            range_anchor: None,
            schedulers: Vec::new(),
            scheduler_selected: 0,
            scheduler_return: Screen::Overview,
            workers_tab: WorkersTab::Busy,
            active_locks: Vec::new(),
            active_selected: 0,
            workers: Vec::new(),
            worker_selected: 0,
            workers_scope: None,
            workers_error: None,
            workers_return: Screen::Overview,
            rate_limit: None,
            events: VecDeque::new(),
            events_cap: 5000,
            events_follow: true,
            events_paused: false,
            events_selected: 0,
            events_filter: None,
            events_scope: EventScope::All,
            events_return: Screen::Overview,
            events_resubscribe: false,
            events_total: 0,
            job: None,
            job_tab: JobTab::Data,
            job_logs: Vec::new(),
            job_flow: None,
            detail_scroll: 0,
            detail_view: DetailView::default(),
            flow_selected: 0,
            redis_info: None,
            metrics: None,
            mouse_capture: mouse_on,
            mouse_regions: Vec::new(),
        }
    }

    // -- helpers ------------------------------------------------------------

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.last_error = None;
    }

    fn set_error(&mut self, msg: impl Into<String>) {
        self.last_error = Some(msg.into());
    }

    pub fn read_only(&self) -> bool {
        self.args.read_only
    }

    /// Queues filtered by the active search and sorted by the active sort.
    pub fn visible_queues(&self) -> Vec<&QueueSummary> {
        let mut v: Vec<&QueueSummary> = self
            .queues
            .iter()
            .filter(|q| match &self.overview_search {
                Some(s) => crate::fuzzy::Filter::parse(s).matches(&q.name),
                None => true,
            })
            .filter(|q| match self.overview_status_filter {
                Some(state) => q.counts.get(state) > 0,
                None => true,
            })
            .collect();
        use std::cmp::Reverse;
        match self.overview_sort {
            OverviewSort::Name => v.sort_by(|a, b| a.name.cmp(&b.name)),
            OverviewSort::Total => v.sort_by_key(|q| Reverse(q.total_jobs())),
            OverviewSort::Active => v.sort_by_key(|q| Reverse(q.counts.active)),
            OverviewSort::Waiting => v.sort_by_key(|q| Reverse(q.counts.waiting)),
            OverviewSort::Completed => v.sort_by_key(|q| Reverse(q.counts.completed)),
            OverviewSort::Failed => v.sort_by_key(|q| Reverse(q.counts.failed)),
            OverviewSort::Delayed => v.sort_by_key(|q| Reverse(q.counts.delayed)),
        }
        v
    }

    pub fn selected_queue_name(&self) -> Option<String> {
        self.visible_queues()
            .get(self.overview_selected)
            .map(|q| q.name.clone())
    }

    /// The jobs currently visible after applying the live `/` filter. The
    /// cursor (`job_selected`) and all selection logic index into this view.
    pub fn visible_jobs(&self) -> Vec<&Job> {
        match &self.job_filter {
            Some(q) => {
                let f = crate::fuzzy::Filter::parse(q);
                self.jobs
                    .iter()
                    .filter(|j| f.matches(&job_haystack(j)))
                    .collect()
            }
            None => self.jobs.iter().collect(),
        }
    }

    pub fn selected_job(&self) -> Option<&Job> {
        self.visible_jobs().get(self.job_selected).copied()
    }

    /// The job ids covered by the live range-select preview (anchor → cursor).
    fn range_preview_ids(&self) -> Vec<String> {
        let Some(anchor) = self.range_anchor else {
            return Vec::new();
        };
        let vis = self.visible_jobs();
        if vis.is_empty() {
            return Vec::new();
        }
        let cur = self.job_selected.min(vis.len() - 1);
        let (lo, hi) = (anchor.min(cur), anchor.max(cur));
        vis[lo..=hi.min(vis.len() - 1)]
            .iter()
            .map(|j| j.id.clone())
            .collect()
    }

    /// The durable selection plus any live range-select preview.
    pub fn effective_job_selection(&self) -> HashSet<String> {
        let mut set = self.job_selection.clone();
        set.extend(self.range_preview_ids());
        set
    }

    fn clear_selection(&mut self) {
        self.job_selection.clear();
        self.range_anchor = None;
    }

    fn toggle_select_current(&mut self) {
        self.range_anchor = None;
        if let Some(id) = self.selected_job().map(|j| j.id.clone()) {
            if !self.job_selection.remove(&id) {
                self.job_selection.insert(id);
            }
        }
    }

    fn toggle_range_mode(&mut self) {
        if self.range_anchor.is_some() {
            for id in self.range_preview_ids() {
                self.job_selection.insert(id);
            }
            self.range_anchor = None;
            self.flash("range selection committed");
        } else {
            self.range_anchor = Some(self.job_selected);
            self.flash("range select: move to extend, v to commit");
        }
    }

    /// Total count for the active status tab.
    pub fn current_status_count(&self) -> i64 {
        let Some(summary) = &self.queue_summary else {
            return 0;
        };
        match self.status_tab {
            StatusTab::Latest => summary.counts.total(),
            StatusTab::State(s) => summary.counts.get(s),
        }
    }

    pub fn page_count(&self) -> usize {
        match self.status_tab {
            StatusTab::Latest => 1,
            StatusTab::State(_) => {
                let total = self.current_status_count().max(0) as usize;
                total.div_ceil(self.settings.jobs_per_page).max(1)
            }
        }
    }

    // -- data loading -------------------------------------------------------

    /// Discover queues and load their summaries (overview refresh).
    pub async fn refresh_overview(&mut self) {
        self.now = crate::format::now_ms();
        let names = if self.args.queues.is_empty() {
            match self.client.discover_queues().await {
                Ok(n) => {
                    self.connected = true;
                    n
                }
                Err(e) => {
                    self.connected = false;
                    self.set_error(format!("discover queues: {e}"));
                    return;
                }
            }
        } else {
            self.args.queues.clone()
        };
        let mut summaries = Vec::with_capacity(names.len());
        let mut any_ok = false;
        for name in &names {
            match self.client.queue_summary(name).await {
                Ok(s) => {
                    any_ok = true;
                    summaries.push(s);
                }
                Err(e) => self.set_error(format!("summary {name}: {e}")),
            }
        }
        if !names.is_empty() {
            self.connected = any_ok;
        }
        // Shimmer on changed counts to signal a live update (skipped on first load).
        if !self.queues.is_empty() && overview_sig(&self.queues) != overview_sig(&summaries) {
            self.animations.live_update();
        }
        self.queues = summaries;
        let len = self.visible_queues().len();
        if self.overview_selected >= len {
            self.overview_selected = len.saturating_sub(1);
        }
        if self.last_error.is_none() {
            self.flash(format!("{} queue(s)", self.queues.len()));
        }
    }

    /// Open a queue, loading its summary and first page of jobs.
    pub async fn open_queue(&mut self, name: String) {
        self.queue_name = Some(name);
        self.status_tab = StatusTab::Latest;
        self.page = 0;
        self.job_selected = 0;
        self.job_filter = None;
        self.clear_selection();
        self.screen = Screen::Queue;
        self.animations.transition();
        self.reload_queue().await;
    }

    /// Reload the current queue's summary and the active page of jobs.
    pub async fn reload_queue(&mut self) {
        self.now = crate::format::now_ms();
        let Some(name) = self.queue_name.clone() else {
            return;
        };
        match self.client.queue_summary(&name).await {
            Ok(s) => {
                self.connected = true;
                // Shimmer when a poll changes the live counts (not on first open).
                if let Some(prev) = &self.queue_summary {
                    if prev.counts != s.counts {
                        self.animations.live_update();
                    }
                }
                self.queue_summary = Some(s);
            }
            Err(e) => {
                self.connected = false;
                self.set_error(format!("summary {name}: {e}"));
            }
        }
        self.rate_limit = self.client.rate_limit_status(&name).await.ok();
        self.load_jobs().await;
    }

    async fn load_jobs(&mut self) {
        let Some(name) = self.queue_name.clone() else {
            return;
        };
        let per = self.settings.jobs_per_page as isize;
        let result = match self.status_tab {
            StatusTab::Latest => {
                self.client
                    .get_jobs_latest(&name, &JobState::ALL, 0, per - 1)
                    .await
            }
            StatusTab::State(state) => {
                // Clamp page within range.
                let pc = self.page_count();
                if self.page >= pc {
                    self.page = pc.saturating_sub(1);
                }
                let start = (self.page as isize) * per;
                let end = start + per - 1;
                self.client.list_status_jobs(&name, state, start, end).await
            }
        };
        match result {
            Ok(jobs) => {
                self.jobs = jobs;
                let vis = self.visible_jobs().len();
                if self.job_selected >= vis {
                    self.job_selected = vis.saturating_sub(1);
                }
            }
            Err(e) => self.set_error(format!("load jobs: {e}")),
        }
    }

    /// Open the selected job's detail view.
    pub async fn open_job(&mut self) {
        let Some(job) = self.selected_job().cloned() else {
            return;
        };
        let Some(name) = self.queue_name.clone() else {
            return;
        };
        self.job = Some(job.clone());
        self.job_tab = JobTab::Data;
        self.detail_scroll = 0;
        self.screen = Screen::Job;
        self.animations.transition();

        // Logs.
        match self.client.job_logs(&name, &job.id, 0, -1).await {
            Ok(l) => self.job_logs = l.logs,
            Err(e) => {
                self.job_logs.clear();
                self.set_error(format!("logs: {e}"));
            }
        }
        // Flow (root → tree).
        self.job_flow = None;
        match self.client.find_flow_root(&name, &job.id).await {
            Ok(Some((rq, rid))) => {
                if let Ok(tree) = self.client.get_flow_tree(&rq, &rid, 6).await {
                    self.job_flow = tree;
                }
            }
            Ok(None) => {
                // Standalone job: still show itself as a single node.
                if let Ok(tree) = self.client.get_flow_tree(&name, &job.id, 6).await {
                    self.job_flow = tree;
                }
            }
            Err(e) => self.set_error(format!("flow: {e}")),
        }
        self.sync_flow_selection();
    }

    async fn reload_job(&mut self) {
        let (Some(name), Some(job)) = (self.queue_name.clone(), self.job.clone()) else {
            return;
        };
        match self.client.get_job(&name, &job.id).await {
            Ok(Some(j)) => {
                self.connected = true;
                self.job = Some(j);
            }
            Ok(None) => {
                self.connected = true;
                self.flash("job no longer exists");
                self.screen = Screen::Queue;
                self.animations.transition();
                self.reload_queue().await;
            }
            Err(e) => {
                self.connected = false;
                self.set_error(format!("reload job: {e}"));
            }
        }
    }

    /// Open the schedulers screen for `queue`.
    pub async fn open_schedulers(&mut self, queue: String) {
        self.scheduler_return = self.screen;
        self.queue_name = Some(queue);
        self.scheduler_selected = 0;
        self.screen = Screen::Schedulers;
        self.animations.transition();
        self.reload_schedulers().await;
    }

    pub async fn reload_schedulers(&mut self) {
        self.now = crate::format::now_ms();
        let Some(name) = self.queue_name.clone() else {
            return;
        };
        match self.client.list_job_schedulers(&name, 0, -1).await {
            Ok(s) => {
                self.connected = true;
                if self.scheduler_selected >= s.len() {
                    self.scheduler_selected = s.len().saturating_sub(1);
                }
                self.schedulers = s;
            }
            Err(e) => {
                self.connected = false;
                self.set_error(format!("schedulers: {e}"));
            }
        }
    }

    /// Open the workers / busy view (`scope = None` ⇒ all queues).
    pub async fn open_workers(&mut self, scope: Option<String>) {
        self.workers_return = self.screen;
        self.workers_scope = scope;
        self.workers_tab = WorkersTab::Busy;
        self.active_selected = 0;
        self.worker_selected = 0;
        self.screen = Screen::Workers;
        self.animations.transition();
        self.reload_workers().await;
    }

    pub async fn reload_workers(&mut self) {
        self.now = crate::format::now_ms();
        self.workers_error = None;
        let locks = match &self.workers_scope {
            Some(q) => self.client.list_active_jobs_with_locks(q).await,
            None => {
                let names: Vec<String> = self.queues.iter().map(|q| q.name.clone()).collect();
                self.client.list_active_jobs_all(&names).await
            }
        };
        match locks {
            Ok(l) => {
                self.connected = true;
                if self.active_selected >= l.len() {
                    self.active_selected = l.len().saturating_sub(1);
                }
                self.active_locks = l;
            }
            Err(e) => {
                self.connected = false;
                self.set_error(format!("busy: {e}"));
            }
        }
        // Roster via CLIENT LIST; surface permission errors inline.
        match self.client.list_workers().await {
            Ok(w) => {
                if self.worker_selected >= w.len() {
                    self.worker_selected = w.len().saturating_sub(1);
                }
                self.workers = w;
            }
            Err(e) => {
                self.workers.clear();
                self.workers_error = Some(format!("{e}"));
            }
        }
    }

    /// Open the live events feed with the given scope (run loop spawns the task).
    pub async fn open_events(&mut self, scope: EventScope) {
        self.events_return = self.screen;
        self.events_scope = scope;
        self.events.clear();
        self.events_selected = 0;
        self.events_follow = true;
        self.events_paused = false;
        self.events_resubscribe = true;
        self.screen = Screen::Events;
        self.animations.transition();
    }

    /// The determinism seam: the run loop AND tests both call this to push events
    /// into the ring. No socket here.
    pub fn ingest_events(&mut self, batch: Vec<QueueEvent>) {
        for ev in batch {
            self.events_total += 1;
            if self.events.len() >= self.events_cap {
                self.events.pop_front();
            }
            self.events.push_back(ev);
        }
        if self.events_follow && !self.events_paused {
            self.events_selected = self.filtered_event_count().saturating_sub(1);
        }
    }

    fn events_filter_obj(&self) -> Option<crate::fuzzy::Filter> {
        self.events_filter
            .as_deref()
            .map(crate::fuzzy::Filter::parse)
    }

    pub fn filtered_events(&self) -> Vec<&QueueEvent> {
        let f = self.events_filter_obj();
        self.events
            .iter()
            .filter(|e| match &f {
                Some(f) => f.matches(&event_haystack(e)),
                None => true,
            })
            .collect()
    }

    pub fn filtered_event_count(&self) -> usize {
        match self.events_filter_obj() {
            Some(f) => self
                .events
                .iter()
                .filter(|e| f.matches(&event_haystack(e)))
                .count(),
            None => self.events.len(),
        }
    }

    pub fn hidden_event_count(&self) -> usize {
        self.events.len() - self.filtered_event_count()
    }

    fn events_move(&mut self, delta: isize) {
        let n = self.filtered_event_count();
        if n == 0 {
            return;
        }
        let cur = self.events_selected.min(n - 1) as isize;
        let next = (cur + delta).clamp(0, n as isize - 1) as usize;
        self.events_selected = next;
        self.events_follow = next + 1 >= n; // follow only when pinned to the tail
    }

    fn events_jump_failure(&mut self, forward: bool) {
        let filtered = self.filtered_events();
        let n = filtered.len();
        if n == 0 {
            return;
        }
        let cur = self.events_selected.min(n - 1);
        let range: Vec<usize> = if forward {
            (cur + 1..n).collect()
        } else {
            (0..cur).rev().collect()
        };
        for i in range {
            if filtered[i].kind.is_failure() {
                self.events_selected = i;
                self.events_follow = i + 1 >= n;
                return;
            }
        }
    }

    async fn open_event_from_selected(&mut self) {
        let target = {
            let filtered = self.filtered_events();
            filtered
                .get(self.events_selected)
                .and_then(|ev| ev.job_id.clone().map(|id| (ev.queue.clone(), id)))
        };
        if let Some((queue, id)) = target {
            self.open_job_by_id(queue, id).await;
        }
    }

    async fn events_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.events_move(1),
            KeyCode::Char('k') | KeyCode::Up => self.events_move(-1),
            KeyCode::Char('g') | KeyCode::Home => {
                self.events_selected = 0;
                self.events_follow = false;
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.events_selected = self.filtered_event_count().saturating_sub(1);
                self.events_follow = true;
            }
            KeyCode::Char('f') => self.events_follow = !self.events_follow,
            KeyCode::Char('p') | KeyCode::Char(' ') => {
                self.events_paused = !self.events_paused;
                self.flash(if self.events_paused {
                    "events paused"
                } else {
                    "events following"
                });
            }
            KeyCode::Char('/') => self.open_filter(FilterScope::Events),
            KeyCode::Char('s') => {
                self.events_scope = match (&self.events_scope, &self.queue_name) {
                    (EventScope::All, Some(q)) => EventScope::Queue(q.clone()),
                    _ => EventScope::All,
                };
                self.events.clear();
                self.events_selected = 0;
                self.events_resubscribe = true;
                let label = self.events_scope.label();
                self.flash(format!("events scope: {label}"));
            }
            KeyCode::Char('n') => self.events_jump_failure(true),
            KeyCode::Char('N') => self.events_jump_failure(false),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                self.open_event_from_selected().await
            }
            KeyCode::Left | KeyCode::Char('h') => self.back(),
            _ => {}
        }
    }

    async fn load_redis_info(&mut self) {
        match self.client.redis_info().await {
            Ok(info) => self.redis_info = Some(info),
            Err(e) => self.set_error(format!("redis info: {e}")),
        }
    }

    async fn load_metrics(&mut self) {
        let Some(name) = self.queue_name.clone() else {
            self.set_error("open a queue to view metrics");
            return;
        };
        let completed = self
            .client
            .metrics(&name, MetricsKind::Completed, 0, -1)
            .await;
        let failed = self.client.metrics(&name, MetricsKind::Failed, 0, -1).await;
        match (completed, failed) {
            (Ok(c), Ok(f)) => self.metrics = Some((c, f)),
            (Err(e), _) | (_, Err(e)) => self.set_error(format!("metrics: {e}")),
        }
    }

    /// Refresh whatever the active screen shows.
    async fn refresh_active(&mut self) {
        self.now = crate::format::now_ms();
        match self.screen {
            Screen::Overview => self.refresh_overview().await,
            Screen::Schedulers => self.reload_schedulers().await,
            Screen::Workers => self.reload_workers().await,
            Screen::Events => {} // push-driven; nothing to fetch on a tick
            Screen::Queue => self.reload_queue().await,
            Screen::Job => {
                self.reload_job().await;
                // refresh logs too
                if let (Some(name), Some(job)) = (self.queue_name.clone(), self.job.clone()) {
                    if let Ok(l) = self.client.job_logs(&name, &job.id, 0, -1).await {
                        self.job_logs = l.logs;
                    }
                }
            }
        }
        if matches!(self.overlay, Overlay::RedisStats) {
            self.load_redis_info().await;
        }
        if matches!(self.overlay, Overlay::Metrics) {
            self.load_metrics().await;
        }
    }

    /// Refresh the data backing the active screen after a write action.
    async fn refresh_after_action(&mut self) {
        match self.screen {
            Screen::Overview => self.refresh_overview().await,
            Screen::Schedulers => self.reload_schedulers().await,
            Screen::Workers => self.reload_workers().await,
            Screen::Events => {} // push-driven
            Screen::Queue | Screen::Job => self.reload_queue().await,
        }
    }

    // -- navigation ---------------------------------------------------------

    fn back(&mut self) {
        match self.screen {
            Screen::Job => {
                self.screen = Screen::Queue;
                self.animations.transition();
            }
            Screen::Queue => {
                self.screen = Screen::Overview;
                self.queue_name = None;
                self.animations.transition();
            }
            Screen::Schedulers => {
                self.screen = self.scheduler_return;
                self.animations.transition();
            }
            Screen::Workers => {
                self.screen = self.workers_return;
                self.animations.transition();
            }
            Screen::Events => {
                self.screen = self.events_return;
                self.animations.transition();
            }
            Screen::Overview => {}
        }
    }

    fn move_overview(&mut self, delta: isize) {
        let len = self.visible_queues().len();
        if len == 0 {
            return;
        }
        let next = (self.overview_selected as isize + delta).clamp(0, len as isize - 1);
        self.overview_selected = next as usize;
    }

    fn move_job(&mut self, delta: isize) {
        let len = self.visible_jobs().len();
        if len == 0 {
            return;
        }
        let next = (self.job_selected as isize + delta).clamp(0, len as isize - 1);
        self.job_selected = next as usize;
    }

    /// Clamp-move a selection cursor over a list of `len` items, mirroring
    /// [`Self::move_overview`]. Shared by the key handlers and mouse wheel so
    /// schedulers / busy / roster all scroll identically. Returns the new index.
    fn clamp_move(cur: usize, delta: isize, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        (cur as isize + delta).clamp(0, len as isize - 1) as usize
    }

    fn move_scheduler(&mut self, delta: isize) {
        self.scheduler_selected =
            Self::clamp_move(self.scheduler_selected, delta, self.schedulers.len());
    }

    fn move_active(&mut self, delta: isize) {
        self.active_selected =
            Self::clamp_move(self.active_selected, delta, self.active_locks.len());
    }

    fn move_worker(&mut self, delta: isize) {
        self.worker_selected = Self::clamp_move(self.worker_selected, delta, self.workers.len());
    }

    /// Number of nodes in the current flow tree (0 when there's no flow).
    fn flow_len(&self) -> usize {
        self.job_flow
            .as_ref()
            .map(|r| flatten_flow(r).len())
            .unwrap_or(0)
    }

    /// Move the Flow-tab cursor, clamped to the tree. No-op without a tree.
    fn move_flow(&mut self, delta: isize) {
        let len = self.flow_len();
        if len == 0 {
            return;
        }
        let next = (self.flow_selected as isize + delta).clamp(0, len as isize - 1);
        self.flow_selected = next as usize;
    }

    /// Move the detail-body scroll by `delta` lines, clamped to the content
    /// bounds the last render recorded ([`DetailView::max_scroll`]) so a short
    /// payload can't be scrolled off into empty space.
    fn scroll_detail(&mut self, delta: isize) {
        self.detail_scroll = clamp_scroll(self.detail_scroll, delta, self.detail_view.max_scroll);
    }

    /// A PageUp/PageDown step: a viewport-worth of lines minus one for
    /// continuity, at least one. Derived from the height the last render recorded.
    fn page_step(&self) -> isize {
        (self.detail_view.page.saturating_sub(1)).max(1) as isize
    }

    /// Land the Flow-tab cursor on the currently-focused job within the freshly
    /// fetched tree (so it starts on the `▶` node). Falls back to the root.
    fn sync_flow_selection(&mut self) {
        let here = self
            .job
            .as_ref()
            .map(|j| j.id.clone())
            .zip(self.queue_name.clone());
        self.flow_selected = match (&self.job_flow, here) {
            (Some(root), Some((id, queue))) => flatten_flow(root)
                .iter()
                .position(|(_, n)| n.job.id == id && n.queue_name == queue)
                .unwrap_or(0),
            _ => 0,
        };
    }

    /// Open the flow node under the cursor as the new focused job, staying on
    /// the Flow tab so the user can keep drilling. No-op when the cursor is
    /// already on the focused job (avoids a redundant fetch).
    async fn jump_to_selected_flow_node(&mut self) {
        let target = self.job_flow.as_ref().and_then(|root| {
            flatten_flow(root)
                .get(self.flow_selected)
                .map(|(_, n)| (n.queue_name.clone(), n.job.id.clone()))
        });
        let Some((queue, id)) = target else {
            return;
        };
        let already_here = self.queue_name.as_deref() == Some(queue.as_str())
            && self.job.as_ref().map(|j| j.id.as_str()) == Some(id.as_str());
        if already_here {
            return;
        }
        self.open_job_by_id(queue, id).await;
        self.job_tab = JobTab::Flow;
    }

    fn cycle_tab(&mut self, forward: bool) {
        let tabs = StatusTab::all();
        let idx = tabs.iter().position(|t| *t == self.status_tab).unwrap_or(0);
        let n = tabs.len();
        let next = if forward {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        self.status_tab = tabs[next];
        self.page = 0;
        self.job_selected = 0;
        self.job_filter = None;
        self.clear_selection();
    }

    fn cycle_job_tab(&mut self, forward: bool) {
        let tabs = JobTab::all();
        let idx = tabs.iter().position(|t| *t == self.job_tab).unwrap_or(0);
        let n = tabs.len();
        let next = if forward {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        self.job_tab = tabs[next];
        self.detail_scroll = 0;
    }

    // -- mouse navigation ---------------------------------------------------

    /// Flip terminal mouse capture. The run loop reconciles the real terminal to
    /// this flag; we only flash so the change is never silent (the active mode
    /// also shows in the header). The keyboard keeps working in either mode.
    fn toggle_mouse_capture(&mut self) {
        self.mouse_capture = !self.mouse_capture;
        if self.mouse_capture {
            self.flash("mouse capture ON — click a row to select, click again to open · Shift/⌥-drag to select text");
        } else {
            self.flash("mouse capture OFF — native click-drag text selection restored");
        }
    }

    /// Route a mouse event. Additive to the keyboard: clicks select / open and
    /// the wheel scrolls, all through the same state the keys drive. Overlays
    /// stay keyboard-only, so mouse input is ignored while one is open. Public
    /// so tests can inject synthetic `MouseEvent`s after a render (no terminal).
    pub async fn on_mouse(&mut self, me: MouseEvent) {
        if !self.overlay.is_none() {
            return;
        }
        match me.kind {
            MouseEventKind::Down(MouseButton::Left) => self.mouse_click(me.column, me.row).await,
            MouseEventKind::ScrollDown => self.mouse_scroll(1),
            MouseEventKind::ScrollUp => self.mouse_scroll(-1),
            _ => {}
        }
    }

    /// The (kind, index) of the list row drawn under `(col, row)` last frame.
    fn mouse_hit(&self, col: u16, row: u16) -> Option<(HitKind, usize)> {
        self.mouse_regions
            .iter()
            .find_map(|r| r.index_at(col, row).map(|i| (r.kind, i)))
    }

    /// Left-click: a click on a different row moves the cursor there; a click on
    /// the row already under the cursor activates it (open / drill in). This
    /// two-step model needs no double-click timer, so it stays deterministic.
    async fn mouse_click(&mut self, col: u16, row: u16) {
        let Some((kind, idx)) = self.mouse_hit(col, row) else {
            return;
        };
        match kind {
            HitKind::OverviewQueue => {
                if self.overview_selected == idx {
                    if let Some(name) = self.selected_queue_name() {
                        self.open_queue(name).await;
                    }
                } else {
                    self.overview_selected = idx;
                }
            }
            HitKind::Job => {
                if self.job_selected == idx {
                    self.open_job().await;
                } else {
                    self.job_selected = idx;
                }
            }
            HitKind::FlowNode => {
                if self.flow_selected == idx {
                    self.jump_to_selected_flow_node().await;
                } else {
                    self.flow_selected = idx;
                }
            }
            // Schedulers and Roster have no per-row drill-in; click only selects.
            HitKind::Scheduler => self.scheduler_selected = idx,
            HitKind::Worker => self.worker_selected = idx,
            HitKind::ActiveLock => {
                if self.active_selected == idx {
                    if let Some(lock) = self.active_locks.get(idx) {
                        let (q, id) = (lock.queue.clone(), lock.job.id.clone());
                        self.open_job_by_id(q, id).await;
                    }
                } else {
                    self.active_selected = idx;
                }
            }
            HitKind::Event => {
                if self.events_selected == idx {
                    self.open_event_from_selected().await;
                } else {
                    self.events_selected = idx;
                    self.events_follow = idx + 1 >= self.filtered_event_count();
                }
            }
        }
    }

    /// Wheel: scroll the active screen's primary list / body by moving the same
    /// cursor (or detail scroll) the arrow keys drive.
    fn mouse_scroll(&mut self, delta: isize) {
        const WHEEL_LINES: u16 = 3;
        match self.screen {
            Screen::Overview => self.move_overview(delta),
            Screen::Queue => self.move_job(delta),
            Screen::Schedulers => self.move_scheduler(delta),
            Screen::Workers => match self.workers_tab {
                WorkersTab::Busy => self.move_active(delta),
                WorkersTab::Roster => self.move_worker(delta),
            },
            Screen::Events => self.events_move(delta),
            Screen::Job => {
                if self.job_tab == JobTab::Flow {
                    self.move_flow(delta);
                } else {
                    self.scroll_detail(delta * WHEEL_LINES as isize);
                }
            }
        }
    }

    // -- action execution ---------------------------------------------------

    /// Ask for confirmation (or execute immediately if confirmations are off).
    async fn request(&mut self, action: PendingAction) {
        if self.read_only() {
            self.set_error("read-only mode: writes are disabled");
            return;
        }
        if self.settings.confirm_actions {
            self.overlay = Overlay::Confirm(action);
        } else {
            self.execute(action).await;
        }
    }

    async fn execute(&mut self, action: PendingAction) {
        // Bulk actions loop over the selection and report partial failures by
        // name (never a silent swallow).
        if let PendingAction::BulkJobs { queue, action, ids } = &action {
            let (mut ok, mut failed) = (0usize, Vec::<String>::new());
            for id in ids {
                let r = match action {
                    BulkAction::Retry => self.client.retry_job(queue, id).await,
                    BulkAction::Remove => self.client.clean_job(queue, id).await,
                    BulkAction::Promote => self.client.promote_job(queue, id).await,
                };
                match r {
                    Ok(_) => ok += 1,
                    Err(_) => failed.push(id.clone()),
                }
            }
            if failed.is_empty() {
                self.flash(format!("{} {ok} job(s)", action.past()));
            } else {
                let shown: Vec<&str> = failed.iter().take(5).map(|s| s.as_str()).collect();
                self.set_error(format!(
                    "{} {ok}/{} — {} failed: {}{}",
                    action.past(),
                    ok + failed.len(),
                    failed.len(),
                    shown.join(", "),
                    if failed.len() > 5 { ", …" } else { "" },
                ));
            }
            self.clear_selection();
            self.refresh_after_action().await;
            return;
        }

        let result: bullmq::Result<String> = match &action {
            PendingAction::PauseQueue(q) => {
                self.client.pause(q).await.map(|_| format!("paused {q}"))
            }
            PendingAction::ResumeQueue(q) => {
                self.client.resume(q).await.map(|_| format!("resumed {q}"))
            }
            PendingAction::PauseAll => self
                .client
                .pause_all()
                .await
                .map(|n| format!("paused {n} queue(s)")),
            PendingAction::ResumeAll => self
                .client
                .resume_all()
                .await
                .map(|n| format!("resumed {n} queue(s)")),
            PendingAction::EmptyQueue(q) => self
                .client
                .empty(q)
                .await
                .map(|n| format!("emptied {q} ({n} jobs)")),
            PendingAction::ObliterateQueue(q) => self
                .client
                .obliterate(q)
                .await
                .map(|_| format!("obliterated {q}")),
            PendingAction::CleanStatus(q, s) => self
                .client
                .clean(q, *s, 5000, 0)
                .await
                .map(|ids| format!("cleaned {} {} job(s)", ids.len(), s.status_str())),
            PendingAction::RetryAll(q, s) => self
                .client
                .retry_all(q, *s)
                .await
                .map(|n| format!("retried {n} {} job(s)", s.status_str())),
            PendingAction::PromoteAll(q) => self
                .client
                .promote_all(q)
                .await
                .map(|n| format!("promoted {n} delayed job(s)")),
            PendingAction::RetryJob(q, id) => self
                .client
                .retry_job(q, id)
                .await
                .map(|_| format!("retried job {id}")),
            PendingAction::PromoteJob(q, id) => self
                .client
                .promote_job(q, id)
                .await
                .map(|_| format!("promoted job {id}")),
            PendingAction::RemoveJob(q, id) => self
                .client
                .clean_job(q, id)
                .await
                .map(|_| format!("removed job {id}")),
            PendingAction::DuplicateJob(q, id) => self
                .client
                .duplicate_job(q, id)
                .await
                .map(|new| format!("duplicated job {id} -> {new}")),
            PendingAction::TriggerScheduler(q, id) => self
                .client
                .trigger_scheduler(q, id)
                .await
                .map(|_| format!("triggered scheduler {id}")),
            PendingAction::RemoveScheduler(q, id) => {
                self.client.remove_job_scheduler(q, id).await.map(|ok| {
                    if ok {
                        format!("removed scheduler {id}")
                    } else {
                        format!("no scheduler {id}")
                    }
                })
            }
            PendingAction::Reschedule(q, id, ms) => self
                .client
                .change_delay(q, id, *ms)
                .await
                .map(|_| format!("rescheduled job {id}")),
            PendingAction::Reprioritize(q, id, p) => self
                .client
                .change_priority(q, id, *p, false)
                .await
                .map(|_| format!("set job {id} priority to {p}")),
            PendingAction::BulkJobs { .. } => unreachable!("bulk handled above"),
        };
        match result {
            Ok(msg) => self.flash(msg),
            Err(e) => self.set_error(format!("action failed: {e}")),
        }
        // Refresh affected views.
        self.refresh_after_action().await;
    }

    async fn submit_input(&mut self, form: InputForm) {
        let result: bullmq::Result<String> = match &form.kind {
            InputKind::SetConcurrency { queue } => {
                match form.fields[0].value.trim().parse::<i64>() {
                    Ok(n) => self
                        .client
                        .set_global_concurrency(queue, n)
                        .await
                        .map(|_| format!("set concurrency for {queue} to {n}")),
                    Err(_) => Err(bullmq::Error::InvalidArgument(
                        "concurrency must be an integer".into(),
                    )),
                }
            }
            InputKind::UpdateData { queue, id } => {
                match serde_json::from_str::<Value>(&form.fields[0].value) {
                    Ok(v) => self
                        .client
                        .update_job_data(queue, id, &v)
                        .await
                        .map(|_| format!("updated data for job {id}")),
                    Err(e) => Err(bullmq::Error::InvalidArgument(format!("invalid JSON: {e}"))),
                }
            }
            InputKind::Reschedule { queue, id } => {
                match crate::format::parse_delay(&form.fields[0].value) {
                    Some(ms) => self
                        .client
                        .change_delay(queue, id, ms)
                        .await
                        .map(|_| format!("rescheduled job {id}")),
                    None => Err(bullmq::Error::InvalidArgument(
                        "delay must be ms or like 30s / 5m / 2h".into(),
                    )),
                }
            }
            InputKind::Reprioritize { queue, id } => {
                match form.fields[0].value.trim().parse::<i64>() {
                    Ok(p) => self
                        .client
                        .change_priority(queue, id, p, false)
                        .await
                        .map(|_| format!("set job {id} priority to {p}")),
                    Err(_) => Err(bullmq::Error::InvalidArgument(
                        "priority must be an integer".into(),
                    )),
                }
            }
            InputKind::AddJob { queue } => {
                let name = form.fields[0].value.trim().to_string();
                let data = if form.fields[1].value.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    match serde_json::from_str::<Value>(&form.fields[1].value) {
                        Ok(v) => v,
                        Err(e) => {
                            self.set_error(format!("invalid data JSON: {e}"));
                            self.overlay = Overlay::Input(form);
                            return;
                        }
                    }
                };
                let opts = if form.fields[2].value.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    match serde_json::from_str::<Value>(&form.fields[2].value) {
                        Ok(v) => v,
                        Err(e) => {
                            self.set_error(format!("invalid opts JSON: {e}"));
                            self.overlay = Overlay::Input(form);
                            return;
                        }
                    }
                };
                if name.is_empty() {
                    self.set_error("job name is required");
                    self.overlay = Overlay::Input(form);
                    return;
                }
                self.client
                    .add_job(queue, &name, &data, &opts)
                    .await
                    .map(|id| format!("added job {id} to {queue}"))
            }
        };
        match result {
            Ok(msg) => self.flash(msg),
            Err(e) => self.set_error(format!("{e}")),
        }
        self.refresh_after_action().await;
    }

    // -- key routing --------------------------------------------------------

    /// Handle a key event. Public so tests can drive the app with `TestBackend`.
    pub async fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        // Overlay handling takes precedence.
        match &self.overlay {
            Overlay::Input(_) => return self.input_key(key).await,
            Overlay::Confirm(_) => return self.confirm_key(key).await,
            Overlay::Palette(_) => return self.palette_key(key).await,
            Overlay::Filter(_) => return self.filter_key(key),
            Overlay::Help | Overlay::RedisStats | Overlay::Metrics | Overlay::Settings => {
                return self.overlay_key(key).await
            }
            Overlay::None => {}
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Global keys.
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('c') if ctrl => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.overlay = Overlay::Help;
                return;
            }
            KeyCode::Char('i') => {
                self.load_redis_info().await;
                self.overlay = Overlay::RedisStats;
                return;
            }
            KeyCode::Char(',') => {
                self.settings.focus = 0;
                self.overlay = Overlay::Settings;
                return;
            }
            KeyCode::Char('r') if ctrl => {
                self.refresh_active().await;
                return;
            }
            KeyCode::Char('o') if ctrl => {
                self.toggle_mouse_capture();
                return;
            }
            KeyCode::Char(':') => {
                self.open_palette();
                return;
            }
            KeyCode::Char('w') => {
                let scope = match self.screen {
                    Screen::Queue | Screen::Job | Screen::Schedulers => self.queue_name.clone(),
                    _ => None,
                };
                self.open_workers(scope).await;
                return;
            }
            KeyCode::Char('E') => {
                let scope = match (self.screen, &self.queue_name) {
                    (Screen::Queue | Screen::Job, Some(q)) => EventScope::Queue(q.clone()),
                    _ => EventScope::All,
                };
                self.open_events(scope).await;
                return;
            }
            KeyCode::Esc => {
                // Esc first clears any active filter/selection on the current
                // screen (a quick clear with no dialog); otherwise navigates back.
                if self.screen == Screen::Overview
                    && (self.overview_search.is_some() || self.overview_status_filter.is_some())
                {
                    self.overview_search = None;
                    self.overview_status_filter = None;
                    self.overview_selected = 0;
                    self.flash("search & filter cleared");
                } else if self.screen == Screen::Queue
                    && (self.job_filter.is_some()
                        || !self.job_selection.is_empty()
                        || self.range_anchor.is_some())
                {
                    self.job_filter = None;
                    self.clear_selection();
                    self.job_selected = 0;
                    self.flash("filter & selection cleared");
                } else {
                    self.back();
                }
                return;
            }
            _ => {}
        }

        match self.screen {
            Screen::Overview => self.overview_key(key).await,
            Screen::Queue => self.queue_key(key).await,
            Screen::Job => self.job_key(key).await,
            Screen::Schedulers => self.schedulers_key(key).await,
            Screen::Workers => self.workers_key(key).await,
            Screen::Events => self.events_key(key).await,
        }
    }

    async fn overview_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_overview(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_overview(-1),
            KeyCode::Char('g') | KeyCode::Home => self.overview_selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.overview_selected = self.visible_queues().len().saturating_sub(1)
            }
            KeyCode::Char('r') => self.refresh_overview().await,
            KeyCode::Char('s') => self.overview_sort = self.overview_sort.next(),
            KeyCode::Char('v') => {
                self.overview_view = self.overview_view.toggled();
                self.flash(format!("overview: {} view", self.overview_view.label()));
            }
            KeyCode::Char('f') => {
                // Cycle: None -> each state with jobs -> None.
                let order: Vec<Option<JobState>> = std::iter::once(None)
                    .chain(JobState::ALL.iter().map(|s| Some(*s)))
                    .collect();
                let idx = order
                    .iter()
                    .position(|f| *f == self.overview_status_filter)
                    .unwrap_or(0);
                self.overview_status_filter = order[(idx + 1) % order.len()];
                self.overview_selected = 0;
            }
            KeyCode::Char('/') => self.open_filter(FilterScope::Overview),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if let Some(name) = self.selected_queue_name() {
                    self.open_queue(name).await;
                }
            }
            KeyCode::Char('P') => self.request(PendingAction::PauseAll).await,
            KeyCode::Char('R') => self.request(PendingAction::ResumeAll).await,
            KeyCode::Char('S') => {
                if let Some(name) = self.selected_queue_name() {
                    self.open_schedulers(name).await;
                }
            }
            _ => {}
        }
    }

    async fn queue_key(&mut self, key: KeyEvent) {
        let qn = self.queue_name.clone();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_job(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_job(-1),
            KeyCode::Char('g') | KeyCode::Home => self.job_selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.job_selected = self.visible_jobs().len().saturating_sub(1)
            }
            KeyCode::Char('/') => self.open_filter(FilterScope::QueueJobs),
            KeyCode::Char(' ') => self.toggle_select_current(),
            KeyCode::Char('v') => self.toggle_range_mode(),
            KeyCode::Tab | KeyCode::Char('L') => {
                self.cycle_tab(true);
                self.load_jobs().await;
                self.animations.tab_switch();
            }
            KeyCode::BackTab | KeyCode::Char('H') => {
                self.cycle_tab(false);
                self.load_jobs().await;
                self.animations.tab_switch();
            }
            KeyCode::Char(']') => {
                if self.page + 1 < self.page_count() {
                    self.page += 1;
                    self.job_selected = 0;
                    self.range_anchor = None;
                    self.load_jobs().await;
                }
            }
            KeyCode::Char('[') => {
                if self.page > 0 {
                    self.page -= 1;
                    self.job_selected = 0;
                    self.range_anchor = None;
                    self.load_jobs().await;
                }
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.open_job().await,
            KeyCode::Left | KeyCode::Char('h') => self.back(),
            KeyCode::Char('m') => {
                self.load_metrics().await;
                self.overlay = Overlay::Metrics;
            }
            // queue-level actions
            KeyCode::Char('p') => {
                if let (Some(q), Some(summary)) = (qn.clone(), &self.queue_summary) {
                    let action = if summary.is_paused {
                        PendingAction::ResumeQueue(q)
                    } else {
                        PendingAction::PauseQueue(q)
                    };
                    self.request(action).await;
                }
            }
            KeyCode::Char('e') => {
                if let Some(q) = qn.clone() {
                    self.request(PendingAction::EmptyQueue(q)).await;
                }
            }
            KeyCode::Char('o') => {
                if let Some(q) = qn.clone() {
                    self.request(PendingAction::ObliterateQueue(q)).await;
                }
            }
            KeyCode::Char('c') => {
                if let (Some(q), StatusTab::State(s)) = (qn.clone(), self.status_tab) {
                    self.request(PendingAction::CleanStatus(q, s)).await;
                } else {
                    self.set_error("select a specific status tab to clean");
                }
            }
            KeyCode::Char('A') => {
                if let (Some(q), StatusTab::State(s)) = (qn.clone(), self.status_tab) {
                    if matches!(s, JobState::Failed | JobState::Completed) {
                        self.request(PendingAction::RetryAll(q, s)).await;
                    } else {
                        self.set_error("retry-all only applies to failed/completed tabs");
                    }
                }
            }
            KeyCode::Char('M') => {
                if let Some(q) = qn.clone() {
                    self.request(PendingAction::PromoteAll(q)).await;
                }
            }
            KeyCode::Char('a') => {
                if let Some(q) = qn.clone() {
                    self.open_add_job_form(q);
                }
            }
            KeyCode::Char('C') => {
                if let Some(q) = qn.clone() {
                    self.open_concurrency_form(q);
                }
            }
            KeyCode::Char('S') => {
                if let Some(q) = qn.clone() {
                    self.open_schedulers(q).await;
                }
            }
            KeyCode::Char('R') => {
                if let (Some(q), StatusTab::State(JobState::Delayed)) =
                    (qn.clone(), self.status_tab)
                {
                    if let Some(job) = self.selected_job().cloned() {
                        self.open_reschedule_form(q, job.id, job.delay);
                    }
                } else {
                    self.set_error("reschedule applies to the Delayed tab");
                }
            }
            KeyCode::Char('#') => {
                if let (Some(q), StatusTab::State(JobState::Prioritized)) =
                    (qn.clone(), self.status_tab)
                {
                    if let Some(job) = self.selected_job().cloned() {
                        self.open_reprioritize_form(q, job.id, job.priority);
                    }
                } else {
                    self.set_error("re-prioritize applies to the Prioritized tab");
                }
            }
            // per-job actions
            _ => self.job_action_key(key).await,
        }
    }

    async fn job_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
                self.cycle_job_tab(true);
                self.animations.tab_switch();
            }
            KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => {
                self.cycle_job_tab(false);
                self.animations.tab_switch();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                // On the Flow tab the vertical keys move the tree cursor; on
                // every other tab they scroll the detail body (bounded).
                if self.job_tab == JobTab::Flow {
                    self.move_flow(1);
                } else {
                    self.scroll_detail(1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.job_tab == JobTab::Flow {
                    self.move_flow(-1);
                } else {
                    self.scroll_detail(-1);
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if self.job_tab == JobTab::Flow {
                    self.flow_selected = 0;
                } else {
                    self.detail_scroll = 0;
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if self.job_tab == JobTab::Flow {
                    self.flow_selected = self.flow_len().saturating_sub(1);
                } else {
                    self.detail_scroll = self.detail_view.max_scroll;
                }
            }
            KeyCode::PageDown => {
                let step = self.page_step();
                if self.job_tab == JobTab::Flow {
                    self.move_flow(step);
                } else {
                    self.scroll_detail(step);
                }
            }
            KeyCode::PageUp => {
                let step = self.page_step();
                if self.job_tab == JobTab::Flow {
                    self.move_flow(-step);
                } else {
                    self.scroll_detail(-step);
                }
            }
            // Enter drills into the selected flow node (Flow tab only). `→`/`l`
            // are left to tab-cycling, so Enter is the jump key.
            KeyCode::Enter => {
                if self.job_tab == JobTab::Flow {
                    self.jump_to_selected_flow_node().await;
                }
            }
            KeyCode::Char('y') => self.copy_current_tab(),
            _ => self.job_action_key(key).await,
        }
    }

    async fn schedulers_key(&mut self, key: KeyEvent) {
        let q = self.queue_name.clone();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_scheduler(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_scheduler(-1),
            KeyCode::Char('g') | KeyCode::Home => self.scheduler_selected = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.scheduler_selected = self.schedulers.len().saturating_sub(1)
            }
            KeyCode::Char('r') => self.reload_schedulers().await,
            KeyCode::Char('t') => {
                let id = self
                    .schedulers
                    .get(self.scheduler_selected)
                    .map(|s| s.id.clone());
                if let (Some(q), Some(id)) = (q, id) {
                    self.request(PendingAction::TriggerScheduler(q, id)).await;
                }
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                let id = self
                    .schedulers
                    .get(self.scheduler_selected)
                    .map(|s| s.id.clone());
                if let (Some(q), Some(id)) = (q, id) {
                    self.request(PendingAction::RemoveScheduler(q, id)).await;
                }
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => self.back(),
            _ => {}
        }
    }

    async fn workers_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.workers_tab = match self.workers_tab {
                    WorkersTab::Busy => WorkersTab::Roster,
                    WorkersTab::Roster => WorkersTab::Busy,
                };
                self.animations.tab_switch();
            }
            KeyCode::Char('j') | KeyCode::Down => match self.workers_tab {
                WorkersTab::Busy => self.move_active(1),
                WorkersTab::Roster => self.move_worker(1),
            },
            KeyCode::Char('k') | KeyCode::Up => match self.workers_tab {
                WorkersTab::Busy => self.move_active(-1),
                WorkersTab::Roster => self.move_worker(-1),
            },
            KeyCode::Char('r') => self.reload_workers().await,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if self.workers_tab == WorkersTab::Busy {
                    if let Some(lock) = self.active_locks.get(self.active_selected) {
                        let (q, id) = (lock.queue.clone(), lock.job.id.clone());
                        self.open_job_by_id(q, id).await;
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.back(),
            _ => {}
        }
    }

    /// Open a job's detail view by explicit `(queue, id)` (used from the workers
    /// view, where the job isn't the queue-screen cursor).
    async fn open_job_by_id(&mut self, queue: String, id: String) {
        match self.client.get_job(&queue, &id).await {
            Ok(Some(job)) => {
                self.queue_name = Some(queue.clone());
                self.job = Some(job);
                self.job_tab = JobTab::Data;
                self.detail_scroll = 0;
                self.screen = Screen::Job;
                self.animations.transition();
                self.job_logs = self
                    .client
                    .job_logs(&queue, &id, 0, -1)
                    .await
                    .map(|l| l.logs)
                    .unwrap_or_default();
                self.job_flow = None;
                if let Ok(Some((rq, rid))) = self.client.find_flow_root(&queue, &id).await {
                    if let Ok(tree) = self.client.get_flow_tree(&rq, &rid, 6).await {
                        self.job_flow = tree;
                    }
                } else if let Ok(tree) = self.client.get_flow_tree(&queue, &id, 6).await {
                    self.job_flow = tree;
                }
                self.sync_flow_selection();
            }
            Ok(None) => self.set_error(format!("job {id} not found")),
            Err(e) => self.set_error(format!("open job: {e}")),
        }
    }

    /// Copy the active job-detail tab's contents to the system clipboard.
    fn copy_current_tab(&mut self) {
        let Some(job) = self.job.as_ref() else {
            self.set_error("no job loaded to copy");
            return;
        };
        let text = self.job_tab_plain_text(job);
        if text.is_empty() {
            self.flash("nothing to copy");
            return;
        }
        let bytes = text.len();
        match crate::clipboard::copy(&text, self.args.clipboard) {
            Ok(method) => self.flash(format!(
                "copied {} ({} bytes) to clipboard [{}]",
                self.job_tab.label(),
                bytes,
                method.label()
            )),
            Err(e) => self.set_error(format!("clipboard: {e}")),
        }
    }

    /// Plain-text (unstyled) rendering of the active job-detail tab, used for
    /// clipboard copy. Kept in sync with `ui::job`'s styled renderers.
    fn job_tab_plain_text(&self, job: &Job) -> String {
        use crate::format::{
            datetime, duration_between, human_duration, pretty_json, pretty_json_str, progress,
            relative,
        };
        match self.job_tab {
            JobTab::Data => {
                let mut s = format!("data:\n{}", pretty_json_str(&job.data));
                if let Some(rv) = &job.return_value {
                    s.push_str(&format!("\n\nreturnValue:\n{}", pretty_json(rv)));
                }
                s
            }
            JobTab::Options => pretty_json(&job.opts),
            JobTab::Progress => {
                let mut s = format!("progress: {}", progress(&job.progress));
                if job.progress.is_object() {
                    s.push_str(&format!("\n{}", pretty_json(&job.progress)));
                }
                s
            }
            JobTab::Error => {
                if !job.is_failed() {
                    return "No errors.".to_string();
                }
                let mut s = String::new();
                if let Some(reason) = &job.failed_reason {
                    s.push_str(&format!("failedReason:\n{reason}\n\n"));
                }
                if !job.stacktrace.is_empty() {
                    s.push_str("stacktrace:\n");
                    s.push_str(&job.stacktrace.join("\n"));
                }
                s.trim_end().to_string()
            }
            JobTab::Logs => {
                if self.job_logs.is_empty() {
                    "No logs.".to_string()
                } else {
                    self.job_logs.join("\n")
                }
            }
            JobTab::Timeline => {
                let run_at = job.timestamp.map(|t| t + job.delay);
                let mut lines = vec![
                    format!("Added at        {}", datetime(job.timestamp)),
                    format!("Added           {}", relative(job.timestamp, self.now)),
                ];
                if job.delay > 0 {
                    lines.push(format!("Delay           {}", human_duration(job.delay)));
                    lines.push(format!("Will run at     {}", datetime(run_at)));
                }
                lines.push(format!("Process started {}", datetime(job.processed_on)));
                if let Some(by) = &job.processed_by {
                    lines.push(format!("Processed by    {by}"));
                }
                lines.push(format!("Finished at     {}", datetime(job.finished_on)));
                lines.push(format!(
                    "Duration        {}",
                    duration_between(job.processed_on, job.finished_on)
                ));
                lines.join("\n")
            }
            JobTab::Flow => match &self.job_flow {
                Some(root) => {
                    let mut out = String::new();
                    flow_plain_text(root, 0, &mut out);
                    out.trim_end().to_string()
                }
                None => "No flow data.".to_string(),
            },
        }
    }

    /// Per-job actions shared between the queue and job-detail screens.
    async fn job_action_key(&mut self, key: KeyEvent) {
        let qn = self.queue_name.clone();

        // With a multi-selection on the queue screen, r / d|x / P act on the
        // whole set (bulk) rather than the single cursor job.
        if self.screen == Screen::Queue {
            let sel = self.effective_job_selection();
            if !sel.is_empty() {
                let bulk = match key.code {
                    KeyCode::Char('r') => Some(BulkAction::Retry),
                    KeyCode::Char('d') | KeyCode::Char('x') => Some(BulkAction::Remove),
                    KeyCode::Char('P') => Some(BulkAction::Promote),
                    _ => None,
                };
                if let (Some(action), Some(q)) = (bulk, qn.clone()) {
                    let mut ids: Vec<String> = sel.into_iter().collect();
                    ids.sort();
                    self.request(PendingAction::BulkJobs {
                        queue: q,
                        action,
                        ids,
                    })
                    .await;
                    return;
                }
            }
        }

        let id = match self.screen {
            Screen::Job => self.job.as_ref().map(|j| j.id.clone()),
            _ => self.selected_job().map(|j| j.id.clone()),
        };
        let (Some(q), Some(id)) = (qn, id) else {
            return;
        };
        match key.code {
            KeyCode::Char('r') => self.request(PendingAction::RetryJob(q, id)).await,
            KeyCode::Char('P') => self.request(PendingAction::PromoteJob(q, id)).await,
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.request(PendingAction::RemoveJob(q, id)).await
            }
            KeyCode::Char('D') => self.request(PendingAction::DuplicateJob(q, id)).await,
            KeyCode::Char('u') => {
                let data = self.current_job_data().unwrap_or_else(|| "{}".to_string());
                self.overlay = Overlay::Input(InputForm {
                    title: format!("Update data for job {id}"),
                    kind: InputKind::UpdateData { queue: q, id },
                    fields: vec![Field::new("data (JSON)", &data, true)],
                    focus: 0,
                });
            }
            _ => {}
        }
    }

    fn current_job_data(&self) -> Option<String> {
        let job = match self.screen {
            Screen::Job => self.job.as_ref(),
            _ => self.selected_job(),
        }?;
        Some(crate::format::pretty_json_str(&job.data))
    }

    // -- overlay key handlers ----------------------------------------------

    async fn confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Overlay::Confirm(action) =
                    std::mem::replace(&mut self.overlay, Overlay::None)
                {
                    self.execute(action).await;
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.overlay = Overlay::None;
                self.flash("cancelled");
            }
            _ => {}
        }
    }

    async fn overlay_key(&mut self, key: KeyEvent) {
        // Settings has its own editing keys.
        if matches!(self.overlay, Overlay::Settings) {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
                KeyCode::Char('j') | KeyCode::Down => {
                    self.settings.focus = (self.settings.focus + 1) % 3
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.settings.focus = (self.settings.focus + 2) % 3
                }
                KeyCode::Char('h') | KeyCode::Left => self.adjust_setting(-1),
                KeyCode::Char('l') | KeyCode::Right => self.adjust_setting(1),
                _ => {}
            }
            return;
        }
        // Help / RedisStats / Metrics: any of these closes.
        match key.code {
            KeyCode::Esc
            | KeyCode::Char('q')
            | KeyCode::Char('?')
            | KeyCode::Char('i')
            | KeyCode::Char('m') => {
                self.overlay = Overlay::None;
            }
            _ => {}
        }
    }

    fn adjust_setting(&mut self, dir: i64) {
        match self.settings.focus {
            0 => {
                // poll interval (cycle through presets)
                let presets = [0u64, 3, 5, 10, 20, 60];
                let idx = presets
                    .iter()
                    .position(|p| *p == self.settings.poll_secs)
                    .unwrap_or(2);
                let n = presets.len() as i64;
                let next = ((idx as i64 + dir).rem_euclid(n)) as usize;
                self.settings.poll_secs = presets[next];
            }
            1 => {
                let cur = self.settings.jobs_per_page as i64;
                self.settings.jobs_per_page = (cur + dir).clamp(1, 300) as usize;
            }
            2 => self.settings.confirm_actions = !self.settings.confirm_actions,
            _ => {}
        }
    }

    async fn input_key(&mut self, key: KeyEvent) {
        let Overlay::Input(mut form) = std::mem::replace(&mut self.overlay, Overlay::None) else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.flash("cancelled");
                return; // overlay already taken (None)
            }
            KeyCode::Enter if ctrl => {
                // Ctrl+Enter submits (works even in multiline fields).
                self.submit_input(form).await;
                return;
            }
            KeyCode::Enter => {
                let multiline = form.fields[form.focus].multiline;
                if multiline {
                    form.current_field_mut().value.push('\n');
                } else if form.fields.len() == 1 {
                    self.submit_input(form).await;
                    return;
                } else {
                    form.next_field();
                }
            }
            KeyCode::Tab => form.next_field(),
            KeyCode::BackTab => form.prev_field(),
            // Ctrl+U clears the focused field (readline "kill line").
            KeyCode::Char('u') if ctrl => {
                form.current_field_mut().value.clear();
            }
            // Ctrl+W deletes the previous whitespace-delimited word.
            KeyCode::Char('w') if ctrl => {
                let v = &mut form.current_field_mut().value;
                let trimmed = v.trim_end_matches(|c: char| c.is_whitespace());
                let cut = trimmed
                    .rfind(char::is_whitespace)
                    .map(|i| i + 1)
                    .unwrap_or(0);
                v.truncate(cut);
            }
            KeyCode::Backspace => {
                form.current_field_mut().value.pop();
            }
            KeyCode::Char(c) => {
                form.current_field_mut().value.push(c);
            }
            _ => {}
        }
        self.overlay = Overlay::Input(form);
    }

    // -- command palette & live filter -------------------------------------

    fn open_add_job_form(&mut self, q: String) {
        self.overlay = Overlay::Input(InputForm {
            title: format!("Add job to {q}"),
            kind: InputKind::AddJob { queue: q },
            fields: vec![
                Field::new("name", "", false),
                Field::new("data (JSON)", "{}", true),
                Field::new("opts (JSON)", "{}", true),
            ],
            focus: 0,
        });
    }

    fn open_reschedule_form(&mut self, q: String, id: String, current_delay: i64) {
        self.overlay = Overlay::Input(InputForm {
            title: format!("Reschedule job {id}"),
            kind: InputKind::Reschedule { queue: q, id },
            fields: vec![Field::new(
                "delay (ms, or 30s / 5m / 2h)",
                &current_delay.to_string(),
                false,
            )],
            focus: 0,
        });
    }

    fn open_reprioritize_form(&mut self, q: String, id: String, current: i64) {
        self.overlay = Overlay::Input(InputForm {
            title: format!("Set priority for job {id}"),
            kind: InputKind::Reprioritize { queue: q, id },
            fields: vec![Field::new(
                "priority (0 = none)",
                &current.to_string(),
                false,
            )],
            focus: 0,
        });
    }

    fn open_concurrency_form(&mut self, q: String) {
        let cur = self
            .queue_summary
            .as_ref()
            .and_then(|s| s.global_concurrency)
            .map(|n| n.to_string())
            .unwrap_or_default();
        self.overlay = Overlay::Input(InputForm {
            title: format!("Set global concurrency for {q}"),
            kind: InputKind::SetConcurrency { queue: q },
            fields: vec![Field::new("concurrency (0 = unlimited)", &cur, false)],
            focus: 0,
        });
    }

    fn open_palette(&mut self) {
        let items = self.build_palette_items();
        self.overlay = Overlay::Palette(PaletteState::new(items));
    }

    fn build_palette_items(&self) -> Vec<PaletteItem> {
        let mut items = Vec::new();
        // Navigation: jump straight to any queue.
        for q in &self.queues {
            items.push(PaletteItem::new(
                format!("→ queue: {}", q.name),
                vec!["queue", "open", "goto"],
                Command::OpenQueue(q.name.clone()),
            ));
        }
        // Navigation within a queue: jump to a status tab.
        if self.queue_name.is_some() {
            for s in JobState::ALL {
                items.push(PaletteItem::new(
                    format!("tab: {}", s.label()),
                    vec!["tab", "state", "goto"],
                    Command::GotoState(s),
                ));
            }
            items.push(PaletteItem::new(
                "queue metrics",
                vec!["metrics", "chart"],
                Command::Metrics,
            ));
        }
        items.push(PaletteItem::new(
            "overview",
            vec!["home", "queues", "back"],
            Command::GotoOverview,
        ));
        items.push(PaletteItem::new(
            "refresh",
            vec!["reload"],
            Command::Refresh,
        ));
        items.push(PaletteItem::new(
            "toggle overview view (table / bars)",
            vec!["view", "bars", "table"],
            Command::ToggleView,
        ));
        items.push(PaletteItem::new(
            "cycle sort",
            vec!["sort"],
            Command::CycleSort,
        ));
        items.push(PaletteItem::new(
            "redis stats",
            vec!["info", "server", "redis"],
            Command::RedisStats,
        ));
        items.push(PaletteItem::new(
            "settings",
            vec!["config", "prefs"],
            Command::Settings,
        ));
        items.push(PaletteItem::new("help", vec!["keys"], Command::Help));
        items.push(PaletteItem::new("quit", vec!["exit"], Command::Quit));

        // Write actions; hidden in read-only mode.
        if !self.read_only() {
            if self.queue_name.is_some() {
                items.push(PaletteItem::new(
                    "pause / resume queue",
                    vec!["pause", "resume"],
                    Command::TogglePauseQueue,
                ));
                items.push(PaletteItem::new(
                    "empty queue (drain)",
                    vec!["empty", "drain"],
                    Command::EmptyQueue,
                ));
                items.push(PaletteItem::new(
                    "obliterate queue",
                    vec!["obliterate", "delete", "nuke"],
                    Command::ObliterateQueue,
                ));
                items.push(PaletteItem::new(
                    "promote all delayed",
                    vec!["promote"],
                    Command::PromoteAllDelayed,
                ));
                items.push(PaletteItem::new(
                    "add job",
                    vec!["add", "new"],
                    Command::AddJob,
                ));
                items.push(PaletteItem::new(
                    "set global concurrency",
                    vec!["concurrency"],
                    Command::SetConcurrency,
                ));
            }
            items.push(PaletteItem::new(
                "pause ALL queues",
                vec!["pause all"],
                Command::PauseAll,
            ));
            items.push(PaletteItem::new(
                "resume ALL queues",
                vec!["resume all"],
                Command::ResumeAll,
            ));
        }
        items
    }

    async fn palette_key(&mut self, key: KeyEvent) {
        let Overlay::Palette(mut st) = std::mem::replace(&mut self.overlay, Overlay::None) else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.flash("cancelled");
                return;
            }
            KeyCode::Enter => {
                if let Some(cmd) = st.selected_command().cloned() {
                    self.run_command(cmd).await;
                }
                return;
            }
            KeyCode::Down => {
                if !st.filtered.is_empty() {
                    st.selected = (st.selected + 1).min(st.filtered.len() - 1);
                }
            }
            KeyCode::Up => st.selected = st.selected.saturating_sub(1),
            KeyCode::Char('n') if ctrl => {
                if !st.filtered.is_empty() {
                    st.selected = (st.selected + 1).min(st.filtered.len() - 1);
                }
            }
            KeyCode::Char('p') if ctrl => st.selected = st.selected.saturating_sub(1),
            KeyCode::Char('u') if ctrl => {
                st.buffer.clear();
                st.refilter();
            }
            KeyCode::Backspace => {
                st.buffer.pop();
                st.refilter();
            }
            KeyCode::Char(c) => {
                st.buffer.push(c);
                st.refilter();
            }
            _ => {}
        }
        self.overlay = Overlay::Palette(st);
    }

    async fn run_command(&mut self, cmd: Command) {
        match cmd {
            Command::OpenQueue(n) => self.open_queue(n).await,
            Command::GotoState(s) => {
                if self.queue_name.is_some() {
                    self.status_tab = StatusTab::State(s);
                    self.page = 0;
                    self.job_selected = 0;
                    self.job_filter = None;
                    self.clear_selection();
                    self.animations.tab_switch();
                    self.load_jobs().await;
                }
            }
            Command::GotoOverview => {
                self.screen = Screen::Overview;
                self.queue_name = None;
                self.animations.transition();
                self.refresh_overview().await;
            }
            Command::Refresh => self.refresh_active().await,
            Command::ToggleView => {
                self.overview_view = self.overview_view.toggled();
                self.flash(format!("overview: {} view", self.overview_view.label()));
            }
            Command::CycleSort => self.overview_sort = self.overview_sort.next(),
            Command::RedisStats => {
                self.load_redis_info().await;
                self.overlay = Overlay::RedisStats;
            }
            Command::Settings => {
                self.settings.focus = 0;
                self.overlay = Overlay::Settings;
            }
            Command::Help => self.overlay = Overlay::Help,
            Command::Metrics => {
                self.load_metrics().await;
                self.overlay = Overlay::Metrics;
            }
            Command::TogglePauseQueue => {
                if let (Some(q), Some(summary)) = (self.queue_name.clone(), &self.queue_summary) {
                    let action = if summary.is_paused {
                        PendingAction::ResumeQueue(q)
                    } else {
                        PendingAction::PauseQueue(q)
                    };
                    self.request(action).await;
                }
            }
            Command::EmptyQueue => {
                if let Some(q) = self.queue_name.clone() {
                    self.request(PendingAction::EmptyQueue(q)).await;
                }
            }
            Command::ObliterateQueue => {
                if let Some(q) = self.queue_name.clone() {
                    self.request(PendingAction::ObliterateQueue(q)).await;
                }
            }
            Command::PromoteAllDelayed => {
                if let Some(q) = self.queue_name.clone() {
                    self.request(PendingAction::PromoteAll(q)).await;
                }
            }
            Command::AddJob => {
                if let Some(q) = self.queue_name.clone() {
                    self.open_add_job_form(q);
                }
            }
            Command::SetConcurrency => {
                if let Some(q) = self.queue_name.clone() {
                    self.open_concurrency_form(q);
                }
            }
            Command::PauseAll => self.request(PendingAction::PauseAll).await,
            Command::ResumeAll => self.request(PendingAction::ResumeAll).await,
            Command::Quit => self.should_quit = true,
        }
    }

    fn open_filter(&mut self, scope: FilterScope) {
        let buffer = match scope {
            FilterScope::Overview => self.overview_search.clone().unwrap_or_default(),
            FilterScope::QueueJobs => self.job_filter.clone().unwrap_or_default(),
            FilterScope::Events => self.events_filter.clone().unwrap_or_default(),
        };
        self.overlay = Overlay::Filter(FilterState { scope, buffer });
    }

    /// Live `/` filter editor. Edits write through to the active filter on every
    /// keystroke so the list narrows as you type; Esc clears, Enter keeps.
    fn filter_key(&mut self, key: KeyEvent) {
        let Overlay::Filter(mut st) = std::mem::replace(&mut self.overlay, Overlay::None) else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                match st.scope {
                    FilterScope::Overview => {
                        self.overview_search = None;
                        self.overview_selected = 0;
                    }
                    FilterScope::QueueJobs => {
                        self.job_filter = None;
                        self.job_selected = 0;
                        self.range_anchor = None;
                    }
                    FilterScope::Events => {
                        self.events_filter = None;
                        self.events_selected = 0;
                    }
                }
                self.flash("filter cleared");
                return;
            }
            KeyCode::Enter => {
                self.flash("filter applied");
                return;
            }
            KeyCode::Char('u') if ctrl => st.buffer.clear(),
            KeyCode::Backspace => {
                st.buffer.pop();
            }
            KeyCode::Char(c) => st.buffer.push(c),
            _ => {}
        }
        let buf = st.buffer.trim().to_string();
        match st.scope {
            FilterScope::Overview => {
                self.overview_search = if buf.is_empty() { None } else { Some(buf) };
                self.overview_selected = 0;
            }
            FilterScope::QueueJobs => {
                self.job_filter = if buf.is_empty() { None } else { Some(buf) };
                self.job_selected = 0;
                self.range_anchor = None;
            }
            FilterScope::Events => {
                self.events_filter = if buf.is_empty() { None } else { Some(buf) };
                self.events_selected = 0;
            }
        }
        self.overlay = Overlay::Filter(st);
    }

    async fn on_tick(&mut self) {
        self.now = crate::format::now_ms();
        if self.settings.poll_secs > 0 && self.overlay.is_none() {
            self.refresh_active().await;
        }
    }

    /// Advance and paint animation effects over the just-rendered `frame`.
    /// Invoked by the run loop only; the pure `ui::draw` path (and therefore
    /// the test harness) never calls this, so rendered output stays
    /// deterministic.
    pub fn render_effects(&mut self, frame: &mut ratatui::Frame, elapsed: Duration) {
        self.animations.process(frame, elapsed, self.connected);
    }
}

/// The searchable text for a job row used by the live `/` filter: id, name and
/// state label, so `/failed` or `!retry` work against any of them.
fn job_haystack(job: &Job) -> String {
    let state = job.state.map(|s| s.label()).unwrap_or("");
    format!("{} {} {}", job.id, job.name, state)
}

/// The searchable text for an event row used by the live `/` filter.
fn event_haystack(e: &QueueEvent) -> String {
    format!(
        "{} {} {}",
        e.kind.label(),
        e.job_id.as_deref().unwrap_or(""),
        e.queue
    )
}

/// A stable signature of every queue's per-state counts, used to decide whether
/// a poll actually changed anything (and so deserves a shimmer).
fn overview_sig(queues: &[QueueSummary]) -> Vec<(&str, &JobCounts)> {
    let mut v: Vec<(&str, &JobCounts)> = queues
        .iter()
        .map(|q| (q.name.as_str(), &q.counts))
        .collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    v
}

/// Run the application event loop.
pub async fn run(terminal: &mut DefaultTerminal, client: BullClient, args: Args) -> Result<()> {
    let mut app = App::new(client, args);
    app.refresh_overview().await;
    app.animations.intro(); // animate the interface in on startup

    // Lazy live-events task: spawned only while the Events screen is open, so it
    // costs nothing (no extra connection, no task) otherwise. `ev_tx` is held
    // here for the whole run so `ev_rx.recv()` never resolves to `None`.
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::channel::<Vec<QueueEvent>>(1024);
    let mut ev_handle: Option<crate::events::EventStreamHandle> = None;

    let mut events = EventStream::new();
    let mut current_poll = app.settings.poll_secs;
    let mut tick = tokio::time::interval(Duration::from_secs(tick_secs(current_poll)));
    tick.tick().await; // consume immediate tick

    // Reconcile the terminal's mouse capture to `app.mouse_capture`. On by
    // default (mainstream TUI posture); `--no-mouse` / `Ctrl+O` flip it, and
    // Shift/⌥-drag selects text natively while captured. Tracked separately so we
    // only issue the escape sequence on an actual change.
    let mut mouse_on = false;
    let want_mouse = app.mouse_capture;
    set_mouse_capture(&mut app, &mut mouse_on, want_mouse);

    let mut last_frame = Instant::now();
    loop {
        // Time since the previous frame drives the animations forward.
        let now = Instant::now();
        let elapsed = now.duration_since(last_frame);
        last_frame = now;

        // Advance the clock each frame for live countdowns and elapsed times.
        // Tests drive `ui::draw` directly, so their `now` stays fixed.
        app.now = crate::format::now_ms();

        terminal.draw(|frame| {
            ui::draw(frame, &mut app);
            app.render_effects(frame, elapsed);
        })?;

        // While effects play we wake on a frame timer to keep animating; when
        // everything settles we fall back to event/poll-driven redraws only.
        let budget = app.animations.frame_budget(app.connected);
        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => app.on_key(key).await,
                    Some(Ok(Event::Mouse(me))) => app.on_mouse(me).await,
                    _ => {}
                }
            }
            _ = tick.tick() => {
                app.on_tick().await;
            }
            _ = frame_delay(budget) => {
                // Wake to advance animations; no state change needed.
            }
            maybe_batch = ev_rx.recv() => {
                if let Some(batch) = maybe_batch {
                    if app.screen == Screen::Events {
                        app.ingest_events(batch);
                        app.animations.live_update();
                    }
                }
            }
        }

        // Rebuild the ticker if the poll interval changed via settings.
        if app.settings.poll_secs != current_poll {
            current_poll = app.settings.poll_secs;
            tick = tokio::time::interval(Duration::from_secs(tick_secs(current_poll)));
            tick.tick().await;
        }

        // Reconcile mouse capture if it was toggled this frame (Ctrl+O).
        if app.mouse_capture != mouse_on {
            let want = app.mouse_capture;
            set_mouse_capture(&mut app, &mut mouse_on, want);
        }

        // Manage the lazy events-stream task lifecycle.
        if app.events_resubscribe {
            app.events_resubscribe = false;
            if let Some(h) = ev_handle.take() {
                h.shutdown().await;
            }
        }
        let want_events = app.screen == Screen::Events;
        if want_events && ev_handle.is_none() {
            let queues: Vec<String> = match &app.events_scope {
                EventScope::All => app.queues.iter().map(|q| q.name.clone()).collect(),
                EventScope::Queue(q) => vec![q.clone()],
            };
            ev_handle = Some(crate::events::EventStreamHandle::spawn(
                app.client.clone(),
                queues,
                200,
                ev_tx.clone(),
            ));
        } else if !want_events && ev_handle.is_some() {
            if let Some(h) = ev_handle.take() {
                h.shutdown().await;
            }
            app.events.clear();
        }

        if app.should_quit {
            if mouse_on {
                set_mouse_capture(&mut app, &mut mouse_on, false);
            }
            if let Some(h) = ev_handle.take() {
                h.shutdown().await;
            }
            break;
        }
    }
    Ok(())
}

/// Enable or disable terminal mouse capture, syncing `tracked` to `want`. A
/// failed escape sequence is surfaced in the status line (never swallowed) and
/// `tracked` is left untouched so we don't claim a mode the terminal rejected.
fn set_mouse_capture(app: &mut App, tracked: &mut bool, want: bool) {
    if *tracked == want {
        return;
    }
    let mut out = std::io::stdout();
    let res = if want {
        crossterm::execute!(out, EnableMouseCapture)
    } else {
        crossterm::execute!(out, DisableMouseCapture)
    };
    match res {
        Ok(()) => *tracked = want,
        Err(e) => app.set_error(format!("mouse capture: {e}")),
    }
}

/// Resolve to the next animation frame after `budget`, or never when `None`
/// (so a quiescent UI blocks purely on events and polls).
async fn frame_delay(budget: Option<Duration>) {
    match budget {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending::<()>().await,
    }
}

/// Plain-text rendering of a flow tree (mirrors `ui::job::render_flow`).
fn flow_plain_text(node: &FlowNode, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let state = node.state.map(|s| s.label()).unwrap_or("?");
    out.push_str(&format!(
        "{indent}• [{state}] {}/{} {}\n",
        node.queue_name, node.job.id, node.job.name
    ));
    for child in &node.children {
        flow_plain_text(child, depth + 1, out);
    }
}

fn tick_secs(poll: u64) -> u64 {
    if poll == 0 {
        3600
    } else {
        poll
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a minimal flow node. `Job::from_hash` yields a real `Job` from a
    /// non-empty hash, so tests don't have to spell out every field.
    fn node(id: &str, queue: &str, children: Vec<FlowNode>) -> FlowNode {
        let hash = HashMap::from([("name".to_string(), format!("job-{id}"))]);
        FlowNode {
            queue_qualified_name: format!("bull:{queue}"),
            queue_name: queue.to_string(),
            job: Job::from_hash(id, &hash).expect("non-empty hash builds a job"),
            state: None,
            children,
        }
    }

    #[test]
    fn flatten_flow_is_preorder_depth_first_with_depth() {
        //   root
        //   ├─ a
        //   │  └─ a1
        //   └─ b
        let tree = node(
            "root",
            "orchestrator",
            vec![
                node("a", "workers", vec![node("a1", "workers", vec![])]),
                node("b", "workers", vec![]),
            ],
        );
        let shape: Vec<(usize, &str)> = flatten_flow(&tree)
            .iter()
            .map(|(d, n)| (*d, n.job.id.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![(0, "root"), (1, "a"), (2, "a1"), (1, "b")],
            "preorder DFS, children in stored order, carrying depth"
        );
    }

    #[test]
    fn flatten_flow_single_node_is_one_row() {
        let tree = node("solo", "q", vec![]);
        assert_eq!(flatten_flow(&tree).len(), 1);
    }

    #[test]
    fn list_offset_windows_to_keep_selection_visible() {
        // Everything fits: no scroll regardless of selection.
        assert_eq!(list_offset(0, 5, 3), 0);
        assert_eq!(list_offset(2, 5, 3), 0);
        // Selection past the bottom edge scrolls just enough to pin it last.
        assert_eq!(list_offset(4, 5, 10), 0, "last visible row needs no scroll");
        assert_eq!(list_offset(5, 5, 10), 1, "the 6th row scrolls one line");
        assert_eq!(list_offset(9, 5, 10), 5);
        // Degenerate inputs never panic or scroll.
        assert_eq!(list_offset(3, 0, 10), 0);
        assert_eq!(list_offset(3, 5, 0), 0);
    }

    #[test]
    fn clamp_scroll_never_runs_past_the_content() {
        // Within bounds: moves by delta.
        assert_eq!(clamp_scroll(0, 1, 10), 1);
        assert_eq!(clamp_scroll(5, 3, 10), 8);
        // Clamps at the bottom - a page past the end lands exactly on max.
        assert_eq!(clamp_scroll(9, 50, 10), 10, "can't scroll into the void");
        assert_eq!(clamp_scroll(10, 1, 10), 10, "already at the bottom");
        // Clamps at the top - no negative offset.
        assert_eq!(clamp_scroll(2, -50, 10), 0);
        assert_eq!(clamp_scroll(0, -1, 10), 0);
        // A zero max (content fits) pins to the top: scrolling is a no-op.
        assert_eq!(clamp_scroll(0, 5, 0), 0);
    }

    #[test]
    fn hit_region_maps_click_row_to_offset_index() {
        let region = HitRegion {
            kind: HitKind::Job,
            area: Rect {
                x: 2,
                y: 4,
                width: 10,
                height: 3,
            },
            offset: 5,
            count: 9,
        };
        // Within the band: index = offset + (row - area.y).
        assert_eq!(region.index_at(5, 4), Some(5), "first visible row → offset");
        assert_eq!(region.index_at(5, 6), Some(7));
        // Outside the columns or rows: no hit.
        assert_eq!(region.index_at(1, 4), None, "left of the band");
        assert_eq!(region.index_at(12, 4), None, "right of the band");
        assert_eq!(region.index_at(5, 3), None, "above the band");
        assert_eq!(region.index_at(5, 7), None, "below the band");
        // A row inside the band but past the last item resolves to nothing.
        let short = HitRegion { count: 6, ..region };
        assert_eq!(short.index_at(5, 6), None, "offset 5 + row 2 = 7 ≥ count 6");
    }
}
