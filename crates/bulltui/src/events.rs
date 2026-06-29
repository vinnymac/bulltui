//! Background orchestration for the live events feed.
//!
//! A tokio task owns a *dedicated* Redis connection, tails the queue event
//! streams with a blocking `XREAD`, and forwards batches over an `mpsc` channel
//! to the run loop. It is lazy-spawned when the Events screen opens and torn
//! down on exit, so it costs nothing when unused. See ADR-0001.

use std::time::Duration;

use bullmq::{BullClient, QueueEvent};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const BLOCK_MS: usize = 2000;
const BATCH: usize = 256;
const BACKOFF_START_MS: u64 = 250;
const BACKOFF_MAX_MS: u64 = 5000;

/// A running event-stream task. Call [`shutdown`](EventStreamHandle::shutdown)
/// to stop it.
pub struct EventStreamHandle {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl EventStreamHandle {
    /// Spawn a tail over `queues`, sending the backfill first then live batches
    /// to `tx`. Reconnects with backoff; exits when `shutdown` fires or `tx` is
    /// dropped.
    pub fn spawn(
        client: BullClient,
        queues: Vec<String>,
        backfill: usize,
        tx: mpsc::Sender<Vec<QueueEvent>>,
    ) -> Self {
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            // Backfill per queue, tracking each stream's last id so the live
            // reader resumes exactly after it (no gap, no `"$"` race).
            let mut start_ids = vec!["$".to_string(); queues.len()];
            let mut merged = Vec::new();
            for (i, q) in queues.iter().enumerate() {
                if let Ok(evs) = client.backfill_events(q, backfill).await {
                    if let Some(last) = evs.last() {
                        start_ids[i] = last.stream_id.clone();
                    }
                    merged.extend(evs);
                }
            }
            merged.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.stream_id.cmp(&b.stream_id)));
            if !merged.is_empty() && tx.send(merged).await.is_err() {
                return;
            }

            let mut backoff = BACKOFF_START_MS;
            loop {
                if *shutdown_rx.borrow() {
                    return;
                }
                let mut reader = match client.open_event_reader(&queues, &start_ids).await {
                    Ok(r) => r,
                    Err(_) => {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(backoff)) => {}
                            _ = shutdown_rx.changed() => return,
                        }
                        backoff = (backoff * 2).min(BACKOFF_MAX_MS);
                        continue;
                    }
                };
                backoff = BACKOFF_START_MS;
                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => return,
                        batch = reader.next_batch(BLOCK_MS, BATCH) => {
                            match batch {
                                Ok(evs) => {
                                    if !evs.is_empty() && tx.send(evs).await.is_err() {
                                        return;
                                    }
                                }
                                Err(_) => {
                                    // Resume after the last seen id on reconnect.
                                    start_ids = reader.cursor();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        });
        EventStreamHandle { shutdown, task }
    }

    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        self.task.abort();
    }
}
