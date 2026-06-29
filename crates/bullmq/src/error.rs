//! Error types for the bullmq client.

use thiserror::Error;

/// Result alias for bullmq operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors raised by the bullmq client.
#[derive(Debug, Error)]
pub enum Error {
    /// A Redis command failed.
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),

    /// A requested job was not found.
    #[error("job {id} not found in queue {queue}")]
    JobNotFound { queue: String, id: String },

    /// An operation could not be completed because of queue state
    /// (e.g. obliterating a queue that still has active jobs).
    #[error("operation refused: {0}")]
    Refused(String),

    /// An invalid argument was supplied by the caller.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}
