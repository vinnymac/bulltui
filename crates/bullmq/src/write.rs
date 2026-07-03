//! Admin write operations (pause/resume, retry, promote, clean, remove, add).
//!
//! Each operation reproduces the observable BullMQ effect, including the events
//! stream and the ZSET marker that workers `BZPOPMIN` on. Verified behaviourally
//! in the e2e write tests.

use std::time::{SystemTime, UNIX_EPOCH};

use redis::Value as RedisValue;
use serde_json::{Map, Value};

use crate::client::BullClient;
use crate::error::{Error, Result};
use crate::keys::KeyBuilder;
use crate::types::JobState;

const MAX_EVENTS: i64 = 10_000;
const PRIORITY_MULT: f64 = 4_294_967_296.0; // 2^32
const DELAY_MULT: i64 = 4096; // 2^12

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl BullClient {
    // -- queue: pause / resume ---------------------------------------------

    /// Pause a queue.
    pub async fn pause(&self, queue: &str) -> Result<()> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let has_jobs: bool = redis::cmd("EXISTS")
            .arg(kb.wait())
            .query_async(&mut conn)
            .await?;
        let mut pipe = redis::pipe();
        if has_jobs {
            pipe.cmd("RENAME").arg(kb.wait()).arg(kb.paused()).ignore();
        }
        pipe.cmd("HSET")
            .arg(kb.meta())
            .arg("paused")
            .arg(1)
            .ignore();
        pipe.cmd("DEL").arg(kb.marker()).ignore();
        pipe.cmd("XADD")
            .arg(kb.events())
            .arg("*")
            .arg("event")
            .arg("paused")
            .ignore();
        pipe.query_async::<()>(&mut conn).await?;
        Ok(())
    }

    /// Resume a queue.
    pub async fn resume(&self, queue: &str) -> Result<()> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let has_jobs: bool = redis::cmd("EXISTS")
            .arg(kb.paused())
            .query_async(&mut conn)
            .await?;
        let prioritized: i64 = redis::cmd("ZCARD")
            .arg(kb.prioritized())
            .query_async(&mut conn)
            .await?;

        let mut pipe = redis::pipe();
        if has_jobs {
            pipe.cmd("RENAME").arg(kb.paused()).arg(kb.wait()).ignore();
        }
        pipe.cmd("HDEL").arg(kb.meta()).arg("paused").ignore();
        pipe.query_async::<()>(&mut conn).await?;

        if has_jobs || prioritized > 0 {
            redis::cmd("ZADD")
                .arg(kb.marker())
                .arg(0)
                .arg("0")
                .query_async::<()>(&mut conn)
                .await?;
        } else {
            self.add_delay_marker_if_needed(&kb).await?;
        }
        redis::cmd("XADD")
            .arg(kb.events())
            .arg("*")
            .arg("event")
            .arg("resumed")
            .query_async::<()>(&mut conn)
            .await?;
        Ok(())
    }

    /// Pause all discovered queues. Returns the number paused.
    pub async fn pause_all(&self) -> Result<usize> {
        let queues = self.discover_queues().await?;
        for q in &queues {
            self.pause(q).await?;
        }
        Ok(queues.len())
    }

    /// Resume all discovered queues. Returns the number resumed.
    pub async fn resume_all(&self) -> Result<usize> {
        let queues = self.discover_queues().await?;
        for q in &queues {
            self.resume(q).await?;
        }
        Ok(queues.len())
    }

    // -- queue: empty / obliterate -----------------------------------------

    /// Empty a queue: removes waiting, paused, and prioritized jobs and their
    /// data, leaving active, delayed, completed, and failed untouched.
    /// Returns the number of jobs removed.
    pub async fn empty(&self, queue: &str) -> Result<u64> {
        let kb = self.keys(queue);
        let mut conn = self.conn();

        let mut ids: Vec<String> = Vec::new();
        let wait: Vec<String> = redis::cmd("LRANGE")
            .arg(kb.wait())
            .arg(0)
            .arg(-1)
            .query_async(&mut conn)
            .await?;
        let paused: Vec<String> = redis::cmd("LRANGE")
            .arg(kb.paused())
            .arg(0)
            .arg(-1)
            .query_async(&mut conn)
            .await?;
        let prioritized: Vec<String> = redis::cmd("ZRANGE")
            .arg(kb.prioritized())
            .arg(0)
            .arg(-1)
            .query_async(&mut conn)
            .await?;
        for id in wait.into_iter().chain(paused).chain(prioritized) {
            if !id.starts_with("0:") {
                ids.push(id);
            }
        }

        for id in &ids {
            self.delete_job_keys(&kb, id).await?;
            self.detach_from_parent(&kb, id).await?;
        }
        redis::pipe()
            .cmd("DEL")
            .arg(kb.wait())
            .arg(kb.paused())
            .arg(kb.prioritized())
            .ignore()
            .cmd("XADD")
            .arg(kb.events())
            .arg("*")
            .arg("event")
            .arg("cleaned")
            .arg("count")
            .arg(ids.len())
            .ignore()
            .query_async::<()>(&mut conn)
            .await?;
        Ok(ids.len() as u64)
    }

    /// Obliterate a queue: destroys all of its keys.
    /// Refuses if the queue is not paused or has active jobs.
    pub async fn obliterate(&self, queue: &str) -> Result<()> {
        let kb = self.keys(queue);
        let mut conn = self.conn();

        if !self.is_paused(queue).await? {
            return Err(Error::Refused(format!(
                "queue {queue} must be paused before obliterating"
            )));
        }
        let active_len: i64 = redis::cmd("LLEN")
            .arg(kb.active())
            .query_async(&mut conn)
            .await?;
        if active_len > 0 {
            return Err(Error::Refused(format!(
                "queue {queue} has {active_len} active job(s); cannot obliterate"
            )));
        }

        // Gather all job ids across every state and delete their data.
        let mut ids: Vec<String> = Vec::new();
        for state in JobState::ALL {
            let key = kb.state(state);
            let some: Vec<String> = if state.is_list() {
                redis::cmd("LRANGE")
                    .arg(&key)
                    .arg(0)
                    .arg(-1)
                    .query_async(&mut conn)
                    .await?
            } else {
                redis::cmd("ZRANGE")
                    .arg(&key)
                    .arg(0)
                    .arg(-1)
                    .query_async(&mut conn)
                    .await?
            };
            ids.extend(some.into_iter().filter(|id| !id.starts_with("0:")));
        }
        for id in &ids {
            self.delete_job_keys(&kb, id).await?;
        }

        // The `repeat` ZSET is only the index; each scheduler also has a
        // `repeat:{id}` metadata hash. Delete the hashes explicitly to avoid
        // orphans. (There is no separate `jobschedulers` key - `repeat` is it.)
        let scheduler_ids: Vec<String> = redis::cmd("ZRANGE")
            .arg(kb.repeat())
            .arg(0)
            .arg(-1)
            .query_async(&mut conn)
            .await?;

        // Delete every infrastructure key for the queue.
        let mut pipe = redis::pipe();
        for state in JobState::ALL {
            pipe.cmd("DEL").arg(kb.state(state)).ignore();
        }
        for id in &scheduler_ids {
            pipe.cmd("DEL").arg(kb.repeat_scheduler(id)).ignore();
        }
        for suffix in [
            "meta",
            "id",
            "events",
            "marker",
            "pc",
            "stalled",
            "stalled-check",
            "repeat",
            "delay",
            "metrics:completed",
            "metrics:completed:data",
            "metrics:failed",
            "metrics:failed:data",
        ] {
            pipe.cmd("DEL").arg(kb.key(suffix)).ignore();
        }
        pipe.query_async::<()>(&mut conn).await?;
        Ok(())
    }

    // -- queue: clean -------------------------------------------------------

    /// Clean jobs from a state older than `grace_ms` (bull-board uses 5000ms),
    /// up to `limit` (0 = unlimited). Returns the removed job ids.
    pub async fn clean(
        &self,
        queue: &str,
        status: JobState,
        grace_ms: i64,
        limit: i64,
    ) -> Result<Vec<String>> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let cutoff = now_ms() - grace_ms;
        let key = kb.state(status);

        // Candidates, oldest-first.
        let candidates: Vec<String> = if status.is_list() {
            redis::cmd("LRANGE")
                .arg(&key)
                .arg(0)
                .arg(-1)
                .query_async(&mut conn)
                .await?
        } else {
            redis::cmd("ZRANGE")
                .arg(&key)
                .arg(0)
                .arg(-1)
                .query_async(&mut conn)
                .await?
        };
        let candidates: Vec<String> = candidates
            .into_iter()
            .filter(|id| !id.starts_with("0:"))
            .collect();

        // The job timestamp field that determines age, per state.
        let ts_field = match status {
            JobState::Completed | JobState::Failed => "finishedOn",
            JobState::Delayed | JobState::Active => "processedOn",
            _ => "timestamp",
        };

        let mut removed: Vec<String> = Vec::new();
        for id in &candidates {
            if limit > 0 && (removed.len() as i64) >= limit {
                break;
            }
            let raw: Vec<Option<String>> = redis::cmd("HMGET")
                .arg(kb.job(id))
                .arg(ts_field)
                .arg("timestamp")
                .query_async(&mut conn)
                .await?;
            let ts = raw
                .first()
                .and_then(|o| o.as_ref())
                .and_then(|s| s.parse::<i64>().ok())
                .or_else(|| {
                    raw.get(1)
                        .and_then(|o| o.as_ref())
                        .and_then(|s| s.parse::<i64>().ok())
                })
                .unwrap_or(0);
            if ts <= cutoff {
                if status.is_list() {
                    redis::cmd("LREM")
                        .arg(&key)
                        .arg(0)
                        .arg(id)
                        .query_async::<()>(&mut conn)
                        .await?;
                } else {
                    redis::cmd("ZREM")
                        .arg(&key)
                        .arg(id)
                        .query_async::<()>(&mut conn)
                        .await?;
                }
                self.delete_job_keys(&kb, id).await?;
                self.detach_from_parent(&kb, id).await?;
                removed.push(id.clone());
            }
        }

        redis::cmd("XADD")
            .arg(kb.events())
            .arg("*")
            .arg("event")
            .arg("cleaned")
            .arg("count")
            .arg(removed.len())
            .query_async::<()>(&mut conn)
            .await?;
        Ok(removed)
    }

    // -- job: retry ---------------------------------------------------------

    /// Retry a single failed or completed job. Determines the current state automatically.
    pub async fn retry_job(&self, queue: &str, id: &str) -> Result<()> {
        let mut conn = self.conn();
        let kb = self.keys(queue);
        let in_failed: Option<f64> = redis::cmd("ZSCORE")
            .arg(kb.failed())
            .arg(id)
            .query_async(&mut conn)
            .await?;
        let state = if in_failed.is_some() {
            JobState::Failed
        } else {
            let in_completed: Option<f64> = redis::cmd("ZSCORE")
                .arg(kb.completed())
                .arg(id)
                .query_async(&mut conn)
                .await?;
            if in_completed.is_some() {
                JobState::Completed
            } else {
                return Err(Error::Refused(format!(
                    "job {id} is not in a retriable state (failed/completed)"
                )));
            }
        };
        self.reprocess_job(&kb, id, state).await
    }

    /// Retry all jobs in `status` (failed or completed). Returns the count.
    pub async fn retry_all(&self, queue: &str, status: JobState) -> Result<usize> {
        if !matches!(status, JobState::Failed | JobState::Completed) {
            return Err(Error::InvalidArgument(format!(
                "{} is not a retriable status",
                status.status_str()
            )));
        }
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let ids: Vec<String> = redis::cmd("ZRANGE")
            .arg(kb.state(status))
            .arg(0)
            .arg(-1)
            .query_async(&mut conn)
            .await?;
        let mut count = 0;
        for id in &ids {
            // Ignore individual failures (job may have moved) but keep going.
            if self.reprocess_job(&kb, id, status).await.is_ok() {
                count += 1;
            }
        }
        Ok(count)
    }

    async fn reprocess_job(&self, kb: &KeyBuilder, id: &str, state: JobState) -> Result<()> {
        let mut conn = self.conn();
        let job_key = kb.job(id);

        let exists: bool = redis::cmd("EXISTS")
            .arg(&job_key)
            .query_async(&mut conn)
            .await?;
        if !exists {
            return Err(Error::JobNotFound {
                queue: kb.name().to_string(),
                id: id.to_string(),
            });
        }
        // Remove from the source set.
        let removed: i64 = redis::cmd("ZREM")
            .arg(kb.state(state))
            .arg(id)
            .query_async(&mut conn)
            .await?;
        if removed != 1 {
            return Err(Error::Refused(format!(
                "job {id} was not found in {} set",
                state.status_str()
            )));
        }

        let lifo = self.job_lifo(kb, id).await?;
        let prop = if state == JobState::Failed {
            "failedReason"
        } else {
            "returnvalue"
        };
        redis::cmd("HDEL")
            .arg(&job_key)
            .arg("finishedOn")
            .arg("processedOn")
            .arg(prop)
            .query_async::<()>(&mut conn)
            .await?;

        let (target, paused_or_maxed) = self.target_queue_list(kb).await?;
        let push = if lifo { "RPUSH" } else { "LPUSH" };
        redis::cmd(push)
            .arg(&target)
            .arg(id)
            .query_async::<()>(&mut conn)
            .await?;
        if !paused_or_maxed {
            redis::cmd("ZADD")
                .arg(kb.marker())
                .arg(0)
                .arg("0")
                .query_async::<()>(&mut conn)
                .await?;
        }

        // Re-attach to parent's pending dependencies if applicable.
        let parent_key: Option<String> = redis::cmd("HGET")
            .arg(&job_key)
            .arg("parentKey")
            .query_async(&mut conn)
            .await?;
        if let Some(parent_key) = parent_key {
            let parent_exists: bool = redis::cmd("EXISTS")
                .arg(&parent_key)
                .query_async(&mut conn)
                .await?;
            if parent_exists {
                let reattach = if state == JobState::Failed {
                    let z: i64 = redis::cmd("ZREM")
                        .arg(format!("{parent_key}:unsuccessful"))
                        .arg(&job_key)
                        .query_async(&mut conn)
                        .await?;
                    let h: i64 = redis::cmd("HDEL")
                        .arg(format!("{parent_key}:failed"))
                        .arg(&job_key)
                        .query_async(&mut conn)
                        .await?;
                    z == 1 || h == 1
                } else {
                    let h: i64 = redis::cmd("HDEL")
                        .arg(format!("{parent_key}:processed"))
                        .arg(&job_key)
                        .query_async(&mut conn)
                        .await?;
                    h == 1
                };
                if reattach {
                    redis::cmd("SADD")
                        .arg(format!("{parent_key}:dependencies"))
                        .arg(&job_key)
                        .query_async::<()>(&mut conn)
                        .await?;
                }
            }
        }

        self.emit_waiting(kb, id, Some(state.status_str())).await?;
        Ok(())
    }

    // -- job: promote -------------------------------------------------------

    /// Promote a delayed job to waiting.
    pub async fn promote_job(&self, queue: &str, id: &str) -> Result<()> {
        let kb = self.keys(queue);
        let mut conn = self.conn();

        let removed: i64 = redis::cmd("ZREM")
            .arg(kb.delayed())
            .arg(id)
            .query_async(&mut conn)
            .await?;
        if removed != 1 {
            return Err(Error::Refused(format!("job {id} is not delayed")));
        }
        let job_key = kb.job(id);
        let priority: i64 = redis::cmd("HGET")
            .arg(&job_key)
            .arg("priority")
            .query_async::<Option<String>>(&mut conn)
            .await?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let (target, paused_or_maxed) = self.target_queue_list(&kb).await?;
        // Drop a deprecated delayed marker at the head of the target list.
        let head: Option<String> = redis::cmd("LINDEX")
            .arg(&target)
            .arg(0)
            .query_async(&mut conn)
            .await?;
        if head
            .as_deref()
            .map(|h| h.starts_with("0:"))
            .unwrap_or(false)
        {
            redis::cmd("LPOP")
                .arg(&target)
                .query_async::<()>(&mut conn)
                .await?;
        }

        if priority == 0 {
            redis::cmd("LPUSH")
                .arg(&target)
                .arg(id)
                .query_async::<()>(&mut conn)
                .await?;
            if !paused_or_maxed {
                redis::cmd("ZADD")
                    .arg(kb.marker())
                    .arg(0)
                    .arg("0")
                    .query_async::<()>(&mut conn)
                    .await?;
            }
        } else {
            self.add_job_with_priority(&kb, priority, id, paused_or_maxed)
                .await?;
        }

        redis::cmd("HSET")
            .arg(&job_key)
            .arg("delay")
            .arg(0)
            .query_async::<()>(&mut conn)
            .await?;
        self.emit_waiting(&kb, id, Some("delayed")).await?;
        Ok(())
    }

    /// Promote all delayed jobs. Returns the count.
    pub async fn promote_all(&self, queue: &str) -> Result<usize> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let ids: Vec<String> = redis::cmd("ZRANGE")
            .arg(kb.delayed())
            .arg(0)
            .arg(-1)
            .query_async(&mut conn)
            .await?;
        let mut count = 0;
        for id in &ids {
            if self.promote_job(queue, id).await.is_ok() {
                count += 1;
            }
        }
        Ok(count)
    }

    // -- job: remove / clean -----------------------------------------------

    /// Remove a job and all of its data. Refuses if the job is active/locked.
    /// Removes children recursively.
    pub async fn remove_job(&self, queue: &str, id: &str) -> Result<()> {
        self.remove_job_inner(queue, id, true, 0).await
    }

    fn remove_job_inner<'a>(
        &'a self,
        queue: &'a str,
        id: &'a str,
        remove_children: bool,
        depth: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let kb = self.keys(queue);
            let mut conn = self.conn();
            let job_key = kb.job(id);

            let exists: bool = redis::cmd("EXISTS")
                .arg(&job_key)
                .query_async(&mut conn)
                .await?;
            if !exists {
                return Ok(()); // already gone
            }
            let locked: bool = redis::cmd("EXISTS")
                .arg(kb.job_lock(id))
                .query_async(&mut conn)
                .await?;
            let active_pos: Option<i64> = redis::cmd("LPOS")
                .arg(kb.active())
                .arg(id)
                .query_async(&mut conn)
                .await?;
            if locked || active_pos.is_some() {
                return Err(Error::Refused(format!(
                    "job {id} is active/locked; cannot remove"
                )));
            }

            if remove_children && depth < 64 {
                let child_keys = self.child_job_keys(queue, id).await?;
                for ck in child_keys {
                    if let Some((cq, cid)) = self.parse_job_key(&ck) {
                        let _ = self.remove_job_inner(&cq, &cid, true, depth + 1).await;
                    }
                }
            }

            self.detach_from_parent(&kb, id).await?;
            self.remove_from_states(&kb, id).await?;
            self.delete_job_keys(&kb, id).await?;
            redis::cmd("XADD")
                .arg(kb.events())
                .arg("*")
                .arg("event")
                .arg("removed")
                .arg("jobId")
                .arg(id)
                .query_async::<()>(&mut conn)
                .await?;
            Ok(())
        })
    }

    /// bull-board's job "Clean" action: remove the job scheduler if the job is
    /// produced by one, otherwise remove the job.
    pub async fn clean_job(&self, queue: &str, id: &str) -> Result<()> {
        let job = self.require_job(queue, id).await?;
        if let Some(rjk) = job.repeat_job_key.as_deref().filter(|s| !s.is_empty()) {
            let removed = self.remove_job_scheduler(queue, rjk).await?;
            if !removed {
                return Err(Error::Refused(format!("failed to remove scheduler {rjk}")));
            }
            Ok(())
        } else {
            self.remove_job(queue, id).await
        }
    }

    /// Remove a job scheduler and its next scheduled job.
    /// Returns true if the scheduler existed.
    pub async fn remove_job_scheduler(&self, queue: &str, scheduler_id: &str) -> Result<bool> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let repeat_key = kb.repeat();

        let millis: Option<String> = redis::cmd("ZSCORE")
            .arg(&repeat_key)
            .arg(scheduler_id)
            .query_async(&mut conn)
            .await?;
        if let Some(millis) = millis {
            // ZSCORE may return a float string; normalise to an integer when possible.
            let millis = millis.split('.').next().unwrap_or(&millis).to_string();
            let delayed_job_id = format!("repeat:{scheduler_id}:{millis}");
            let zrem: i64 = redis::cmd("ZREM")
                .arg(kb.delayed())
                .arg(&delayed_job_id)
                .query_async(&mut conn)
                .await?;
            if zrem == 1 {
                self.delete_job_keys(&kb, &delayed_job_id).await?;
                redis::cmd("XADD")
                    .arg(kb.events())
                    .arg("*")
                    .arg("event")
                    .arg("removed")
                    .arg("jobId")
                    .arg(&delayed_job_id)
                    .arg("prev")
                    .arg("delayed")
                    .query_async::<()>(&mut conn)
                    .await?;
            }
        }
        let removed: i64 = redis::cmd("ZREM")
            .arg(&repeat_key)
            .arg(scheduler_id)
            .query_async(&mut conn)
            .await?;
        if removed == 1 {
            redis::cmd("DEL")
                .arg(kb.repeat_scheduler(scheduler_id))
                .query_async::<()>(&mut conn)
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // -- job: add / update --------------------------------------------------

    /// Add a job to the queue, routing to delayed, prioritized, or wait as
    /// appropriate. `opts` is a JSON object with BullMQ option names
    /// (`delay`, `priority`, `attempts`, `lifo`, `jobId`, `failParentOnFailure`).
    /// Returns the new job id.
    pub async fn add_job(
        &self,
        queue: &str,
        name: &str,
        data: &Value,
        opts: &Value,
    ) -> Result<String> {
        let kb = self.keys(queue);
        let mut conn = self.conn();

        let opts_obj = opts.as_object().cloned().unwrap_or_default();
        let delay = opts_obj.get("delay").and_then(Value::as_i64).unwrap_or(0);
        let priority = opts_obj
            .get("priority")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let lifo = opts_obj
            .get("lifo")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let custom_id = opts_obj
            .get("jobId")
            .and_then(Value::as_str)
            .map(str::to_string);

        // Resolve job id.
        let id = match custom_id {
            Some(cid) => {
                let exists: bool = redis::cmd("EXISTS")
                    .arg(kb.job(&cid))
                    .query_async(&mut conn)
                    .await?;
                if exists {
                    return Err(Error::Refused(format!("job {cid} already exists")));
                }
                cid
            }
            None => {
                let n: i64 = redis::cmd("INCR")
                    .arg(kb.id_counter())
                    .query_async(&mut conn)
                    .await?;
                n.to_string()
            }
        };

        let timestamp = now_ms();
        let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
        let opts_str = encode_opts(&opts_obj);

        // Store the job hash + "added" event.
        let mut hset = redis::cmd("HSET");
        hset.arg(kb.job(&id))
            .arg("name")
            .arg(name)
            .arg("data")
            .arg(&data_str)
            .arg("opts")
            .arg(&opts_str)
            .arg("timestamp")
            .arg(timestamp)
            .arg("delay")
            .arg(delay)
            .arg("priority")
            .arg(priority);
        hset.query_async::<()>(&mut conn).await?;
        redis::cmd("XADD")
            .arg(kb.events())
            .arg("MAXLEN")
            .arg("~")
            .arg(MAX_EVENTS)
            .arg("*")
            .arg("event")
            .arg("added")
            .arg("jobId")
            .arg(&id)
            .arg("name")
            .arg(name)
            .query_async::<()>(&mut conn)
            .await?;

        if delay > 0 {
            self.add_delayed(&kb, &id, timestamp, delay).await?;
        } else if priority > 0 {
            let (_, paused_or_maxed) = self.target_queue_list(&kb).await?;
            self.add_job_with_priority(&kb, priority, &id, paused_or_maxed)
                .await?;
            self.emit_waiting(&kb, &id, None).await?;
        } else {
            let (target, paused_or_maxed) = self.target_queue_list(&kb).await?;
            let push = if lifo { "RPUSH" } else { "LPUSH" };
            redis::cmd(push)
                .arg(&target)
                .arg(&id)
                .query_async::<()>(&mut conn)
                .await?;
            if !paused_or_maxed {
                redis::cmd("ZADD")
                    .arg(kb.marker())
                    .arg(0)
                    .arg("0")
                    .query_async::<()>(&mut conn)
                    .await?;
            }
            self.emit_waiting(&kb, &id, None).await?;
        }

        Ok(id)
    }

    /// Duplicate a job: add a new job with the same name, data and options
    /// (a fresh id is generated). Returns the new job id.
    pub async fn duplicate_job(&self, queue: &str, id: &str) -> Result<String> {
        let job = self.require_job(queue, id).await?;
        let data: Value =
            serde_json::from_str(&job.data).unwrap_or(Value::Object(Default::default()));
        let mut opts = job.opts.clone();
        if let Some(obj) = opts.as_object_mut() {
            obj.remove("jobId"); // generate a new id
        }
        self.add_job(queue, &job.name, &data, &opts).await
    }

    /// Update a job's `data` field.
    pub async fn update_job_data(&self, queue: &str, id: &str, data: &Value) -> Result<()> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let exists: bool = redis::cmd("EXISTS")
            .arg(kb.job(id))
            .query_async(&mut conn)
            .await?;
        if !exists {
            return Err(Error::JobNotFound {
                queue: queue.to_string(),
                id: id.to_string(),
            });
        }
        let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
        redis::cmd("HSET")
            .arg(kb.job(id))
            .arg("data")
            .arg(data_str)
            .query_async::<()>(&mut conn)
            .await?;
        Ok(())
    }

    /// Change the delay of a delayed job.
    pub async fn change_delay(&self, queue: &str, id: &str, delay_ms: i64) -> Result<()> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let job_key = kb.job(id);
        let exists: bool = redis::cmd("EXISTS")
            .arg(&job_key)
            .query_async(&mut conn)
            .await?;
        if !exists {
            return Err(Error::JobNotFound {
                queue: queue.to_string(),
                id: id.to_string(),
            });
        }
        let (score, _ts) = self.delayed_score(&kb, now_ms(), delay_ms).await?;
        let removed: i64 = redis::cmd("ZREM")
            .arg(kb.delayed())
            .arg(id)
            .query_async(&mut conn)
            .await?;
        if removed < 1 {
            return Err(Error::Refused(format!("job {id} is not delayed")));
        }
        redis::pipe()
            .cmd("HSET")
            .arg(&job_key)
            .arg("delay")
            .arg(delay_ms)
            .ignore()
            .cmd("ZADD")
            .arg(kb.delayed())
            .arg(score)
            .arg(id)
            .ignore()
            .query_async::<()>(&mut conn)
            .await?;
        self.add_delay_marker_if_needed(&kb).await?;
        Ok(())
    }

    /// Change a waiting or prioritized job's priority. Removes the job from its
    /// current list/set and re-inserts: priority 0 goes to wait/paused,
    /// otherwise into the prioritized ZSET.
    pub async fn change_priority(
        &self,
        queue: &str,
        id: &str,
        priority: i64,
        lifo: bool,
    ) -> Result<()> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let job_key = kb.job(id);
        let exists: bool = redis::cmd("EXISTS")
            .arg(&job_key)
            .query_async(&mut conn)
            .await?;
        if !exists {
            return Err(Error::JobNotFound {
                queue: queue.to_string(),
                id: id.to_string(),
            });
        }
        let (target, paused_or_maxed) = self.target_queue_list(&kb).await?;
        // Remove from the prioritized ZSET and from both wait/paused lists.
        redis::cmd("ZREM")
            .arg(kb.prioritized())
            .arg(id)
            .query_async::<()>(&mut conn)
            .await?;
        for list in [kb.wait(), kb.paused()] {
            redis::cmd("LREM")
                .arg(&list)
                .arg(0)
                .arg(id)
                .query_async::<()>(&mut conn)
                .await?;
        }
        if priority == 0 {
            let push = if lifo { "RPUSH" } else { "LPUSH" };
            redis::cmd(push)
                .arg(&target)
                .arg(id)
                .query_async::<()>(&mut conn)
                .await?;
            if !paused_or_maxed {
                redis::cmd("ZADD")
                    .arg(kb.marker())
                    .arg(0)
                    .arg("0")
                    .query_async::<()>(&mut conn)
                    .await?;
            }
        } else {
            self.add_job_with_priority(&kb, priority, id, paused_or_maxed)
                .await?;
        }
        redis::cmd("HSET")
            .arg(&job_key)
            .arg("priority")
            .arg(priority)
            .query_async::<()>(&mut conn)
            .await?;
        Ok(())
    }

    /// Trigger a job scheduler now: promote its already-produced next delayed
    /// job (`repeat:{id}:{nextMillis}`) to waiting. Reuses the verified promote
    /// path.
    pub async fn trigger_scheduler(&self, queue: &str, scheduler_id: &str) -> Result<()> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let millis: Option<String> = redis::cmd("ZSCORE")
            .arg(kb.repeat())
            .arg(scheduler_id)
            .query_async(&mut conn)
            .await?;
        let Some(millis) = millis else {
            return Err(Error::Refused(format!(
                "no scheduler {scheduler_id} in {queue}"
            )));
        };
        let millis = millis.split('.').next().unwrap_or(&millis);
        let delayed_job_id = format!("repeat:{scheduler_id}:{millis}");
        self.promote_job(queue, &delayed_job_id).await
    }

    // -- queue: global concurrency -----------------------------------------

    /// Set (or with `concurrency <= 0`, remove) the global concurrency.
    pub async fn set_global_concurrency(&self, queue: &str, concurrency: i64) -> Result<()> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        if concurrency <= 0 {
            redis::cmd("HDEL")
                .arg(kb.meta())
                .arg("concurrency")
                .query_async::<()>(&mut conn)
                .await?;
        } else {
            redis::cmd("HSET")
                .arg(kb.meta())
                .arg("concurrency")
                .arg(concurrency)
                .query_async::<()>(&mut conn)
                .await?;
        }
        Ok(())
    }

    // -- internal helpers ---------------------------------------------------

    /// Returns the target list key and whether the queue is paused or at max concurrency.
    async fn target_queue_list(&self, kb: &KeyBuilder) -> Result<(String, bool)> {
        let mut conn = self.conn();
        let attrs: Vec<Option<String>> = redis::cmd("HMGET")
            .arg(kb.meta())
            .arg("paused")
            .arg("concurrency")
            .query_async(&mut conn)
            .await?;
        let paused = attrs.first().and_then(|o| o.as_ref()).is_some();
        if paused {
            return Ok((kb.paused(), true));
        }
        if let Some(conc) = attrs
            .get(1)
            .and_then(|o| o.as_ref())
            .and_then(|s| s.parse::<i64>().ok())
        {
            let active: i64 = redis::cmd("LLEN")
                .arg(kb.active())
                .query_async(&mut conn)
                .await?;
            return Ok((kb.wait(), active >= conc));
        }
        Ok((kb.wait(), false))
    }

    async fn add_job_with_priority(
        &self,
        kb: &KeyBuilder,
        priority: i64,
        id: &str,
        paused_or_maxed: bool,
    ) -> Result<()> {
        let mut conn = self.conn();
        let counter: i64 = redis::cmd("INCR")
            .arg(kb.priority_counter())
            .query_async(&mut conn)
            .await?;
        let score = priority as f64 * PRIORITY_MULT + (counter as f64 % PRIORITY_MULT);
        redis::cmd("ZADD")
            .arg(kb.prioritized())
            .arg(score)
            .arg(id)
            .query_async::<()>(&mut conn)
            .await?;
        if !paused_or_maxed {
            redis::cmd("ZADD")
                .arg(kb.marker())
                .arg(0)
                .arg("0")
                .query_async::<()>(&mut conn)
                .await?;
        }
        Ok(())
    }

    async fn add_delayed(
        &self,
        kb: &KeyBuilder,
        id: &str,
        timestamp: i64,
        delay: i64,
    ) -> Result<()> {
        let mut conn = self.conn();
        let (score, delayed_ts) = self.delayed_score(kb, timestamp, delay).await?;
        redis::cmd("ZADD")
            .arg(kb.delayed())
            .arg(score)
            .arg(id)
            .query_async::<()>(&mut conn)
            .await?;
        redis::cmd("XADD")
            .arg(kb.events())
            .arg("MAXLEN")
            .arg("~")
            .arg(MAX_EVENTS)
            .arg("*")
            .arg("event")
            .arg("delayed")
            .arg("jobId")
            .arg(id)
            .arg("delay")
            .arg(delayed_ts)
            .query_async::<()>(&mut conn)
            .await?;
        self.add_delay_marker_if_needed(kb).await?;
        Ok(())
    }

    /// Bakes ordering into the delayed ZSET score. Returns `(score, ts)`.
    async fn delayed_score(
        &self,
        kb: &KeyBuilder,
        timestamp: i64,
        delay: i64,
    ) -> Result<(i64, i64)> {
        let mut conn = self.conn();
        let delayed_ts = if delay > 0 {
            timestamp + delay
        } else {
            timestamp
        };
        let min_score = delayed_ts * DELAY_MULT;
        let max_score = (delayed_ts + 1) * DELAY_MULT - 1;
        let res: Vec<RedisValue> = redis::cmd("ZREVRANGEBYSCORE")
            .arg(kb.delayed())
            .arg(max_score)
            .arg(min_score)
            .arg("WITHSCORES")
            .arg("LIMIT")
            .arg(0)
            .arg(1)
            .query_async(&mut conn)
            .await?;
        // res = [member, score] if any.
        let current_max = res
            .get(1)
            .and_then(|v| redis::from_redis_value::<String>(v).ok())
            .and_then(|s| s.split('.').next().unwrap_or(&s).parse::<i64>().ok());
        let score = match current_max {
            Some(cur) if cur >= max_score => max_score,
            Some(cur) => cur + 1,
            None => min_score,
        };
        Ok((score, delayed_ts))
    }

    /// Set the delay marker to the next delayed job's epoch ms, if any.
    async fn add_delay_marker_if_needed(&self, kb: &KeyBuilder) -> Result<()> {
        let mut conn = self.conn();
        let res: Vec<RedisValue> = redis::cmd("ZRANGE")
            .arg(kb.delayed())
            .arg(0)
            .arg(0)
            .arg("WITHSCORES")
            .query_async(&mut conn)
            .await?;
        if let Some(score) = res
            .get(1)
            .and_then(|v| redis::from_redis_value::<String>(v).ok())
        {
            if let Ok(score) = score.split('.').next().unwrap_or(&score).parse::<i64>() {
                let next_ts = score / DELAY_MULT;
                redis::cmd("ZADD")
                    .arg(kb.marker())
                    .arg(next_ts)
                    .arg("1")
                    .query_async::<()>(&mut conn)
                    .await?;
            }
        }
        Ok(())
    }

    async fn job_lifo(&self, kb: &KeyBuilder, id: &str) -> Result<bool> {
        let mut conn = self.conn();
        let opts: Option<String> = redis::cmd("HGET")
            .arg(kb.job(id))
            .arg("opts")
            .query_async(&mut conn)
            .await?;
        Ok(opts
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.get("lifo").and_then(Value::as_bool))
            .unwrap_or(false))
    }

    async fn emit_waiting(&self, kb: &KeyBuilder, id: &str, prev: Option<&str>) -> Result<()> {
        let mut conn = self.conn();
        let mut cmd = redis::cmd("XADD");
        cmd.arg(kb.events())
            .arg("MAXLEN")
            .arg("~")
            .arg(MAX_EVENTS)
            .arg("*")
            .arg("event")
            .arg("waiting")
            .arg("jobId")
            .arg(id);
        if let Some(prev) = prev {
            cmd.arg("prev").arg(prev);
        }
        cmd.query_async::<()>(&mut conn).await?;
        Ok(())
    }

    /// Delete all Redis keys associated with a job id.
    async fn delete_job_keys(&self, kb: &KeyBuilder, id: &str) -> Result<()> {
        let mut conn = self.conn();
        redis::cmd("DEL")
            .arg(kb.job(id))
            .arg(kb.job_logs(id))
            .arg(kb.job_lock(id))
            .arg(kb.key(&format!("{id}:dependencies")))
            .arg(kb.key(&format!("{id}:processed")))
            .arg(kb.key(&format!("{id}:failed")))
            .arg(kb.key(&format!("{id}:unsuccessful")))
            .query_async::<()>(&mut conn)
            .await?;
        Ok(())
    }

    /// Remove a job id from every state structure.
    async fn remove_from_states(&self, kb: &KeyBuilder, id: &str) -> Result<()> {
        let mut conn = self.conn();
        let mut pipe = redis::pipe();
        for state in JobState::ALL {
            if state.is_list() {
                pipe.cmd("LREM")
                    .arg(kb.state(state))
                    .arg(0)
                    .arg(id)
                    .ignore();
            } else {
                pipe.cmd("ZREM").arg(kb.state(state)).arg(id).ignore();
            }
        }
        pipe.query_async::<()>(&mut conn).await?;
        Ok(())
    }

    /// Detach a job from its parent's dependency structures.
    async fn detach_from_parent(&self, kb: &KeyBuilder, id: &str) -> Result<()> {
        let mut conn = self.conn();
        let job_key = kb.job(id);
        let parent_key: Option<String> = redis::cmd("HGET")
            .arg(&job_key)
            .arg("parentKey")
            .query_async(&mut conn)
            .await?;
        if let Some(pk) = parent_key {
            redis::pipe()
                .cmd("SREM")
                .arg(format!("{pk}:dependencies"))
                .arg(&job_key)
                .ignore()
                .cmd("ZREM")
                .arg(format!("{pk}:unsuccessful"))
                .arg(&job_key)
                .ignore()
                .cmd("HDEL")
                .arg(format!("{pk}:processed"))
                .arg(&job_key)
                .ignore()
                .cmd("HDEL")
                .arg(format!("{pk}:failed"))
                .arg(&job_key)
                .ignore()
                .query_async::<()>(&mut conn)
                .await?;
        }
        Ok(())
    }
}

/// Encode bull-board option names to BullMQ's abbreviated storage keys
/// (inverse of `optsDecodeMap`).
fn encode_opts(opts: &Map<String, Value>) -> String {
    let mut out = Map::new();
    for (k, v) in opts {
        match k.as_str() {
            "deduplication" => {
                out.insert("de".into(), v.clone());
            }
            "failParentOnFailure" => {
                out.insert("fpof".into(), v.clone());
            }
            "continueParentOnFailure" => {
                out.insert("cpof".into(), v.clone());
            }
            "ignoreDependencyOnFailure" => {
                out.insert("idof".into(), v.clone());
            }
            "keepLogs" => {
                out.insert("kl".into(), v.clone());
            }
            "removeDependencyOnFailure" => {
                out.insert("rdof".into(), v.clone());
            }
            "telemetry" => {
                if let Some(meta) = v.get("metadata") {
                    out.insert("tm".into(), meta.clone());
                }
                if let Some(omc) = v.get("omitContext") {
                    out.insert("omc".into(), omc.clone());
                }
            }
            _ => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    serde_json::to_string(&Value::Object(out)).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_option_abbreviations() {
        let opts = json!({
            "attempts": 3,
            "failParentOnFailure": true,
            "keepLogs": 10,
            "telemetry": { "metadata": "abc", "omitContext": true }
        });
        let encoded = encode_opts(opts.as_object().unwrap());
        let v: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(v["attempts"], json!(3));
        assert_eq!(v["fpof"], json!(true));
        assert_eq!(v["kl"], json!(10));
        assert_eq!(v["tm"], json!("abc"));
        assert_eq!(v["omc"], json!(true));
        assert!(v.get("failParentOnFailure").is_none());
    }
}
