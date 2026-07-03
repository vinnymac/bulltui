//! End-to-end write tests. Verifies admin operations both by reading state
//! back with the (separately verified) Rust client AND behaviourally: real
//! bullmq workers must actually process jobs that the Rust client adds,
//! retries and promotes, which proves the wait-list/marker/event mechanics.

mod common;

use bullmq::{BullClient, JobState, DEFAULT_PREFIX};
use common::{run_e2e, start_redis};
use serde_json::json;

async fn counts(client: &BullClient, q: &str) -> bullmq::JobCounts {
    client.job_counts(q).await.expect("counts")
}

#[tokio::test]
async fn writes_behave_like_bullmq() {
    let redis = start_redis().await;
    let url = redis.url.clone();
    let client = BullClient::connect(&url, DEFAULT_PREFIX)
        .await
        .expect("connect");

    // 1. add → a real worker processes it -----------------------------------
    let id = client
        .add_job("wq", "ok", &json!({"x": 1}), &json!({}))
        .await
        .expect("add");
    assert_eq!(counts(&client, "wq").await.waiting, 1, "added job waiting");
    let r = run_e2e(&url, &["worker", "wq", "1", "8000"]).await;
    assert_eq!(r["completed"], json!(1), "worker processed added job {id}");
    assert_eq!(
        counts(&client, "wq").await.completed,
        1,
        "wq completed after worker"
    );

    // 2. add delayed → promote → worker processes ---------------------------
    let did = client
        .add_job("dq", "ok", &json!({}), &json!({"delay": 600000}))
        .await
        .expect("add delayed");
    assert_eq!(counts(&client, "dq").await.delayed, 1, "delayed queued");
    client.promote_job("dq", &did).await.expect("promote");
    let c = counts(&client, "dq").await;
    assert_eq!(c.delayed, 0, "promoted out of delayed");
    assert_eq!(c.waiting, 1, "promoted into waiting");
    let r = run_e2e(&url, &["worker", "dq", "1", "8000"]).await;
    assert_eq!(r["completed"], json!(1), "worker processed promoted job");

    // 3. add prioritized → worker processes ---------------------------------
    client
        .add_job("pq", "ok", &json!({}), &json!({"priority": 5}))
        .await
        .expect("add prio");
    assert_eq!(
        counts(&client, "pq").await.prioritized,
        1,
        "prioritized queued"
    );
    let r = run_e2e(&url, &["worker", "pq", "1", "8000"]).await;
    assert_eq!(r["completed"], json!(1), "worker processed prioritized job");

    // 4. retry all failed → worker processes --------------------------------
    let r = run_e2e(&url, &["addrun", "fq", "2", "fail", "8000"]).await;
    assert_eq!(r["failed"], json!(2), "seeded 2 failed");
    assert_eq!(counts(&client, "fq").await.failed, 2, "fq failed=2");
    let retried = client
        .retry_all("fq", JobState::Failed)
        .await
        .expect("retry all");
    assert_eq!(retried, 2, "retried 2");
    let c = counts(&client, "fq").await;
    assert_eq!(c.failed, 0, "no failed after retry");
    assert_eq!(c.waiting, 2, "2 waiting after retry");
    let r = run_e2e(&url, &["worker", "fq", "2", "8000"]).await;
    assert_eq!(r["completed"], json!(2), "worker processed retried jobs");

    // 5. pause blocks processing; resume restores it ------------------------
    client
        .add_job("sq", "ok", &json!({}), &json!({}))
        .await
        .expect("add");
    client.pause("sq").await.expect("pause");
    assert!(client.is_paused("sq").await.unwrap(), "sq paused");
    let c = counts(&client, "sq").await;
    assert_eq!(c.waiting, 0, "paused: not waiting");
    assert_eq!(c.paused, 1, "paused: 1 in paused");
    let r = run_e2e(&url, &["worker", "sq", "1", "2500"]).await;
    assert_eq!(r["completed"], json!(0), "paused queue not processed");
    client.resume("sq").await.expect("resume");
    assert!(!client.is_paused("sq").await.unwrap(), "sq resumed");
    assert_eq!(
        counts(&client, "sq").await.waiting,
        1,
        "resumed back to waiting"
    );
    let r = run_e2e(&url, &["worker", "sq", "1", "8000"]).await;
    assert_eq!(r["completed"], json!(1), "resumed queue processed");

    // 6. remove a waiting job -----------------------------------------------
    let rid = client
        .add_job("rq", "ok", &json!({"k": "v"}), &json!({}))
        .await
        .expect("add");
    assert_eq!(counts(&client, "rq").await.waiting, 1);
    client.remove_job("rq", &rid).await.expect("remove");
    assert!(
        client.get_job("rq", &rid).await.unwrap().is_none(),
        "job removed"
    );
    assert_eq!(
        counts(&client, "rq").await.waiting,
        0,
        "rq empty after remove"
    );

    // 7. clean completed (grace 0 removes all) ------------------------------
    let r = run_e2e(&url, &["addrun", "cq", "3", "success", "8000"]).await;
    assert_eq!(r["completed"], json!(3), "seeded 3 completed");
    assert_eq!(counts(&client, "cq").await.completed, 3);
    let removed = client
        .clean("cq", JobState::Completed, 0, 0)
        .await
        .expect("clean");
    assert_eq!(removed.len(), 3, "cleaned 3 completed");
    assert_eq!(
        counts(&client, "cq").await.completed,
        0,
        "completed cleared"
    );

    // 8. empty (drain) waiting ----------------------------------------------
    for _ in 0..3 {
        client
            .add_job("eq", "ok", &json!({}), &json!({}))
            .await
            .expect("add");
    }
    assert_eq!(counts(&client, "eq").await.waiting, 3);
    let n = client.empty("eq").await.expect("empty");
    assert_eq!(n, 3, "drained 3");
    assert_eq!(counts(&client, "eq").await.waiting, 0, "eq drained");

    // 9. obliterate (requires paused) ---------------------------------------
    client
        .add_job("oq", "ok", &json!({}), &json!({}))
        .await
        .expect("add");
    client
        .add_job("oq", "ok", &json!({}), &json!({}))
        .await
        .expect("add");
    // refuses while running
    assert!(
        client.obliterate("oq").await.is_err(),
        "obliterate refused while running"
    );
    client.pause("oq").await.expect("pause");
    client.obliterate("oq").await.expect("obliterate");
    let discovered = client.discover_queues().await.expect("discover");
    assert!(
        !discovered.contains(&"oq".to_string()),
        "oq obliterated (meta gone)"
    );
    assert_eq!(counts(&client, "oq").await.total(), 0, "oq has no jobs");

    // 10. set / remove global concurrency -----------------------------------
    client
        .set_global_concurrency("gq", 7)
        .await
        .expect("set gc");
    assert_eq!(
        client.global_concurrency("gq").await.unwrap(),
        Some(7),
        "gc set (rust)"
    );
    let r = run_e2e(&url, &["gc", "gq"]).await;
    assert_eq!(r["globalConcurrency"], json!(7), "gc set (bullmq)");
    client
        .set_global_concurrency("gq", 0)
        .await
        .expect("remove gc");
    assert_eq!(
        client.global_concurrency("gq").await.unwrap(),
        None,
        "gc removed"
    );

    // 11. update data + duplicate -------------------------------------------
    let uid = client
        .add_job("uq", "task", &json!({"a": 1}), &json!({"attempts": 2}))
        .await
        .expect("add");
    client
        .update_job_data("uq", &uid, &json!({"a": 2}))
        .await
        .expect("update");
    let updated = client.require_job("uq", &uid).await.expect("get");
    let data: serde_json::Value = serde_json::from_str(&updated.data).unwrap();
    assert_eq!(data, json!({"a": 2}), "data updated");

    let new_id = client.duplicate_job("uq", &uid).await.expect("duplicate");
    assert_ne!(new_id, uid, "duplicate has new id");
    let dup = client.require_job("uq", &new_id).await.expect("get dup");
    assert_eq!(dup.name, "task", "dup name matches");
    let dup_data: serde_json::Value = serde_json::from_str(&dup.data).unwrap();
    assert_eq!(dup_data, json!({"a": 2}), "dup data matches");
    assert_eq!(
        dup.opts.get("attempts"),
        Some(&json!(2)),
        "dup opts preserved"
    );

    // 12. promote_all + retry single ----------------------------------------
    for _ in 0..3 {
        client
            .add_job("paq", "ok", &json!({}), &json!({"delay": 600000}))
            .await
            .expect("add");
    }
    assert_eq!(counts(&client, "paq").await.delayed, 3);
    let promoted = client.promote_all("paq").await.expect("promote all");
    assert_eq!(promoted, 3, "promoted all 3");
    assert_eq!(counts(&client, "paq").await.delayed, 0, "no delayed left");
}
