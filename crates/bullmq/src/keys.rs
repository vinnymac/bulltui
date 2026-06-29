//! Redis key construction for BullMQ queues.
//!
//! BullMQ namespaces every key as `{prefix}:{queueName}:{suffix}`. The default
//! prefix is `bull`. This mirrors `Queue.toKey` / the `keys` map in the bullmq
//! source (see `dist/cjs/classes/queue-keys.js`).

use crate::types::JobState;

/// Builds the fully-qualified Redis keys for a single queue.
#[derive(Debug, Clone)]
pub struct KeyBuilder {
    prefix: String,
    name: String,
}

impl KeyBuilder {
    /// Create a key builder for `name` under `prefix` (e.g. `bull`).
    pub fn new(prefix: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            name: name.into(),
        }
    }

    /// The queue name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The prefix (without trailing colon).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// `{prefix}:{name}:` — the base used by `getCounts`/`getRanges` (KEYS[1]).
    pub fn base(&self) -> String {
        format!("{}:{}:", self.prefix, self.name)
    }

    /// `{prefix}:{name}:{suffix}`.
    pub fn key(&self, suffix: &str) -> String {
        format!("{}:{}:{}", self.prefix, self.name, suffix)
    }

    /// The job hash key `{prefix}:{name}:{id}`.
    pub fn job(&self, id: &str) -> String {
        self.key(id)
    }

    /// The job logs list key `{prefix}:{name}:{id}:logs`.
    pub fn job_logs(&self, id: &str) -> String {
        self.key(&format!("{id}:logs"))
    }

    /// The job lock key `{prefix}:{name}:{id}:lock`.
    pub fn job_lock(&self, id: &str) -> String {
        self.key(&format!("{id}:lock"))
    }

    /// The dependencies set key for a parent job `{parentKey}:dependencies`.
    pub fn job_dependencies(&self, id: &str) -> String {
        self.key(&format!("{id}:dependencies"))
    }

    /// The Redis key that backs a given job state.
    pub fn state(&self, state: JobState) -> String {
        self.key(state.redis_suffix())
    }

    pub fn meta(&self) -> String {
        self.key("meta")
    }
    pub fn id_counter(&self) -> String {
        self.key("id")
    }
    pub fn events(&self) -> String {
        self.key("events")
    }
    pub fn marker(&self) -> String {
        self.key("marker")
    }
    pub fn priority_counter(&self) -> String {
        self.key("pc")
    }
    pub fn wait(&self) -> String {
        self.key("wait")
    }
    pub fn paused(&self) -> String {
        self.key("paused")
    }
    pub fn active(&self) -> String {
        self.key("active")
    }
    pub fn delayed(&self) -> String {
        self.key("delayed")
    }
    pub fn completed(&self) -> String {
        self.key("completed")
    }
    pub fn failed(&self) -> String {
        self.key("failed")
    }
    pub fn prioritized(&self) -> String {
        self.key("prioritized")
    }
    pub fn waiting_children(&self) -> String {
        self.key("waiting-children")
    }
    pub fn metrics(&self, kind: &str) -> String {
        self.key(&format!("metrics:{kind}"))
    }
    pub fn metrics_data(&self, kind: &str) -> String {
        self.key(&format!("metrics:{kind}:data"))
    }
    /// The stalled-jobs SET `{prefix}:{name}:stalled`.
    pub fn stalled(&self) -> String {
        self.key("stalled")
    }
    /// The stalled-check throttle STRING `{prefix}:{name}:stalled-check`.
    pub fn stalled_check(&self) -> String {
        self.key("stalled-check")
    }
    /// The rate-limiter counter STRING `{prefix}:{name}:limiter`.
    pub fn limiter(&self) -> String {
        self.key("limiter")
    }
    /// The job-schedulers index ZSET (member = schedulerId, score = next-run ms).
    pub fn repeat(&self) -> String {
        self.key("repeat")
    }
    /// A single job scheduler's metadata hash `{prefix}:{name}:repeat:{id}`.
    pub fn repeat_scheduler(&self, id: &str) -> String {
        self.key(&format!("repeat:{id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_expected_keys() {
        let k = KeyBuilder::new("bull", "emails");
        assert_eq!(k.base(), "bull:emails:");
        assert_eq!(k.wait(), "bull:emails:wait");
        assert_eq!(k.job("42"), "bull:emails:42");
        assert_eq!(k.job_logs("42"), "bull:emails:42:logs");
        assert_eq!(
            k.state(JobState::WaitingChildren),
            "bull:emails:waiting-children"
        );
        assert_eq!(
            k.metrics_data("completed"),
            "bull:emails:metrics:completed:data"
        );
        assert_eq!(k.stalled(), "bull:emails:stalled");
        assert_eq!(k.stalled_check(), "bull:emails:stalled-check");
        assert_eq!(k.limiter(), "bull:emails:limiter");
        assert_eq!(k.repeat(), "bull:emails:repeat");
        assert_eq!(k.repeat_scheduler("digest"), "bull:emails:repeat:digest");
    }

    #[test]
    fn handles_delimited_queue_names() {
        let k = KeyBuilder::new("bull", "billing:invoices");
        assert_eq!(k.meta(), "bull:billing:invoices:meta");
    }
}
