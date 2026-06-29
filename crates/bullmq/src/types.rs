//! Domain types mirroring BullMQ's data model and bull-board's `AppJob`/`AppQueue`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A BullMQ job state. The eight concrete states map to Redis structures;
/// [`JobState::all`] yields them in bull-board's display order.
///
/// Note: `latest` in bull-board is a *virtual* state (newest across all states)
/// and is represented separately in the UI, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum JobState {
    Active,
    Waiting,
    WaitingChildren,
    Prioritized,
    Completed,
    Failed,
    Delayed,
    Paused,
}

impl JobState {
    /// All concrete states in bull-board display order.
    pub const ALL: [JobState; 8] = [
        JobState::Active,
        JobState::Waiting,
        JobState::WaitingChildren,
        JobState::Prioritized,
        JobState::Completed,
        JobState::Failed,
        JobState::Delayed,
        JobState::Paused,
    ];

    /// The Redis key suffix that backs this state.
    pub fn redis_suffix(self) -> &'static str {
        match self {
            JobState::Active => "active",
            JobState::Waiting => "wait",
            JobState::WaitingChildren => "waiting-children",
            JobState::Prioritized => "prioritized",
            JobState::Completed => "completed",
            JobState::Failed => "failed",
            JobState::Delayed => "delayed",
            JobState::Paused => "paused",
        }
    }

    /// The bull-board status string (e.g. `waiting`, `waiting-children`).
    pub fn status_str(self) -> &'static str {
        match self {
            JobState::Active => "active",
            JobState::Waiting => "waiting",
            JobState::WaitingChildren => "waiting-children",
            JobState::Prioritized => "prioritized",
            JobState::Completed => "completed",
            JobState::Failed => "failed",
            JobState::Delayed => "delayed",
            JobState::Paused => "paused",
        }
    }

    /// A short human label for tabs/columns.
    pub fn label(self) -> &'static str {
        match self {
            JobState::Active => "Active",
            JobState::Waiting => "Waiting",
            JobState::WaitingChildren => "Waiting Children",
            JobState::Prioritized => "Prioritized",
            JobState::Completed => "Completed",
            JobState::Failed => "Failed",
            JobState::Delayed => "Delayed",
            JobState::Paused => "Paused",
        }
    }

    /// Whether the state is backed by a Redis list (vs a sorted set).
    pub fn is_list(self) -> bool {
        matches!(
            self,
            JobState::Active | JobState::Waiting | JobState::Paused
        )
    }

    /// Parse a bull-board status string (accepts both `waiting` and `wait`).
    pub fn from_status_str(s: &str) -> Option<JobState> {
        Some(match s {
            "active" => JobState::Active,
            "waiting" | "wait" => JobState::Waiting,
            "waiting-children" => JobState::WaitingChildren,
            "prioritized" => JobState::Prioritized,
            "completed" => JobState::Completed,
            "failed" => JobState::Failed,
            "delayed" => JobState::Delayed,
            "paused" => JobState::Paused,
            _ => return None,
        })
    }

    /// Statuses eligible for the `clean` operation (bull-board `JobCleanStatus`).
    pub fn cleanable() -> [JobState; 5] {
        [
            JobState::Completed,
            JobState::Waiting,
            JobState::Active,
            JobState::Delayed,
            JobState::Failed,
        ]
    }
}

/// Job counts per state — bull-board's `JobCounts` / `AppQueue.counts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobCounts {
    pub active: i64,
    pub waiting: i64,
    pub waiting_children: i64,
    pub prioritized: i64,
    pub completed: i64,
    pub failed: i64,
    pub delayed: i64,
    pub paused: i64,
}

impl JobCounts {
    /// Count for a given state.
    pub fn get(&self, state: JobState) -> i64 {
        match state {
            JobState::Active => self.active,
            JobState::Waiting => self.waiting,
            JobState::WaitingChildren => self.waiting_children,
            JobState::Prioritized => self.prioritized,
            JobState::Completed => self.completed,
            JobState::Failed => self.failed,
            JobState::Delayed => self.delayed,
            JobState::Paused => self.paused,
        }
    }

    pub fn set(&mut self, state: JobState, value: i64) {
        match state {
            JobState::Active => self.active = value,
            JobState::Waiting => self.waiting = value,
            JobState::WaitingChildren => self.waiting_children = value,
            JobState::Prioritized => self.prioritized = value,
            JobState::Completed => self.completed = value,
            JobState::Failed => self.failed = value,
            JobState::Delayed => self.delayed = value,
            JobState::Paused => self.paused = value,
        }
    }

    /// Total across all states.
    pub fn total(&self) -> i64 {
        JobState::ALL.iter().map(|s| self.get(*s)).sum()
    }
}

/// A fully-decoded BullMQ job, mirroring the fields bull-board's `formatJob`
/// exposes (plus a few extras useful in a TUI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    /// Raw job data, as stored (JSON string).
    pub data: String,
    /// Decoded job options (abbreviations expanded to match bull-board).
    pub opts: Value,
    /// Raw options string, as stored.
    pub opts_raw: String,
    /// Decoded progress (number, string, object or bool).
    pub progress: Value,
    pub attempts_made: i64,
    pub attempts_started: i64,
    pub stalled_counter: i64,
    pub delay: i64,
    pub priority: i64,
    pub timestamp: Option<i64>,
    pub processed_on: Option<i64>,
    pub finished_on: Option<i64>,
    pub processed_by: Option<String>,
    pub failed_reason: Option<String>,
    /// Stack trace lines, filtered and reversed to match bull-board.
    pub stacktrace: Vec<String>,
    pub return_value: Option<Value>,
    pub parent_key: Option<String>,
    pub parent: Option<Value>,
    pub repeat_job_key: Option<String>,
    pub deduplication_id: Option<String>,
    pub deferred_failure: Option<String>,
    pub next_repeatable_job_id: Option<String>,
    /// The state the job was found in (set by the listing call, if known).
    pub state: Option<JobState>,
}

impl Job {
    /// Whether the job is considered failed (bull-board `isFailed`).
    pub fn is_failed(&self) -> bool {
        self.failed_reason.is_some() || !self.stacktrace.is_empty()
    }

    /// Group id, if any (`opts.group.id`).
    pub fn group_id(&self) -> Option<String> {
        self.opts
            .get("group")
            .and_then(|g| g.get("id"))
            .map(value_to_plain_string)
    }

    /// Decode a job from its Redis hash. `id` is the job id.
    ///
    /// Returns `None` if the hash is empty (job does not exist).
    pub fn from_hash(id: &str, hash: &HashMap<String, String>) -> Option<Job> {
        if hash.is_empty() {
            return None;
        }
        let get = |k: &str| hash.get(k).map(String::as_str);

        let data = get("data").unwrap_or("{}").to_string();
        let opts_raw = get("opts").unwrap_or("{}").to_string();
        let opts = decode_opts(&opts_raw);
        let progress =
            serde_json::from_str(get("progress").unwrap_or("0")).unwrap_or(Value::from(0));

        let attempts_made = parse_int(get("attemptsMade").or_else(|| get("atm")));
        let attempts_started = parse_int(get("ats"));
        let stalled_counter = parse_int(get("stc"));
        let delay = parse_int(get("delay"));
        let priority = parse_int(get("priority"));

        let stacktrace = decode_stacktrace(get("stacktrace"));
        let return_value = get("returnvalue").map(decode_return_value);

        let parent = get("parent").and_then(|p| serde_json::from_str(p).ok());

        Some(Job {
            id: get("id").unwrap_or(id).to_string(),
            name: get("name").unwrap_or_default().to_string(),
            data,
            opts,
            opts_raw,
            progress,
            attempts_made,
            attempts_started,
            stalled_counter,
            delay,
            priority,
            timestamp: parse_opt_int(get("timestamp")),
            processed_on: parse_opt_int(get("processedOn")),
            finished_on: parse_opt_int(get("finishedOn")),
            processed_by: get("pb").map(str::to_string),
            failed_reason: get("failedReason").map(str::to_string),
            stacktrace,
            return_value,
            parent_key: get("parentKey").map(str::to_string),
            parent,
            repeat_job_key: get("rjk").map(str::to_string),
            deduplication_id: get("deid").map(str::to_string),
            deferred_failure: get("defa").map(str::to_string),
            next_repeatable_job_id: get("nrjid").map(str::to_string),
            state: None,
        })
    }
}

impl Job {
    /// Classify a job sitting in the `delayed` ZSET.
    pub fn delayed_kind(&self) -> DelayedKind {
        if self
            .repeat_job_key
            .as_deref()
            .is_some_and(|s| !s.is_empty())
            || self.id.starts_with("repeat:")
        {
            DelayedKind::Scheduled
        } else if self.attempts_made > 0 && self.failed_reason.is_some() {
            DelayedKind::RetryBackoff
        } else {
            DelayedKind::Plain
        }
    }

    /// Absolute next-run epoch ms for a delayed job (`timestamp + delay`).
    pub fn delayed_run_at(&self) -> Option<i64> {
        self.timestamp.map(|t| t + self.delay)
    }
}

/// Why a job is sitting in the `delayed` set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelayedKind {
    /// Produced by a job scheduler (cron / repeatable).
    Scheduled,
    /// A failed job waiting out its retry backoff.
    RetryBackoff,
    /// An ordinary one-off delayed job.
    Plain,
}

impl DelayedKind {
    pub fn label(self) -> &'static str {
        match self {
            DelayedKind::Scheduled => "scheduled",
            DelayedKind::RetryBackoff => "retry",
            DelayedKind::Plain => "delayed",
        }
    }
}

/// A BullMQ job scheduler (cron / repeatable), mirroring `getJobSchedulers`.
/// New-style schedulers carry an iteration count (`ic`); legacy repeatables
/// don't ([`JobScheduler::is_new_style`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobScheduler {
    pub id: String,
    pub name: Option<String>,
    /// Next-run epoch ms — the `repeat` ZSET score (authoritative).
    pub next_run_ms: Option<i64>,
    pub pattern: Option<String>,
    pub every: Option<u64>,
    pub tz: Option<String>,
    pub limit: Option<i64>,
    pub iteration_count: Option<i64>,
    pub template_data: Option<Value>,
    pub template_opts: Option<Value>,
}

impl JobScheduler {
    /// `ic` present ⇒ a new-style scheduler (vs a legacy repeatable).
    pub fn is_new_style(&self) -> bool {
        self.iteration_count.is_some()
    }

    /// A short human "schedule" description: the cron pattern or `every Nms`.
    pub fn schedule_label(&self) -> String {
        if let Some(p) = &self.pattern {
            p.clone()
        } else if let Some(e) = self.every {
            format!("every {e}ms")
        } else {
            "—".to_string()
        }
    }

    /// Decode from a scheduler id, its `repeat` ZSET score, and its hash.
    pub fn from_hash(
        id: &str,
        score: Option<i64>,
        hash: &HashMap<String, String>,
    ) -> Option<JobScheduler> {
        if hash.is_empty() {
            return None;
        }
        let get = |k: &str| hash.get(k).map(String::as_str);
        Some(JobScheduler {
            id: id.to_string(),
            name: get("name").map(str::to_string),
            next_run_ms: score,
            pattern: get("pattern").map(str::to_string),
            every: get("every").and_then(|s| s.parse::<u64>().ok()),
            tz: get("tz").map(str::to_string),
            limit: parse_opt_int(get("limit")),
            iteration_count: parse_opt_int(get("ic")),
            template_data: get("data").and_then(|s| serde_json::from_str(s).ok()),
            template_opts: get("opts").map(decode_opts),
        })
    }
}

/// Job logs (bull-board returns the `logs` array; `count` is the total stored).
#[derive(Debug, Clone, Default)]
pub struct JobLogs {
    pub logs: Vec<String>,
    pub count: i64,
}

/// Historical metrics for a queue (completed or failed), mirroring
/// `Queue.getMetrics`.
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub count: i64,
    pub prev_ts: i64,
    pub prev_count: i64,
    /// Per-minute data points (most recent last).
    pub data: Vec<i64>,
}

/// Which metric series to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsKind {
    Completed,
    Failed,
}

impl MetricsKind {
    pub fn key(self) -> &'static str {
        match self {
            MetricsKind::Completed => "completed",
            MetricsKind::Failed => "failed",
        }
    }
}

/// A node in a job flow tree (parent → children).
#[derive(Debug, Clone)]
pub struct FlowNode {
    /// Fully-qualified queue name the job lives in (`{prefix}:{queue}`).
    pub queue_qualified_name: String,
    pub queue_name: String,
    pub job: Job,
    pub state: Option<JobState>,
    pub children: Vec<FlowNode>,
}

/// A lightweight queue summary for the overview screen.
#[derive(Debug, Clone)]
pub struct QueueSummary {
    pub name: String,
    pub prefix: String,
    pub counts: JobCounts,
    pub is_paused: bool,
    pub global_concurrency: Option<i64>,
}

impl QueueSummary {
    pub fn total_jobs(&self) -> i64 {
        self.counts.total()
    }
}

/// Parsed Redis `INFO` output with convenience getters used by the stats view.
#[derive(Debug, Clone, Default)]
pub struct RedisInfo {
    pub fields: HashMap<String, String>,
}

impl RedisInfo {
    pub fn parse(raw: &str) -> RedisInfo {
        let mut fields = HashMap::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once(':') {
                fields.insert(k.to_string(), v.to_string());
            }
        }
        RedisInfo { fields }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    fn get_int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| v.parse().ok())
    }

    fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|v| v.parse().ok())
    }

    pub fn version(&self) -> Option<&str> {
        self.get("redis_version")
    }
    pub fn mode(&self) -> Option<&str> {
        self.get("redis_mode")
    }
    pub fn os(&self) -> Option<&str> {
        self.get("os")
    }
    pub fn tcp_port(&self) -> Option<i64> {
        self.get_int("tcp_port")
    }
    pub fn uptime_seconds(&self) -> Option<i64> {
        self.get_int("uptime_in_seconds")
    }
    pub fn used_memory(&self) -> Option<i64> {
        self.get_int("used_memory")
    }
    pub fn used_memory_peak(&self) -> Option<i64> {
        self.get_int("used_memory_peak")
    }
    pub fn mem_fragmentation_ratio(&self) -> Option<f64> {
        self.get_f64("mem_fragmentation_ratio")
    }
    pub fn connected_clients(&self) -> Option<i64> {
        self.get_int("connected_clients")
    }
    pub fn blocked_clients(&self) -> Option<i64> {
        self.get_int("blocked_clients")
    }
    pub fn maxmemory(&self) -> Option<i64> {
        self.get_int("maxmemory")
    }
    pub fn total_system_memory(&self) -> Option<i64> {
        self.get_int("total_system_memory")
    }

    /// The denominator for the memory-usage gauge: `maxmemory` if configured,
    /// otherwise total system memory.
    pub fn total_memory(&self) -> Option<i64> {
        match self.maxmemory() {
            Some(m) if m > 0 => Some(m),
            _ => self.total_system_memory(),
        }
    }

    /// Memory usage fraction in `0.0..=1.0`, if computable.
    pub fn memory_usage_fraction(&self) -> Option<f64> {
        match (self.used_memory(), self.total_memory()) {
            (Some(used), Some(total)) if total > 0 => Some(used as f64 / total as f64),
            _ => None,
        }
    }

    pub fn keyspace_hits(&self) -> Option<i64> {
        self.get_int("keyspace_hits")
    }
    pub fn keyspace_misses(&self) -> Option<i64> {
        self.get_int("keyspace_misses")
    }
    /// Cache hit ratio in `0.0..=1.0` (`hits / (hits + misses)`).
    pub fn hit_ratio(&self) -> Option<f64> {
        match (self.keyspace_hits(), self.keyspace_misses()) {
            (Some(h), Some(m)) if h + m > 0 => Some(h as f64 / (h + m) as f64),
            _ => None,
        }
    }
    pub fn instantaneous_ops_per_sec(&self) -> Option<i64> {
        self.get_int("instantaneous_ops_per_sec")
    }
    pub fn evicted_keys(&self) -> Option<i64> {
        self.get_int("evicted_keys")
    }
}

/// A connected worker, parsed from Redis `CLIENT LIST`. BullMQ names worker
/// connections `{prefix}:{base64(queueName)}` (unnamed) or `…:w:{name}` (named);
/// QueueEvents connections carry a `:qe` suffix and are excluded.
#[derive(Debug, Clone)]
pub struct WorkerInfo {
    pub addr: String,
    pub raw_name: String,
    pub queue: Option<String>,
    pub worker_name: Option<String>,
    pub age_secs: i64,
    pub idle_secs: i64,
    pub last_cmd: String,
}

impl WorkerInfo {
    /// Parse `CLIENT LIST`, keeping only BullMQ worker connections for `prefix`.
    /// Parses each line into a `key=value` map so it tolerates field-order and
    /// extra fields across redis/valkey versions.
    pub fn parse_client_list(raw: &str, prefix: &str) -> Vec<WorkerInfo> {
        let want = format!("{prefix}:");
        let mut out = Vec::new();
        for line in raw.lines() {
            let mut map: HashMap<&str, &str> = HashMap::new();
            for tok in line.split_whitespace() {
                if let Some((k, v)) = tok.split_once('=') {
                    map.insert(k, v);
                }
            }
            let Some(name) = map.get("name").copied() else {
                continue;
            };
            if !name.starts_with(&want) {
                continue;
            }
            let rest = &name[want.len()..];
            if rest.ends_with(":qe") {
                continue; // QueueEvents connection, not a worker
            }
            let (b64, worker_name) = match rest.split_once(":w:") {
                Some((b, w)) => (b, Some(w.to_string())),
                None => (rest, None),
            };
            let queue = base64_decode(b64).and_then(|b| String::from_utf8(b).ok());
            out.push(WorkerInfo {
                addr: map.get("addr").copied().unwrap_or("?").to_string(),
                raw_name: name.to_string(),
                queue,
                worker_name,
                age_secs: map.get("age").and_then(|s| s.parse().ok()).unwrap_or(0),
                idle_secs: map.get("idle").and_then(|s| s.parse().ok()).unwrap_or(0),
                last_cmd: map.get("cmd").copied().unwrap_or("?").to_string(),
            });
        }
        out
    }
}

/// An active job together with the TTL on its worker lock.
#[derive(Debug, Clone)]
pub struct ActiveJobLock {
    pub queue: String,
    pub job: Job,
    /// `PTTL` of the job lock: `-2` missing/expired, `-1` no expiry, else ms.
    pub lock_ttl_ms: i64,
    pub in_stalled_set: bool,
}

impl ActiveJobLock {
    /// How long the job has been active (`now - processedOn`), if known.
    pub fn active_for_ms(&self, now: i64) -> Option<i64> {
        self.job.processed_on.map(|p| now - p)
    }
    /// The lock is gone, or expires within `warn_ms`.
    pub fn is_at_risk(&self, warn_ms: i64) -> bool {
        self.lock_ttl_ms == -2 || (self.lock_ttl_ms >= 0 && self.lock_ttl_ms <= warn_ms)
    }
    /// In the stalled set with a dead lock ⇒ declared stalled next check.
    pub fn will_be_declared_stalled(&self) -> bool {
        self.in_stalled_set && self.lock_ttl_ms == -2
    }
}

/// Live rate-limit + concurrency state for a queue.
#[derive(Debug, Clone, Default)]
pub struct RateLimitStatus {
    pub counter: Option<i64>,
    pub ttl_ms: i64,
    pub max: Option<i64>,
    pub duration_ms: Option<i64>,
    pub concurrency: Option<i64>,
    pub active: i64,
    /// `queue.rateLimit(ms)` sets the counter to MAX_SAFE_INTEGER — a manual block.
    pub manual: bool,
}

impl RateLimitStatus {
    pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

    pub fn is_throttled(&self) -> bool {
        if self.manual {
            return true;
        }
        match (self.counter, self.max) {
            (Some(c), Some(m)) => c >= m && self.ttl_ms > 0,
            _ => false,
        }
    }
}

/// One parsed entry from a `{prefix}:{queue}:events` Redis stream.
#[derive(Debug, Clone)]
pub struct QueueEvent {
    /// The XADD entry id, `{ms}-{seq}`.
    pub stream_id: String,
    /// Milliseconds parsed from the stream id prefix.
    pub ts: i64,
    pub queue: String,
    pub kind: EventKind,
    pub job_id: Option<String>,
    /// All raw fields (data, returnvalue, prev, failedReason, …).
    pub fields: HashMap<String, String>,
}

impl QueueEvent {
    /// A one-line, kind-specific summary for the feed.
    pub fn summary(&self) -> String {
        let f = |k: &str| self.fields.get(k).map(String::as_str).unwrap_or("");
        match self.kind {
            EventKind::Completed => format!("→ {}", f("returnvalue")),
            EventKind::Failed => f("failedReason").to_string(),
            EventKind::RetriesExhausted => format!("attemptsMade {}", f("attemptsMade")),
            EventKind::Progress => f("data").to_string(),
            EventKind::Delayed => format!("delay {}", f("delay")),
            EventKind::Deduplicated => format!("deduplicationId {}", f("deduplicationId")),
            EventKind::Cleaned => format!("count {}", f("count")),
            _ => match (self.fields.get("prev"), self.fields.get("name")) {
                (Some(prev), _) => format!("prev {prev}"),
                (None, Some(name)) => name.clone(),
                _ => String::new(),
            },
        }
    }
}

/// The BullMQ event types emitted on a queue's events stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Added,
    Waiting,
    Active,
    Completed,
    Failed,
    RetriesExhausted,
    Progress,
    Delayed,
    Stalled,
    Removed,
    Cleaned,
    Paused,
    Resumed,
    Drained,
    WaitingChildren,
    Deduplicated,
    Duplicated,
    /// Forward-compatibility for unknown `event` values.
    Other,
}

impl EventKind {
    pub fn from_event_str(s: &str) -> EventKind {
        match s {
            "added" => EventKind::Added,
            "waiting" => EventKind::Waiting,
            "active" => EventKind::Active,
            "completed" => EventKind::Completed,
            "failed" => EventKind::Failed,
            "retries-exhausted" => EventKind::RetriesExhausted,
            "progress" => EventKind::Progress,
            "delayed" => EventKind::Delayed,
            "stalled" => EventKind::Stalled,
            "removed" => EventKind::Removed,
            "cleaned" => EventKind::Cleaned,
            "paused" => EventKind::Paused,
            "resumed" => EventKind::Resumed,
            "drained" => EventKind::Drained,
            "waiting-children" => EventKind::WaitingChildren,
            "deduplicated" => EventKind::Deduplicated,
            "duplicated" => EventKind::Duplicated,
            _ => EventKind::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EventKind::Added => "added",
            EventKind::Waiting => "waiting",
            EventKind::Active => "active",
            EventKind::Completed => "completed",
            EventKind::Failed => "failed",
            EventKind::RetriesExhausted => "retries-exhausted",
            EventKind::Progress => "progress",
            EventKind::Delayed => "delayed",
            EventKind::Stalled => "stalled",
            EventKind::Removed => "removed",
            EventKind::Cleaned => "cleaned",
            EventKind::Paused => "paused",
            EventKind::Resumed => "resumed",
            EventKind::Drained => "drained",
            EventKind::WaitingChildren => "waiting-children",
            EventKind::Deduplicated => "deduplicated",
            EventKind::Duplicated => "duplicated",
            EventKind::Other => "event",
        }
    }

    pub fn is_failure(self) -> bool {
        matches!(
            self,
            EventKind::Failed | EventKind::RetriesExhausted | EventKind::Stalled
        )
    }
}

// ---------------------------------------------------------------------------
// decoding helpers
// ---------------------------------------------------------------------------

/// Minimal standard base64 decoder (RFC 4648), used for CLIENT LIST queue names.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut buf = [0u8; 4];
        let mut n = 0;
        for (i, &b) in chunk.iter().enumerate() {
            buf[i] = val(b)?;
            n += 1;
        }
        if n >= 2 {
            out.push((buf[0] << 2) | (buf[1] >> 4));
        }
        if n >= 3 {
            out.push((buf[1] << 4) | (buf[2] >> 2));
        }
        if n >= 4 {
            out.push((buf[2] << 6) | buf[3]);
        }
    }
    Some(out)
}

fn parse_int(v: Option<&str>) -> i64 {
    v.and_then(|s| s.parse::<i64>().ok())
        .or_else(|| v.and_then(|s| s.parse::<f64>().ok().map(|f| f as i64)))
        .unwrap_or(0)
}

fn parse_opt_int(v: Option<&str>) -> Option<i64> {
    let v = v?;
    if v.is_empty() {
        return None;
    }
    v.parse::<i64>()
        .ok()
        .or_else(|| v.parse::<f64>().ok().map(|f| f as i64))
}

/// Decode `stacktrace` JSON, filter empties, and reverse (bull-board order).
fn decode_stacktrace(raw: Option<&str>) -> Vec<String> {
    let raw = match raw {
        Some(r) if !r.is_empty() => r,
        _ => return Vec::new(),
    };
    let parsed: Vec<Value> = serde_json::from_str(raw).unwrap_or_default();
    let mut frames: Vec<String> = parsed
        .into_iter()
        .filter_map(|v| match v {
            Value::Null => None,
            Value::String(s) if s.is_empty() => None,
            Value::String(s) => Some(s),
            other => Some(other.to_string()),
        })
        .collect();
    frames.reverse();
    frames
}

/// `getReturnValue`: parse as JSON, falling back to the raw string.
fn decode_return_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// Expand BullMQ's abbreviated option keys to their full names (matches
/// `Job.optsFromJSON` + `optsDecodeMap`).
fn decode_opts(raw: &str) -> Value {
    let parsed: Value = serde_json::from_str(raw).unwrap_or(Value::Object(Default::default()));
    let obj = match parsed {
        Value::Object(map) => map,
        other => return other,
    };

    let mut out = serde_json::Map::new();
    let mut telemetry = serde_json::Map::new();
    for (k, v) in obj {
        match k.as_str() {
            "de" => {
                out.insert("deduplication".into(), v);
            }
            "fpof" => {
                out.insert("failParentOnFailure".into(), v);
            }
            "cpof" => {
                out.insert("continueParentOnFailure".into(), v);
            }
            "idof" => {
                out.insert("ignoreDependencyOnFailure".into(), v);
            }
            "kl" => {
                out.insert("keepLogs".into(), v);
            }
            "rdof" => {
                out.insert("removeDependencyOnFailure".into(), v);
            }
            "tm" => {
                telemetry.insert("metadata".into(), v);
            }
            "omc" => {
                telemetry.insert("omitContext".into(), v);
            }
            _ => {
                out.insert(k, v);
            }
        }
    }
    if !telemetry.is_empty() {
        out.insert("telemetry".into(), Value::Object(telemetry));
    }
    Value::Object(out)
}

/// Render a JSON value as a plain string (unquoting strings).
pub fn value_to_plain_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn empty_hash_is_none() {
        assert!(Job::from_hash("1", &HashMap::new()).is_none());
    }

    #[test]
    fn decodes_basic_job() {
        let h = hash(&[
            ("name", "send-email"),
            ("data", r#"{"to":"a@b.com"}"#),
            ("opts", r#"{"attempts":3,"fpof":true}"#),
            ("timestamp", "1700000000000"),
            ("delay", "0"),
            ("priority", "0"),
            ("attemptsMade", "2"),
            ("progress", "50"),
            ("processedOn", "1700000001000"),
            ("finishedOn", "1700000002000"),
            ("pb", "worker-1"),
        ]);
        let job = Job::from_hash("7", &h).unwrap();
        assert_eq!(job.id, "7");
        assert_eq!(job.name, "send-email");
        assert_eq!(job.attempts_made, 2);
        assert_eq!(job.progress, Value::from(50));
        assert_eq!(job.processed_by.as_deref(), Some("worker-1"));
        // abbreviated opt expanded
        assert_eq!(
            job.opts.get("failParentOnFailure"),
            Some(&Value::Bool(true))
        );
        assert_eq!(job.opts.get("attempts"), Some(&Value::from(3)));
        assert!(!job.is_failed());
    }

    #[test]
    fn decodes_failed_job_with_reversed_stacktrace() {
        let h = hash(&[
            ("name", "x"),
            ("failedReason", "boom"),
            ("stacktrace", r#"["frame1","frame2","",null]"#),
        ]);
        let job = Job::from_hash("1", &h).unwrap();
        assert!(job.is_failed());
        // filtered (no empty/null) + reversed
        assert_eq!(
            job.stacktrace,
            vec!["frame2".to_string(), "frame1".to_string()]
        );
    }

    #[test]
    fn return_value_falls_back_to_raw() {
        let h = hash(&[("name", "x"), ("returnvalue", "not json")]);
        let job = Job::from_hash("1", &h).unwrap();
        assert_eq!(job.return_value, Some(Value::String("not json".into())));

        let h2 = hash(&[("name", "x"), ("returnvalue", r#"{"ok":true}"#)]);
        let job2 = Job::from_hash("1", &h2).unwrap();
        assert_eq!(
            job2.return_value.unwrap().get("ok"),
            Some(&Value::Bool(true))
        );
    }

    #[test]
    fn counts_total_and_get() {
        let mut c = JobCounts::default();
        c.set(JobState::Waiting, 5);
        c.set(JobState::Failed, 2);
        assert_eq!(c.get(JobState::Waiting), 5);
        assert_eq!(c.total(), 7);
    }

    #[test]
    fn redis_info_parses_and_computes_usage() {
        let raw = "# Server\r\nredis_version:7.4.0\r\nuptime_in_seconds:100\r\n# Memory\r\nused_memory:50\r\nmaxmemory:100\r\nconnected_clients:3\r\nkeyspace_hits:30\r\nkeyspace_misses:10\r\ninstantaneous_ops_per_sec:42\r\nevicted_keys:0\r\n";
        let info = RedisInfo::parse(raw);
        assert_eq!(info.version(), Some("7.4.0"));
        assert_eq!(info.uptime_seconds(), Some(100));
        assert_eq!(info.connected_clients(), Some(3));
        assert_eq!(info.memory_usage_fraction(), Some(0.5));
        assert_eq!(info.hit_ratio(), Some(0.75));
        assert_eq!(info.instantaneous_ops_per_sec(), Some(42));
        assert_eq!(info.evicted_keys(), Some(0));
    }

    #[test]
    fn base64_decodes_queue_names() {
        // "emails" → "ZW1haWxz"
        assert_eq!(
            base64_decode("ZW1haWxz").and_then(|b| String::from_utf8(b).ok()),
            Some("emails".to_string())
        );
    }

    #[test]
    fn parses_client_list_workers() {
        // emails = ZW1haWxz, named worker "host-1"; one unnamed; one :qe excluded;
        // one non-bull line excluded.
        let raw =
            "id=5 addr=127.0.0.1:51000 age=12 idle=0 name=bull:ZW1haWxz:w:host-1 cmd=brpoplpush\n\
                   id=6 addr=127.0.0.1:51001 age=30 idle=1 name=bull:ZW1haWxz cmd=evalsha\n\
                   id=7 addr=127.0.0.1:51002 age=30 idle=1 name=bull:ZW1haWxz:qe cmd=xread\n\
                   id=8 addr=127.0.0.1:51003 age=99 idle=9 name=some-other-client cmd=ping\n";
        let ws = WorkerInfo::parse_client_list(raw, "bull");
        assert_eq!(ws.len(), 2, "qe + non-bull excluded");
        assert_eq!(ws[0].queue.as_deref(), Some("emails"));
        assert_eq!(ws[0].worker_name.as_deref(), Some("host-1"));
        assert_eq!(ws[0].age_secs, 12);
        assert_eq!(ws[1].worker_name, None, "unnamed worker");
    }

    #[test]
    fn rate_limit_throttle_detection() {
        let throttled = RateLimitStatus {
            counter: Some(100),
            ttl_ms: 500,
            max: Some(100),
            ..Default::default()
        };
        assert!(throttled.is_throttled());
        let under = RateLimitStatus {
            counter: Some(40),
            ttl_ms: 500,
            max: Some(100),
            ..Default::default()
        };
        assert!(!under.is_throttled());
        let manual = RateLimitStatus {
            counter: Some(RateLimitStatus::MAX_SAFE_INTEGER),
            ttl_ms: 1000,
            manual: true,
            ..Default::default()
        };
        assert!(manual.is_throttled());
    }
}
