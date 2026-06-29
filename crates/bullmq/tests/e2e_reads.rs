//! End-to-end read tests: assert the Rust `BullClient` reproduces bullmq's own
//! view of authentic seeded data (counts, listings, jobs, logs, flows).

mod common;

use bullmq::{BullClient, JobState, MetricsKind, DEFAULT_PREFIX};
use common::{run_seeder, start_redis};

fn counts_match(actual: &bullmq::JobCounts, expected: &common::Counts, queue: &str) {
    assert_eq!(actual.active, expected.active, "{queue}.active");
    assert_eq!(actual.waiting, expected.waiting, "{queue}.waiting");
    assert_eq!(
        actual.waiting_children, expected.waiting_children,
        "{queue}.waiting_children"
    );
    assert_eq!(
        actual.prioritized, expected.prioritized,
        "{queue}.prioritized"
    );
    assert_eq!(actual.completed, expected.completed, "{queue}.completed");
    assert_eq!(actual.failed, expected.failed, "{queue}.failed");
    assert_eq!(actual.delayed, expected.delayed, "{queue}.delayed");
    assert_eq!(actual.paused, expected.paused, "{queue}.paused");
}

#[tokio::test]
async fn reads_match_bullmq() {
    let redis = start_redis().await;
    let manifest = run_seeder(&redis.url).await;
    let client = BullClient::connect(&redis.url, DEFAULT_PREFIX)
        .await
        .expect("connect");

    // -- discovery ----------------------------------------------------------
    let discovered = client.discover_queues().await.expect("discover");
    for q in &manifest.queues {
        assert!(
            discovered.contains(&q.name),
            "discovered queues {discovered:?} missing {}",
            q.name
        );
    }
    assert!(
        discovered.contains(&"workers".to_string()),
        "child queue discoverable"
    );

    // -- per-queue counts / paused / concurrency ----------------------------
    for q in &manifest.queues {
        let counts = client.job_counts(&q.name).await.expect("counts");
        counts_match(&counts, &q.counts, &q.name);

        let paused = client.is_paused(&q.name).await.expect("is_paused");
        assert_eq!(paused, q.is_paused, "{}.isPaused", q.name);

        let gc = client.global_concurrency(&q.name).await.expect("gc");
        assert_eq!(gc, q.global_concurrency, "{}.globalConcurrency", q.name);
    }

    // -- job id listings per state match bullmq's getJobs order -------------
    for q in &manifest.queues {
        for state in JobState::ALL {
            let expected = q
                .jobs_by_state
                .get(state.status_str())
                .cloned()
                .unwrap_or_default();
            // bull-board lists by *status* (waiting → wait + paused).
            let ids = client
                .list_status_ids(&q.name, state, 0, 50)
                .await
                .expect("list_status_ids");
            assert_eq!(
                ids,
                expected,
                "{}.{} id listing",
                q.name,
                state.status_str()
            );
        }
    }

    // -- detailed job fields, logs (emails completed + failed samples) ------
    let emails = manifest.queue("emails");
    for sample in &emails.sample_jobs {
        let id = sample.job["id"].as_str().unwrap();
        let job = client.require_job("emails", id).await.expect("get job");

        assert_eq!(job.name, sample.job["name"].as_str().unwrap(), "name");
        assert_eq!(
            job.attempts_made,
            sample.job["attemptsMade"].as_i64().unwrap(),
            "attemptsMade"
        );
        assert_eq!(job.timestamp, sample.job["timestamp"].as_i64(), "timestamp");
        assert_eq!(
            job.processed_on,
            sample.job["processedOn"].as_i64(),
            "processedOn"
        );
        assert_eq!(
            job.finished_on,
            sample.job["finishedOn"].as_i64(),
            "finishedOn"
        );

        // data round-trips through JSON
        let job_data: serde_json::Value = serde_json::from_str(&job.data).unwrap();
        assert_eq!(job_data, sample.job["data"], "data for {id}");

        if sample.state == "failed" {
            assert!(job.is_failed(), "{id} should be failed");
            assert_eq!(
                job.failed_reason.as_deref(),
                sample.job["failedReason"].as_str(),
                "failedReason"
            );
            assert!(!job.stacktrace.is_empty(), "{id} has stacktrace");
        } else {
            assert!(!job.is_failed(), "{id} should not be failed");
            // return value matches
            assert_eq!(
                job.return_value.as_ref(),
                Some(&sample.job["returnvalue"]),
                "returnvalue"
            );
        }

        // logs match exactly (oldest-first)
        let logs = client.job_logs("emails", id, 0, -1).await.expect("logs");
        assert_eq!(logs.logs, sample.logs, "logs for {id}");
        assert_eq!(logs.count, sample.logs.len() as i64, "log count for {id}");
    }

    // -- metrics parity (both sides agree) ----------------------------------
    if let Some(m) = &emails.metrics {
        let metrics = client
            .metrics("emails", MetricsKind::Completed, 0, -1)
            .await
            .expect("metrics");
        assert_eq!(metrics.count, m.meta_count, "metrics.count");
        assert_eq!(metrics.prev_ts, m.prev_ts, "metrics.prevTS");
        assert_eq!(metrics.prev_count, m.prev_count, "metrics.prevCount");
        assert_eq!(metrics.data.len(), m.data_len, "metrics.data len");
    }

    // -- job state lookup ---------------------------------------------------
    let media_active = &manifest.queue("media").jobs_by_state["active"][0];
    assert_eq!(
        client.job_state("media", media_active).await.unwrap(),
        Some(JobState::Active),
        "media active job state"
    );

    // -- redis info ---------------------------------------------------------
    let info = client.redis_info().await.expect("info");
    assert!(info.version().is_some(), "redis version present");
    assert!(
        info.connected_clients().unwrap_or(0) >= 1,
        "connected clients"
    );

    // -- flow tree ----------------------------------------------------------
    let flow = &manifest.flow;
    // root discoverable by walking up from a child
    let root = client
        .find_flow_root(&flow.child_queue, &flow.child_ids[0])
        .await
        .expect("find root")
        .expect("has root");
    assert_eq!(root.0, flow.root_queue, "flow root queue");
    assert_eq!(root.1, flow.root_id, "flow root id");

    let tree = client
        .get_flow_tree(&flow.root_queue, &flow.root_id, 5)
        .await
        .expect("flow tree")
        .expect("tree exists");
    assert_eq!(tree.job.id, flow.root_id);
    let mut child_ids: Vec<String> = tree.children.iter().map(|c| c.job.id.clone()).collect();
    child_ids.sort();
    let mut expected_children = flow.child_ids.clone();
    expected_children.sort();
    assert_eq!(child_ids, expected_children, "flow children");
    for c in &tree.children {
        assert_eq!(c.queue_name, flow.child_queue, "child queue name");
    }
}
