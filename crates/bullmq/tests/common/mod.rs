//! Shared e2e harness: a Testcontainers Redis + the Node bullmq seeder.
//!
//! The seeder creates authentic BullMQ data and prints a manifest describing
//! bullmq's own view of it. Tests assert the Rust client reproduces that view.

#![allow(dead_code)]

use std::collections::HashMap;

use serde::Deserialize;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// A disposable self-signed cert + private key for the TLS e2e broker.
///
/// **Not a secret.** It exists only to give the throwaway test container a TLS
/// identity: it is served on an ephemeral `localhost` container, trusted by no
/// real client (the test connects with verification off), and can be freely
/// regenerated. CN `bulltui-test-redis`, valid to 2126.
pub const TEST_TLS_CERT_PEM: &str = include_str!("redis-test.crt");
pub const TEST_TLS_KEY_PEM: &str = include_str!("redis-test.key");

/// A running Redis container plus its connection URL.
pub struct RedisFixture {
    pub container: ContainerAsync<GenericImage>,
    pub url: String,
}

/// Start a Redis container (uses the locally cached `redis:7-alpine`).
pub async fn start_redis() -> RedisFixture {
    // Ryuk's reaper image isn't cached and can't be pulled in this environment.
    std::env::set_var("TESTCONTAINERS_RYUK_DISABLED", "true");

    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .expect("start redis container");

    let host = container.get_host().await.expect("container host");
    let port = container
        .get_host_port_ipv4(6379.tcp())
        .await
        .expect("container port");
    let url = format!("redis://{host}:{port}");
    RedisFixture { container, url }
}

/// Start a **TLS-only** Redis container: the plaintext port is disabled
/// (`--port 0`) and the server listens only on a TLS port serving
/// [`TEST_TLS_CERT_PEM`]. Returns a `rediss://` URL. Client cert auth is off
/// (`--tls-auth-clients no`), so the CA file is only there to satisfy config.
pub async fn start_redis_tls() -> RedisFixture {
    // Ryuk's reaper image isn't cached and can't be pulled in this environment.
    std::env::set_var("TESTCONTAINERS_RYUK_DISABLED", "true");

    let container = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .with_copy_to("/tls/redis.crt", TEST_TLS_CERT_PEM.as_bytes().to_vec())
        .with_copy_to("/tls/redis.key", TEST_TLS_KEY_PEM.as_bytes().to_vec())
        // Leading `--` arg → the image entrypoint prepends `redis-server` and
        // drops to the `redis` user; the copied files are world-readable (0644).
        .with_cmd([
            "--tls-port",
            "6379",
            "--port",
            "0",
            "--tls-cert-file",
            "/tls/redis.crt",
            "--tls-key-file",
            "/tls/redis.key",
            "--tls-ca-cert-file",
            "/tls/redis.crt",
            "--tls-auth-clients",
            "no",
        ])
        .start()
        .await
        .expect("start redis TLS container");

    let host = container.get_host().await.expect("container host");
    let port = container
        .get_host_port_ipv4(6379.tcp())
        .await
        .expect("container port");
    let url = format!("rediss://{host}:{port}");
    RedisFixture { container, url }
}

/// Run the Node seeder against `url` and parse the manifest it prints.
pub async fn run_seeder(url: &str) -> Manifest {
    let seeder = format!("{}/../../e2e/seeder/seed.mjs", env!("CARGO_MANIFEST_DIR"));
    let output = tokio::process::Command::new("node")
        .arg(&seeder)
        .arg(url)
        .output()
        .await
        .expect("spawn node seeder");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        panic!(
            "seeder failed ({}):\nSTDERR:\n{stderr}\nSTDOUT:\n{stdout}",
            output.status
        );
    }
    let start = stdout
        .find("<<<MANIFEST>>>")
        .unwrap_or_else(|| panic!("no manifest marker in seeder output:\n{stdout}\n{stderr}"))
        + "<<<MANIFEST>>>".len();
    let end = stdout.find("<<<END>>>").expect("manifest end marker");
    serde_json::from_str(&stdout[start..end]).expect("parse manifest json")
}

/// Run an `e2e.mjs` subcommand (worker/addrun/gc) and parse its JSON result.
pub async fn run_e2e(url: &str, args: &[&str]) -> serde_json::Value {
    let script = format!("{}/../../e2e/seeder/e2e.mjs", env!("CARGO_MANIFEST_DIR"));
    let mut cmd = tokio::process::Command::new("node");
    cmd.arg(&script).arg(args[0]).arg(url);
    for a in &args[1..] {
        cmd.arg(a);
    }
    let output = cmd.output().await.expect("spawn node e2e helper");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        panic!("e2e helper {args:?} failed:\n{stderr}\n{stdout}");
    }
    let start = stdout
        .find("<<<RESULT>>>")
        .unwrap_or_else(|| panic!("no result marker for {args:?}:\n{stdout}\n{stderr}"))
        + "<<<RESULT>>>".len();
    let end = stdout.find("<<<END>>>").expect("result end marker");
    serde_json::from_str(&stdout[start..end]).expect("parse e2e result")
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub prefix: String,
    pub queues: Vec<QueueManifest>,
    pub flow: FlowManifest,
}

impl Manifest {
    pub fn queue(&self, name: &str) -> &QueueManifest {
        self.queues
            .iter()
            .find(|q| q.name == name)
            .unwrap_or_else(|| panic!("queue {name} not in manifest"))
    }
}

#[derive(Debug, Deserialize)]
pub struct QueueManifest {
    pub name: String,
    pub counts: Counts,
    #[serde(rename = "isPaused")]
    pub is_paused: bool,
    #[serde(rename = "globalConcurrency")]
    pub global_concurrency: Option<i64>,
    #[serde(rename = "jobsByState")]
    pub jobs_by_state: HashMap<String, Vec<String>>,
    #[serde(rename = "sampleJobs")]
    pub sample_jobs: Vec<SampleJob>,
    pub metrics: Option<MetricsManifest>,
}

#[derive(Debug, Deserialize)]
pub struct Counts {
    pub active: i64,
    pub waiting: i64,
    #[serde(rename = "waitingChildren")]
    pub waiting_children: i64,
    pub prioritized: i64,
    pub completed: i64,
    pub failed: i64,
    pub delayed: i64,
    pub paused: i64,
}

#[derive(Debug, Deserialize)]
pub struct SampleJob {
    pub state: String,
    pub job: serde_json::Value,
    pub logs: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct MetricsManifest {
    #[serde(rename = "metaCount")]
    pub meta_count: i64,
    #[serde(rename = "prevTS")]
    pub prev_ts: i64,
    #[serde(rename = "prevCount")]
    pub prev_count: i64,
    #[serde(rename = "dataLen")]
    pub data_len: usize,
}

#[derive(Debug, Deserialize)]
pub struct FlowManifest {
    #[serde(rename = "rootQueue")]
    pub root_queue: String,
    #[serde(rename = "rootId")]
    pub root_id: String,
    #[serde(rename = "childQueue")]
    pub child_queue: String,
    #[serde(rename = "childIds")]
    pub child_ids: Vec<String>,
}
