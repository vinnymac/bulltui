//! A direct-to-Redis client for [BullMQ](https://docs.bullmq.io/) queues.
//!
//! `bulltui` talks to Redis/Valkey directly rather than through the Node.js
//! `bullmq` library. This crate implements the parts of BullMQ's data model and
//! operations that a dashboard needs — reads (counts, job listings, logs,
//! metrics, flows) and admin writes (pause/resume, retry, promote, clean,
//! remove, add, …) — matching bullmq v5 semantics as closely as possible.
//!
//! The correctness reference is the bullmq source itself: the Lua command
//! scripts (`dist/cjs/commands/*.lua`) and the `Queue`/`Job` classes.

mod client;
mod error;
mod events;
pub mod keys;
mod types;
pub mod write;

pub use client::{BullClient, DEFAULT_PREFIX};
pub use error::{Error, Result};
pub use events::EventReader;
pub use keys::KeyBuilder;
pub use types::{
    value_to_plain_string, ActiveJobLock, DelayedKind, EventKind, FlowNode, Job, JobCounts,
    JobLogs, JobScheduler, JobState, Metrics, MetricsKind, QueueEvent, QueueSummary,
    RateLimitStatus, RedisInfo, WorkerInfo,
};
