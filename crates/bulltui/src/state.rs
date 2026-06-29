//! Sub-state types: screens, tabs, overlays, pending actions and input forms.

use bullmq::JobState;

/// The status tabs shown in the queue view, in bull-board order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTab {
    Latest,
    State(JobState),
}

impl StatusTab {
    /// All tabs in bull-board display order: latest + the eight states.
    pub fn all() -> Vec<StatusTab> {
        let mut v = vec![StatusTab::Latest];
        v.extend(JobState::ALL.iter().map(|s| StatusTab::State(*s)));
        v
    }

    pub fn label(self) -> &'static str {
        match self {
            StatusTab::Latest => "Latest",
            StatusTab::State(s) => s.label(),
        }
    }

    pub fn job_state(self) -> Option<JobState> {
        match self {
            StatusTab::Latest => None,
            StatusTab::State(s) => Some(s),
        }
    }
}

/// Tabs in the job detail view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobTab {
    Data,
    Options,
    Progress,
    Error,
    Logs,
    Timeline,
    Flow,
}

impl JobTab {
    pub fn all() -> [JobTab; 7] {
        [
            JobTab::Data,
            JobTab::Options,
            JobTab::Progress,
            JobTab::Error,
            JobTab::Logs,
            JobTab::Timeline,
            JobTab::Flow,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            JobTab::Data => "Data",
            JobTab::Options => "Options",
            JobTab::Progress => "Progress",
            JobTab::Error => "Error",
            JobTab::Logs => "Logs",
            JobTab::Timeline => "Timeline",
            JobTab::Flow => "Flow",
        }
    }
}

/// A bulk operation applied to a set of selected jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkAction {
    Retry,
    Remove,
    Promote,
}

impl BulkAction {
    pub fn title(self) -> &'static str {
        match self {
            BulkAction::Retry => "Retry",
            BulkAction::Remove => "Remove",
            BulkAction::Promote => "Promote",
        }
    }

    pub fn past(self) -> &'static str {
        match self {
            BulkAction::Retry => "retried",
            BulkAction::Remove => "removed",
            BulkAction::Promote => "promoted",
        }
    }
}

/// Tabs in the workers / busy view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkersTab {
    Busy,
    Roster,
}

impl WorkersTab {
    pub fn all() -> [WorkersTab; 2] {
        [WorkersTab::Busy, WorkersTab::Roster]
    }

    pub fn label(self) -> &'static str {
        match self {
            WorkersTab::Busy => "Busy",
            WorkersTab::Roster => "Workers",
        }
    }
}

/// A destructive or mutating action awaiting confirmation.
#[derive(Debug, Clone)]
pub enum PendingAction {
    PauseQueue(String),
    ResumeQueue(String),
    PauseAll,
    ResumeAll,
    EmptyQueue(String),
    ObliterateQueue(String),
    CleanStatus(String, JobState),
    RetryAll(String, JobState),
    PromoteAll(String),
    RetryJob(String, String),
    PromoteJob(String, String),
    RemoveJob(String, String),
    DuplicateJob(String, String),
    /// (queue, scheduler id)
    TriggerScheduler(String, String),
    /// (queue, scheduler id)
    RemoveScheduler(String, String),
    /// (queue, job id, new delay ms)
    Reschedule(String, String, i64),
    /// (queue, job id, new priority)
    Reprioritize(String, String, i64),
    /// Apply `BulkAction` to a set of selected job ids in a queue.
    BulkJobs {
        queue: String,
        action: BulkAction,
        ids: Vec<String>,
    },
}

impl PendingAction {
    /// A human description shown in the confirmation prompt.
    pub fn describe(&self) -> String {
        match self {
            PendingAction::PauseQueue(q) => format!("Pause queue \"{q}\"?"),
            PendingAction::ResumeQueue(q) => format!("Resume queue \"{q}\"?"),
            PendingAction::PauseAll => "Pause ALL queues?".to_string(),
            PendingAction::ResumeAll => "Resume ALL queues?".to_string(),
            PendingAction::EmptyQueue(q) => {
                format!("Empty queue \"{q}\"? Removes waiting, paused & prioritized jobs.")
            }
            PendingAction::ObliterateQueue(q) => {
                format!("OBLITERATE queue \"{q}\"? Destroys the queue and ALL its data.")
            }
            PendingAction::CleanStatus(q, s) => {
                format!("Clean {} jobs from \"{q}\"? (grace 5s)", s.status_str())
            }
            PendingAction::RetryAll(q, s) => {
                format!("Retry ALL {} jobs in \"{q}\"?", s.status_str())
            }
            PendingAction::PromoteAll(q) => format!("Promote ALL delayed jobs in \"{q}\"?"),
            PendingAction::RetryJob(_, id) => format!("Retry job {id}?"),
            PendingAction::PromoteJob(_, id) => format!("Promote job {id}?"),
            PendingAction::RemoveJob(_, id) => format!("Remove job {id}? This deletes it."),
            PendingAction::DuplicateJob(_, id) => format!("Duplicate job {id}?"),
            PendingAction::TriggerScheduler(_, id) => format!("Trigger scheduler \"{id}\" now?"),
            PendingAction::RemoveScheduler(_, id) => {
                format!("Remove scheduler \"{id}\"? Deletes its schedule and next job.")
            }
            PendingAction::Reschedule(_, id, ms) => {
                format!("Reschedule job {id} to run in {ms} ms?")
            }
            PendingAction::Reprioritize(_, id, p) => format!("Set job {id} priority to {p}?"),
            PendingAction::BulkJobs { action, ids, .. } => {
                format!("{} {} selected job(s)?", action.title(), ids.len())
            }
        }
    }

    /// Whether this action is destructive (affects confirmation styling).
    pub fn is_destructive(&self) -> bool {
        matches!(
            self,
            PendingAction::EmptyQueue(_)
                | PendingAction::ObliterateQueue(_)
                | PendingAction::CleanStatus(_, _)
                | PendingAction::RemoveJob(_, _)
                | PendingAction::RemoveScheduler(_, _)
                | PendingAction::BulkJobs {
                    action: BulkAction::Remove,
                    ..
                }
        )
    }
}

/// A single editable field in an input form.
#[derive(Debug, Clone)]
pub struct Field {
    pub label: String,
    pub value: String,
    pub multiline: bool,
}

impl Field {
    pub fn new(label: &str, value: &str, multiline: bool) -> Self {
        Self {
            label: label.to_string(),
            value: value.to_string(),
            multiline,
        }
    }
}

/// What an input form is collecting, and the context to apply it to.
#[derive(Debug, Clone)]
pub enum InputKind {
    AddJob { queue: String },
    SetConcurrency { queue: String },
    UpdateData { queue: String, id: String },
    Reschedule { queue: String, id: String },
    Reprioritize { queue: String, id: String },
}

/// An active input form.
#[derive(Debug, Clone)]
pub struct InputForm {
    pub title: String,
    pub kind: InputKind,
    pub fields: Vec<Field>,
    pub focus: usize,
}

impl InputForm {
    pub fn current_field_mut(&mut self) -> &mut Field {
        &mut self.fields[self.focus]
    }

    pub fn next_field(&mut self) {
        if !self.fields.is_empty() {
            self.focus = (self.focus + 1) % self.fields.len();
        }
    }

    pub fn prev_field(&mut self) {
        if !self.fields.is_empty() {
            self.focus = (self.focus + self.fields.len() - 1) % self.fields.len();
        }
    }
}

/// A command invocable from the `:` palette. Maps onto existing `App` behavior.
#[derive(Debug, Clone)]
pub enum Command {
    OpenQueue(String),
    GotoState(JobState),
    GotoOverview,
    Refresh,
    ToggleView,
    CycleSort,
    RedisStats,
    Settings,
    Help,
    Metrics,
    TogglePauseQueue,
    EmptyQueue,
    ObliterateQueue,
    PromoteAllDelayed,
    AddJob,
    SetConcurrency,
    PauseAll,
    ResumeAll,
    Quit,
}

/// A single entry in the command palette.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub label: String,
    pub aliases: Vec<&'static str>,
    pub command: Command,
}

impl PaletteItem {
    pub fn new(label: impl Into<String>, aliases: Vec<&'static str>, command: Command) -> Self {
        Self {
            label: label.into(),
            aliases,
            command,
        }
    }

    /// Best fuzzy score of `q` across the label and aliases.
    pub fn best_score(&self, q: &str) -> Option<i32> {
        std::iter::once(self.label.as_str())
            .chain(self.aliases.iter().copied())
            .filter_map(|h| crate::fuzzy::score(q, h))
            .max()
    }
}

/// State of the `:` command palette.
#[derive(Debug, Clone)]
pub struct PaletteState {
    pub buffer: String,
    pub items: Vec<PaletteItem>,
    /// Indices into `items`, score-sorted for the current `buffer`.
    pub filtered: Vec<usize>,
    pub selected: usize,
}

impl PaletteState {
    pub fn new(items: Vec<PaletteItem>) -> Self {
        let mut s = Self {
            buffer: String::new(),
            items,
            filtered: Vec::new(),
            selected: 0,
        };
        s.refilter();
        s
    }

    pub fn refilter(&mut self) {
        let q = self.buffer.trim();
        if q.is_empty() {
            self.filtered = (0..self.items.len()).collect();
        } else {
            let mut scored: Vec<(i32, usize)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| it.best_score(q).map(|s| (s, i)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        }
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    pub fn selected_command(&self) -> Option<&Command> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.items.get(i))
            .map(|it| &it.command)
    }
}

/// Which list a live `/` filter targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterScope {
    Overview,
    QueueJobs,
    Events,
}

/// Scope of the live events feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventScope {
    All,
    Queue(String),
}

impl EventScope {
    pub fn label(&self) -> String {
        match self {
            EventScope::All => "all queues".to_string(),
            EventScope::Queue(q) => q.clone(),
        }
    }
}

/// State of the live `/` filter editor.
#[derive(Debug, Clone)]
pub struct FilterState {
    pub scope: FilterScope,
    pub buffer: String,
}

/// Which overlay (modal) is currently active.
#[derive(Debug, Clone)]
pub enum Overlay {
    None,
    Help,
    Confirm(PendingAction),
    Input(InputForm),
    RedisStats,
    Metrics,
    Settings,
    Palette(PaletteState),
    Filter(FilterState),
}

impl Overlay {
    pub fn is_none(&self) -> bool {
        matches!(self, Overlay::None)
    }
    pub fn is_input(&self) -> bool {
        matches!(self, Overlay::Input(_))
    }
}

/// User-adjustable settings (mirrors bull-board's persisted settings).
#[derive(Debug, Clone)]
pub struct Settings {
    pub poll_secs: u64,
    pub jobs_per_page: usize,
    pub confirm_actions: bool,
    /// Index of the focused setting in the settings overlay.
    pub focus: usize,
}
