//! A direct-to-Redis client for [BullMQ](https://docs.bullmq.io/) queues.
//!
//! Implements the read and admin-write operations a dashboard needs: counts,
//! job listings, logs, metrics, flows, pause/resume, retry, promote, clean,
//! remove, and add. Semantics match BullMQ v5.

mod client;
mod error;
mod events;
pub mod keys;
mod types;
pub mod write;

pub use client::{BullClient, ConnectOptions, DEFAULT_PREFIX};
pub use error::{Error, Result};
pub use events::EventReader;
pub use keys::KeyBuilder;
pub use types::{
    value_to_plain_string, ActiveJobLock, DelayedKind, EventKind, FlowNode, Job, JobCounts,
    JobLogs, JobScheduler, JobState, Metrics, MetricsKind, QueueEvent, QueueSummary,
    RateLimitStatus, RedisInfo, WorkerInfo,
};
