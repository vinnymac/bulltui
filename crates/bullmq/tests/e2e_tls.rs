//! End-to-end TLS tests. Proves `rediss://` actually rides TLS at runtime,
//! not just that the URL parses, by running real BullMQ commands over a
//! TLS-only broker; verifies certificate verification is on by default.

mod common;

use std::time::Duration;

use bullmq::{BullClient, ConnectOptions, DEFAULT_PREFIX};
use common::start_redis_tls;
use serde_json::json;
use tokio::time::timeout;

/// With verification off, a `rediss://` connection completes the TLS handshake
/// and carries real reads *and* writes; this is the core TLS correctness check.
#[tokio::test]
async fn rediss_carries_commands_over_tls() {
    let redis = start_redis_tls().await;

    let client = BullClient::connect_with(
        &redis.url,
        DEFAULT_PREFIX,
        ConnectOptions { insecure: true },
    )
    .await
    .expect("connect over rediss:// (insecure)");

    // A read proves the TLS channel round-trips commands…
    let counts = client.job_counts("tq").await.expect("counts over TLS");
    assert_eq!(counts.waiting, 0, "fresh queue starts empty");

    // …and a write proves it in the other direction too.
    client
        .add_job("tq", "ok", &json!({"x": 1}), &json!({}))
        .await
        .expect("add job over TLS");
    assert_eq!(
        client.job_counts("tq").await.expect("counts").waiting,
        1,
        "job added over TLS is waiting",
    );
}

/// The secure default verifies the server certificate: connecting to the
/// self-signed broker *without* `insecure` must fail (against the compiled-in
/// public CA roots), rather than silently trusting it.
#[tokio::test]
async fn rediss_verifies_certificate_by_default() {
    let redis = start_redis_tls().await;

    let outcome = timeout(Duration::from_secs(20), async {
        let client = BullClient::connect(&redis.url, DEFAULT_PREFIX).await?;
        // If the handshake is lazy, a command forces it.
        client.job_counts("tq").await?;
        Ok::<(), bullmq::Error>(())
    })
    .await
    .expect("verification should fail fast, not hang");

    assert!(
        outcome.is_err(),
        "verified rediss:// must reject the self-signed cert, got {outcome:?}",
    );
}
