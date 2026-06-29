//! The BullMQ Redis client: connection management and read operations.
//!
//! All semantics here are derived from the bullmq source (v5.79) — see the
//! Lua command scripts under `bullmq/dist/cjs/commands` and the `queue-getters`
//! / `job` classes. Reads never mutate Redis (unlike bullmq's own `getCounts`,
//! which RPOPs a stale marker — we instead account for the marker in-place).

use std::collections::{BTreeSet, HashMap};

use redis::aio::ConnectionManager;
use redis::{from_redis_value, AsyncCommands, Value as RedisValue};

use crate::error::{Error, Result};
use crate::keys::KeyBuilder;
use crate::types::{
    ActiveJobLock, Job, JobCounts, JobLogs, JobScheduler, JobState, Metrics, MetricsKind,
    QueueSummary, RateLimitStatus, RedisInfo, WorkerInfo,
};

/// The default key prefix BullMQ uses.
pub const DEFAULT_PREFIX: &str = "bull";

/// A connection to a Redis/Valkey server hosting BullMQ queues.
#[derive(Clone)]
pub struct BullClient {
    client: redis::Client,
    conn: ConnectionManager,
    prefix: String,
}

impl BullClient {
    /// Connect to Redis at `url` using `prefix` (default [`DEFAULT_PREFIX`]).
    pub async fn connect(url: &str, prefix: impl Into<String>) -> Result<Self> {
        let client = redis::Client::open(url)?;
        let conn = client.get_connection_manager().await?;
        Ok(Self {
            client,
            conn,
            prefix: prefix.into(),
        })
    }

    /// The underlying [`redis::Client`], for opening a *dedicated* connection
    /// (e.g. a blocking `XREAD` stream reader) separate from the shared
    /// multiplexed [`ConnectionManager`].
    pub fn redis_client(&self) -> redis::Client {
        self.client.clone()
    }

    /// The configured key prefix.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// A key builder for `queue`.
    pub fn keys(&self, queue: &str) -> KeyBuilder {
        KeyBuilder::new(self.prefix.clone(), queue.to_string())
    }

    pub(crate) fn conn(&self) -> ConnectionManager {
        self.conn.clone()
    }

    // -- job schedulers -----------------------------------------------------

    /// List a queue's job schedulers (cron / repeatable), soonest-first.
    /// Mirrors `getJobSchedulers`: read the `repeat` ZSET with scores, then
    /// fetch each scheduler's metadata hash. The score is the next-run epoch ms.
    pub async fn list_job_schedulers(
        &self,
        queue: &str,
        start: isize,
        end: isize,
    ) -> Result<Vec<JobScheduler>> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let raw: Vec<String> = redis::cmd("ZRANGE")
            .arg(kb.repeat())
            .arg(start)
            .arg(end)
            .arg("WITHSCORES")
            .query_async(&mut conn)
            .await?;

        let mut ids = Vec::new();
        let mut scores = Vec::new();
        let mut pipe = redis::pipe();
        for chunk in raw.chunks(2) {
            let [member, score] = chunk else { break };
            pipe.cmd("HGETALL").arg(kb.repeat_scheduler(member));
            ids.push(member.clone());
            scores.push(score.split('.').next().and_then(|s| s.parse::<i64>().ok()));
        }
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let hashes: Vec<HashMap<String, String>> = pipe.query_async(&mut conn).await?;
        Ok(ids
            .into_iter()
            .zip(scores)
            .zip(hashes)
            .filter_map(|((id, score), hash)| JobScheduler::from_hash(&id, score, &hash))
            .collect())
    }

    /// Count of a queue's job schedulers (`ZCARD repeat`).
    pub async fn job_schedulers_count(&self, queue: &str) -> Result<i64> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        Ok(redis::cmd("ZCARD")
            .arg(kb.repeat())
            .query_async(&mut conn)
            .await?)
    }

    // -- workers / busy / health -------------------------------------------

    /// Active jobs in `queue` with their worker-lock TTLs, oldest-first. Reads
    /// only (`LRANGE active`, pipelined `PTTL {id}:lock`, `SMEMBERS stalled`).
    pub async fn list_active_jobs_with_locks(&self, queue: &str) -> Result<Vec<ActiveJobLock>> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let mut ids: Vec<String> = conn.lrange(kb.active(), 0, -1).await?;
        ids.retain(|id| !id.starts_with("0:"));
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let jobs = self.fetch_jobs(&kb, &ids).await?;
        let mut pipe = redis::pipe();
        for id in &ids {
            pipe.cmd("PTTL").arg(kb.job_lock(id));
        }
        let ttls: Vec<i64> = pipe.query_async(&mut conn).await?;
        let stalled: BTreeSet<String> = {
            let v: Vec<String> = conn.smembers(kb.stalled()).await?;
            v.into_iter().collect()
        };
        let mut job_map: HashMap<String, Job> =
            jobs.into_iter().map(|j| (j.id.clone(), j)).collect();
        let mut out = Vec::new();
        for (id, ttl) in ids.iter().zip(ttls) {
            if let Some(mut job) = job_map.remove(id) {
                job.state = Some(JobState::Active);
                out.push(ActiveJobLock {
                    queue: queue.to_string(),
                    job,
                    lock_ttl_ms: ttl,
                    in_stalled_set: stalled.contains(id),
                });
            }
        }
        out.sort_by_key(|a| a.job.processed_on.unwrap_or(i64::MAX));
        Ok(out)
    }

    /// Active jobs across several queues, oldest-first.
    pub async fn list_active_jobs_all(&self, queues: &[String]) -> Result<Vec<ActiveJobLock>> {
        let mut out = Vec::new();
        for q in queues {
            out.extend(self.list_active_jobs_with_locks(q).await?);
        }
        out.sort_by_key(|a| a.job.processed_on.unwrap_or(i64::MAX));
        Ok(out)
    }

    /// Connected BullMQ workers, via Redis `CLIENT LIST` (filtered by prefix).
    pub async fn list_workers(&self) -> Result<Vec<WorkerInfo>> {
        let mut conn = self.conn();
        let raw: String = redis::cmd("CLIENT")
            .arg("LIST")
            .query_async(&mut conn)
            .await?;
        Ok(WorkerInfo::parse_client_list(&raw, self.prefix()))
    }

    /// Live rate-limit + concurrency state for a queue. Reads only.
    pub async fn rate_limit_status(&self, queue: &str) -> Result<RateLimitStatus> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let (counter, ttl_ms, meta, active): (Option<String>, i64, Vec<Option<String>>, i64) =
            redis::pipe()
                .cmd("GET")
                .arg(kb.limiter())
                .cmd("PTTL")
                .arg(kb.limiter())
                .cmd("HMGET")
                .arg(kb.meta())
                .arg("max")
                .arg("duration")
                .arg("concurrency")
                .cmd("LLEN")
                .arg(kb.active())
                .query_async(&mut conn)
                .await?;
        let counter = counter.and_then(|s| s.parse::<i64>().ok());
        let pick = |i: usize| {
            meta.get(i)
                .and_then(|o| o.as_ref())
                .and_then(|s| s.parse().ok())
        };
        Ok(RateLimitStatus {
            counter,
            ttl_ms,
            max: pick(0),
            duration_ms: pick(1),
            concurrency: pick(2),
            active,
            manual: counter == Some(RateLimitStatus::MAX_SAFE_INTEGER),
        })
    }

    // -- discovery ----------------------------------------------------------

    /// Discover all queues by scanning for `{prefix}:*:meta` keys.
    pub async fn discover_queues(&self) -> Result<Vec<String>> {
        let mut conn = self.conn();
        let pattern = format!("{}:*:meta", self.prefix);
        let strip_start = self.prefix.len() + 1; // "{prefix}:"
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut cursor: u64 = 0;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(500)
                .query_async(&mut conn)
                .await?;
            for k in keys {
                if k.len() > strip_start + 5 && k.ends_with(":meta") {
                    let name = &k[strip_start..k.len() - 5];
                    names.insert(name.to_string());
                }
            }
            if next == 0 {
                break;
            }
            cursor = next;
        }
        Ok(names.into_iter().collect())
    }

    // -- counts / status ----------------------------------------------------

    /// Job counts per state, replicating `getCounts-1.lua` (marker-aware,
    /// without the RPOP side effect).
    pub async fn job_counts(&self, queue: &str) -> Result<JobCounts> {
        let kb = self.keys(queue);
        let mut conn = self.conn();

        // Pipeline layout (see indices below).
        let mut pipe = redis::pipe();
        pipe.cmd("LLEN").arg(kb.active()); // 0
        pipe.cmd("LLEN").arg(kb.wait()); // 1
        pipe.cmd("LINDEX").arg(kb.wait()).arg(-1); // 2
        pipe.cmd("ZCARD").arg(kb.waiting_children()); // 3
        pipe.cmd("ZCARD").arg(kb.prioritized()); // 4
        pipe.cmd("ZCARD").arg(kb.completed()); // 5
        pipe.cmd("ZCARD").arg(kb.failed()); // 6
        pipe.cmd("ZCARD").arg(kb.delayed()); // 7
        pipe.cmd("LLEN").arg(kb.paused()); // 8
        pipe.cmd("LINDEX").arg(kb.paused()).arg(-1); // 9

        let res: Vec<RedisValue> = pipe.query_async(&mut conn).await?;
        let int = |i: usize| -> i64 {
            res.get(i)
                .and_then(|v| from_redis_value::<i64>(v).ok())
                .unwrap_or(0)
        };
        let marker = |i: usize| -> Option<String> {
            res.get(i)
                .and_then(|v| from_redis_value::<Option<String>>(v).ok().flatten())
        };

        let mut counts = JobCounts {
            active: int(0),
            waiting: adjust_for_marker(int(1), marker(2)),
            waiting_children: int(3),
            prioritized: int(4),
            completed: int(5),
            failed: int(6),
            delayed: int(7),
            paused: adjust_for_marker(int(8), marker(9)),
        };
        // Keep totals non-negative defensively.
        for s in JobState::ALL {
            if counts.get(s) < 0 {
                counts.set(s, 0);
            }
        }
        Ok(counts)
    }

    /// Whether the queue is paused (`HEXISTS meta paused`).
    pub async fn is_paused(&self, queue: &str) -> Result<bool> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let exists: bool = conn.hexists(kb.meta(), "paused").await?;
        Ok(exists)
    }

    /// The global concurrency for the queue, if configured.
    pub async fn global_concurrency(&self, queue: &str) -> Result<Option<i64>> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let raw: Option<String> = conn.hget(kb.meta(), "concurrency").await?;
        Ok(raw.and_then(|s| s.parse::<i64>().ok()))
    }

    /// A combined summary for the overview screen.
    pub async fn queue_summary(&self, queue: &str) -> Result<QueueSummary> {
        let counts = self.job_counts(queue).await?;
        let is_paused = self.is_paused(queue).await?;
        let global_concurrency = self.global_concurrency(queue).await?;
        Ok(QueueSummary {
            name: queue.to_string(),
            prefix: self.prefix.clone(),
            counts,
            is_paused,
            global_concurrency,
        })
    }

    /// Determine which state a job is currently in (best-effort, like
    /// `Job.getState`). Returns `None` if the job is not in any set.
    pub async fn job_state(&self, queue: &str, id: &str) -> Result<Option<JobState>> {
        let kb = self.keys(queue);
        let mut conn = self.conn();

        // Sorted-set membership via ZSCORE; list membership via LPOS.
        let mut pipe = redis::pipe();
        pipe.cmd("ZSCORE").arg(kb.completed()).arg(id); // 0
        pipe.cmd("ZSCORE").arg(kb.failed()).arg(id); // 1
        pipe.cmd("ZSCORE").arg(kb.delayed()).arg(id); // 2
        pipe.cmd("ZSCORE").arg(kb.prioritized()).arg(id); // 3
        pipe.cmd("ZSCORE").arg(kb.waiting_children()).arg(id); // 4
        pipe.cmd("LPOS").arg(kb.active()).arg(id); // 5
        pipe.cmd("LPOS").arg(kb.wait()).arg(id); // 6
        pipe.cmd("LPOS").arg(kb.paused()).arg(id); // 7
        let res: Vec<RedisValue> = pipe.query_async(&mut conn).await?;
        let present = |i: usize| -> bool {
            res.get(i)
                .map(|v| !matches!(v, RedisValue::Nil))
                .unwrap_or(false)
        };
        Ok(if present(5) {
            Some(JobState::Active)
        } else if present(0) {
            Some(JobState::Completed)
        } else if present(1) {
            Some(JobState::Failed)
        } else if present(2) {
            Some(JobState::Delayed)
        } else if present(3) {
            Some(JobState::Prioritized)
        } else if present(4) {
            Some(JobState::WaitingChildren)
        } else if present(6) {
            Some(JobState::Waiting)
        } else if present(7) {
            Some(JobState::Paused)
        } else {
            None
        })
    }

    // -- job listings -------------------------------------------------------

    /// Job ids for a single state in `[start, end]` (inclusive), newest-first
    /// for sorted sets (matching bull-board's default `asc=false`).
    pub async fn job_ids(
        &self,
        queue: &str,
        state: JobState,
        start: isize,
        end: isize,
    ) -> Result<Vec<String>> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let key = kb.state(state);
        let ids: Vec<String> = if state.is_list() {
            let mut ids: Vec<String> = conn.lrange(&key, start, end).await?;
            // Drop any deprecated in-list marker ("0:...").
            ids.retain(|id| !id.starts_with("0:"));
            ids
        } else {
            conn.zrevrange(&key, start, end).await?
        };
        Ok(ids)
    }

    /// Fetch full jobs for a list of ids (pipelined HGETALL), preserving order.
    async fn fetch_jobs(&self, kb: &KeyBuilder, ids: &[String]) -> Result<Vec<Job>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn();
        let mut pipe = redis::pipe();
        for id in ids {
            pipe.cmd("HGETALL").arg(kb.job(id));
        }
        let hashes: Vec<HashMap<String, String>> = pipe.query_async(&mut conn).await?;
        let mut jobs = Vec::with_capacity(ids.len());
        for (id, hash) in ids.iter().zip(hashes.into_iter()) {
            if let Some(job) = Job::from_hash(id, &hash) {
                jobs.push(job);
            }
        }
        Ok(jobs)
    }

    /// Get jobs for a single state in `[start, end]`.
    pub async fn get_jobs(
        &self,
        queue: &str,
        state: JobState,
        start: isize,
        end: isize,
    ) -> Result<Vec<Job>> {
        let kb = self.keys(queue);
        let ids = self.job_ids(queue, state, start, end).await?;
        let mut jobs = self.fetch_jobs(&kb, &ids).await?;
        for j in &mut jobs {
            j.state = Some(state);
        }
        Ok(jobs)
    }

    /// The underlying states a bull-board *status* expands to. Mirrors
    /// bullmq's `sanitizeJobTypes`: requesting `waiting` also pulls `paused`.
    pub fn status_states(status: JobState) -> Vec<JobState> {
        if status == JobState::Waiting {
            vec![JobState::Waiting, JobState::Paused]
        } else {
            vec![status]
        }
    }

    /// Job ids for a bull-board status (with `waiting` → `wait` + `paused`),
    /// concatenating each underlying state's `[start, end]` range in order.
    pub async fn list_status_ids(
        &self,
        queue: &str,
        status: JobState,
        start: isize,
        end: isize,
    ) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for state in Self::status_states(status) {
            out.extend(self.job_ids(queue, state, start, end).await?);
        }
        Ok(out)
    }

    /// Jobs for a bull-board status (with `waiting` → `wait` + `paused`).
    pub async fn list_status_jobs(
        &self,
        queue: &str,
        status: JobState,
        start: isize,
        end: isize,
    ) -> Result<Vec<Job>> {
        let kb = self.keys(queue);
        let mut out = Vec::new();
        for state in Self::status_states(status) {
            let ids = self.job_ids(queue, state, start, end).await?;
            let mut jobs = self.fetch_jobs(&kb, &ids).await?;
            for j in &mut jobs {
                j.state = Some(state);
            }
            out.append(&mut jobs);
        }
        Ok(out)
    }

    /// Get the "latest" jobs across the given states, concatenating each
    /// state's `[start, end]` range in order — mirrors bull-board's behaviour
    /// for the `latest` tab (`getJobs(allStatuses, start, end)`).
    pub async fn get_jobs_latest(
        &self,
        queue: &str,
        states: &[JobState],
        start: isize,
        end: isize,
    ) -> Result<Vec<Job>> {
        let kb = self.keys(queue);
        let mut out = Vec::new();
        for &state in states {
            let ids = self.job_ids(queue, state, start, end).await?;
            let mut jobs = self.fetch_jobs(&kb, &ids).await?;
            for j in &mut jobs {
                j.state = Some(state);
            }
            out.append(&mut jobs);
        }
        Ok(out)
    }

    /// Fetch a single job by id. Returns `None` if it does not exist.
    pub async fn get_job(&self, queue: &str, id: &str) -> Result<Option<Job>> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let hash: HashMap<String, String> = conn.hgetall(kb.job(id)).await?;
        Ok(Job::from_hash(id, &hash))
    }

    /// Fetch a job by id or error if missing.
    pub async fn require_job(&self, queue: &str, id: &str) -> Result<Job> {
        self.get_job(queue, id)
            .await?
            .ok_or_else(|| Error::JobNotFound {
                queue: queue.to_string(),
                id: id.to_string(),
            })
    }

    // -- logs / metrics -----------------------------------------------------

    /// Fetch job logs (`{id}:logs` list), oldest-first, plus the total count.
    pub async fn job_logs(
        &self,
        queue: &str,
        id: &str,
        start: isize,
        end: isize,
    ) -> Result<JobLogs> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let logs_key = kb.job_logs(id);
        let mut pipe = redis::pipe();
        pipe.cmd("LRANGE").arg(&logs_key).arg(start).arg(end);
        pipe.cmd("LLEN").arg(&logs_key);
        let (logs, count): (Vec<String>, i64) = pipe.query_async(&mut conn).await?;
        Ok(JobLogs { logs, count })
    }

    /// Fetch historical metrics (`metrics:<kind>` + `:data`), mirroring
    /// `getMetrics-2.lua` / `Queue.getMetrics`.
    pub async fn metrics(
        &self,
        queue: &str,
        kind: MetricsKind,
        start: isize,
        end: isize,
    ) -> Result<Metrics> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let meta_key = kb.metrics(kind.key());
        let data_key = kb.metrics_data(kind.key());

        let mut pipe = redis::pipe();
        pipe.cmd("HMGET")
            .arg(&meta_key)
            .arg("count")
            .arg("prevTS")
            .arg("prevCount");
        pipe.cmd("LRANGE").arg(&data_key).arg(start).arg(end);
        pipe.cmd("LLEN").arg(&data_key);
        let (meta, data, _num): (Vec<Option<String>>, Vec<String>, i64) =
            pipe.query_async(&mut conn).await?;

        let parse = |i: usize| -> i64 {
            meta.get(i)
                .and_then(|o| o.as_ref())
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0)
        };
        let data = data
            .into_iter()
            .map(|s| s.parse::<f64>().map(|f| f as i64).unwrap_or(0))
            .collect();
        Ok(Metrics {
            count: parse(0),
            prev_ts: parse(1),
            prev_count: parse(2),
            data,
        })
    }

    // -- server info --------------------------------------------------------

    /// Fetch and parse Redis `INFO`.
    pub async fn redis_info(&self) -> Result<RedisInfo> {
        let mut conn = self.conn();
        let raw: String = redis::cmd("INFO").query_async(&mut conn).await?;
        Ok(RedisInfo::parse(&raw))
    }

    /// The raw Redis `INFO` string.
    pub async fn redis_info_raw(&self) -> Result<String> {
        let mut conn = self.conn();
        let raw: String = redis::cmd("INFO").query_async(&mut conn).await?;
        Ok(raw)
    }

    // -- flows --------------------------------------------------------------

    /// Walk up `parentKey` links to find the flow root. Returns
    /// `(queue, jobId)` of the root, or `None` if the job has no parent.
    pub async fn find_flow_root(&self, queue: &str, id: &str) -> Result<Option<(String, String)>> {
        let mut current_queue = queue.to_string();
        let mut current_id = id.to_string();
        let mut had_parent = false;
        // Bound the walk to avoid cycles.
        for _ in 0..64 {
            let job = match self.get_job(&current_queue, &current_id).await? {
                Some(j) => j,
                None => break,
            };
            match job.parent_key.as_deref() {
                Some(pk) => {
                    if let Some((pq, pid)) = self.parse_job_key(pk) {
                        had_parent = true;
                        current_queue = pq;
                        current_id = pid;
                    } else {
                        break;
                    }
                }
                None => break,
            }
        }
        Ok(if had_parent {
            Some((current_queue, current_id))
        } else {
            None
        })
    }

    /// Parse a fully-qualified job key `{prefix}:{queue}:{id}` into
    /// `(queue, id)`. The queue name may contain `:`; the id is taken as the
    /// final segment.
    pub fn parse_job_key(&self, key: &str) -> Option<(String, String)> {
        let rest = key.strip_prefix(&format!("{}:", self.prefix))?;
        let idx = rest.rfind(':')?;
        let queue = rest[..idx].to_string();
        let id = rest[idx + 1..].to_string();
        Some((queue, id))
    }

    /// Direct child job keys of a job, gathered from its dependency structures
    /// (`:processed`, `:dependencies`, `:failed`, `:unsuccessful`).
    pub async fn child_job_keys(&self, queue: &str, id: &str) -> Result<Vec<String>> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let processed_key = kb.key(&format!("{id}:processed"));
        let deps_key = kb.key(&format!("{id}:dependencies"));
        let failed_key = kb.key(&format!("{id}:failed"));
        let unsuccessful_key = kb.key(&format!("{id}:unsuccessful"));

        let mut pipe = redis::pipe();
        pipe.cmd("HKEYS").arg(&processed_key); // completed children
        pipe.cmd("SMEMBERS").arg(&deps_key); // pending children
        pipe.cmd("HKEYS").arg(&failed_key); // ignored children
        pipe.cmd("ZRANGE").arg(&unsuccessful_key).arg(0).arg(-1); // failed children
        let (processed, deps, failed, unsuccessful): (
            Vec<String>,
            Vec<String>,
            Vec<String>,
            Vec<String>,
        ) = pipe.query_async(&mut conn).await?;

        let mut keys = Vec::new();
        keys.extend(deps);
        keys.extend(processed);
        keys.extend(failed);
        keys.extend(unsuccessful);
        Ok(keys)
    }

    /// Build the flow tree rooted at `(queue, id)`, recursing into children up
    /// to `max_depth` levels.
    pub async fn get_flow_tree(
        &self,
        queue: &str,
        id: &str,
        max_depth: usize,
    ) -> Result<Option<crate::types::FlowNode>> {
        let job = match self.get_job(queue, id).await? {
            Some(j) => j,
            None => return Ok(None),
        };
        let state = self.job_state(queue, id).await?;
        let mut children = Vec::new();
        if max_depth > 0 {
            let child_keys = self.child_job_keys(queue, id).await?;
            for ck in child_keys {
                if let Some((cq, cid)) = self.parse_job_key(&ck) {
                    if let Some(node) =
                        Box::pin(self.get_flow_tree(&cq, &cid, max_depth - 1)).await?
                    {
                        children.push(node);
                    }
                }
            }
        }
        Ok(Some(crate::types::FlowNode {
            queue_qualified_name: format!("{}:{}", self.prefix, queue),
            queue_name: queue.to_string(),
            job,
            state,
            children,
        }))
    }
}

/// Adjust a list length for a deprecated in-list marker (`0:...` at the tail).
fn adjust_for_marker(len: i64, marker: Option<String>) -> i64 {
    match marker {
        Some(m) if m.starts_with("0:") => {
            if len > 1 {
                len - 1
            } else {
                0
            }
        }
        _ => len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_adjustment() {
        assert_eq!(adjust_for_marker(5, Some("0:123".into())), 4);
        assert_eq!(adjust_for_marker(1, Some("0:123".into())), 0);
        assert_eq!(adjust_for_marker(5, Some("42".into())), 5);
        assert_eq!(adjust_for_marker(5, None), 5);
    }
}
