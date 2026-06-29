//! Live event-stream reading: backfill (`XREVRANGE`) and a dedicated blocking
//! `XREAD` reader over one or more queue event streams.

use std::collections::HashMap;

use redis::aio::MultiplexedConnection;
use redis::streams::{StreamId, StreamRangeReply, StreamReadOptions, StreamReadReply};
use redis::AsyncCommands;

use crate::client::BullClient;
use crate::error::Result;
use crate::types::{EventKind, QueueEvent};

fn event_from_stream_id(queue: &str, id: &StreamId) -> QueueEvent {
    let mut fields = HashMap::new();
    for (k, v) in &id.map {
        if let Ok(s) = redis::from_redis_value::<String>(v) {
            fields.insert(k.clone(), s);
        }
    }
    let kind = fields
        .get("event")
        .map(|e| EventKind::from_event_str(e))
        .unwrap_or(EventKind::Other);
    let job_id = fields.get("jobId").cloned();
    let ts = id
        .id
        .split('-')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    QueueEvent {
        stream_id: id.id.clone(),
        ts,
        queue: queue.to_string(),
        kind,
        job_id,
        fields,
    }
}

impl BullClient {
    /// Backfill the last `count` events for one queue, oldest-first. Non-blocking
    /// (`XREVRANGE`), safe on the shared connection.
    pub async fn backfill_events(&self, queue: &str, count: usize) -> Result<Vec<QueueEvent>> {
        let kb = self.keys(queue);
        let mut conn = self.conn();
        let reply: StreamRangeReply = redis::cmd("XREVRANGE")
            .arg(kb.events())
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(count)
            .query_async(&mut conn)
            .await?;
        let mut events: Vec<QueueEvent> = reply
            .ids
            .iter()
            .map(|id| event_from_stream_id(queue, id))
            .collect();
        events.reverse(); // XREVRANGE is newest-first; we want oldest-first
        Ok(events)
    }

    /// Open a dedicated blocking reader over the given queues' event streams,
    /// resuming after `start_ids[i]` for `queues[i]` (use `"$"` for "only new").
    /// It owns its own multiplexed connection so a blocking `XREAD` never stalls
    /// the shared command connection.
    pub async fn open_event_reader(
        &self,
        queues: &[String],
        start_ids: &[String],
    ) -> Result<EventReader> {
        let conn = self
            .redis_client()
            .get_multiplexed_async_connection()
            .await?;
        let keys: Vec<String> = queues.iter().map(|q| self.keys(q).events()).collect();
        let last_ids = if start_ids.len() == keys.len() {
            start_ids.to_vec()
        } else {
            vec!["$".to_string(); keys.len()]
        };
        Ok(EventReader {
            conn,
            keys,
            queues: queues.to_vec(),
            last_ids,
        })
    }
}

/// A blocking reader over one or more queue event streams.
pub struct EventReader {
    conn: MultiplexedConnection,
    keys: Vec<String>,
    queues: Vec<String>,
    last_ids: Vec<String>,
}

impl EventReader {
    /// The current per-stream cursor, for resuming after a reconnect.
    pub fn cursor(&self) -> Vec<String> {
        self.last_ids.clone()
    }

    /// One `XREAD BLOCK <block_ms> COUNT <count> STREAMS …`. Returns parsed
    /// events (possibly empty on a block timeout) and advances each stream cursor.
    pub async fn next_batch(&mut self, block_ms: usize, count: usize) -> Result<Vec<QueueEvent>> {
        let opts = StreamReadOptions::default().block(block_ms).count(count);
        let reply: StreamReadReply = self
            .conn
            .xread_options(&self.keys, &self.last_ids, &opts)
            .await?;
        let mut out = Vec::new();
        for key in &reply.keys {
            let qi = self.keys.iter().position(|k| k == &key.key);
            let queue = qi
                .map(|i| self.queues[i].clone())
                .unwrap_or_else(|| key.key.clone());
            for id in &key.ids {
                out.push(event_from_stream_id(&queue, id));
                if let Some(i) = qi {
                    self.last_ids[i] = id.id.clone();
                }
            }
        }
        Ok(out)
    }
}
