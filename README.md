# bulltui

[![npm](https://img.shields.io/npm/v/bulltui.svg)](https://www.npmjs.com/package/bulltui)
[![crates.io](https://img.shields.io/crates/v/bulltui.svg)](https://crates.io/crates/bulltui)
[![Downloads](https://img.shields.io/npm/dm/bulltui.svg)](https://www.npmjs.com/package/bulltui)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A fast, keyboard-driven terminal UI for [BullMQ](https://docs.bullmq.io/) — with
feature parity with [bull-board](https://github.com/felixmosh/bull-board), in
your terminal. Written in Rust with [ratatui](https://ratatui.rs); it talks to
Redis/Valkey **directly**, so there's no Node.js runtime to stand up.

<!-- Demo: drop the recorded gif at assets/demo.gif -->
![bulltui](assets/demo.gif)

## Install

```sh
# JS / BullMQ devs — run it straight from npm, no install
npx bulltui

# or install it globally
npm install -g bulltui

# Rust devs
cargo install bulltui

# Docker — see "Containers" below
docker run --rm -it vinnymac/bulltui --url redis://my-redis:6379
```

## Usage

```sh
bulltui                                 # auto-discover queues on localhost:6379
bulltui --url redis://my-redis:6379     # point at any Redis/Valkey
bulltui --url rediss://my-redis:6380    # TLS (rediss://) for managed brokers
bulltui -q emails -q notifications      # restrict to named queues
bulltui --read-only                     # no writes / admin actions
bulltui --no-splash                     # skip the startup splash
bulltui --splash-preview                # hold the splash on screen (any key exits)
bulltui --snapshot                      # render one frame to stdout and exit
```

`BULLTUI_REDIS_URL` and `BULLTUI_PREFIX` mirror `--url` / `--prefix`. Run
`bulltui --help` for the full flag list.

### TLS

Use a `rediss://` URL to connect over TLS — the norm for managed brokers such
as AWS ElastiCache (in-transit encryption), Upstash, Redis Cloud, and Azure
Cache. The server certificate is verified against a CA bundle compiled into the
binary (Mozilla's roots via `webpki-roots`), so there's nothing to install —
even in the distroless image.

```sh
bulltui --url rediss://user:pass@my-redis.example.com:6380
```

For a broker with a self-signed or private-CA certificate on a trusted network,
`--insecure` skips certificate verification. This disables authentication of the
server (man-in-the-middle exposure), so reach for it only when you understand
the trade-off; it errors on a plaintext `redis://` URL rather than doing nothing.

## Features

- **Every bull-board operation** — pause/resume (incl. all queues), empty,
  obliterate, clean, retry-/promote-all, add jobs, set concurrency; per-job
  retry, promote, remove, duplicate, update.
- **Full job detail** — data, options, progress, error + stack trace, logs,
  timeline, and a navigable parent→children **Flow** tree.
- **Beyond bull-board** — a live `XREAD` **events feed**, a **workers/busy**
  view with lock-TTL health, **job schedulers** with next-run countdowns, a
  fuzzy **command palette**, and multi-select **bulk** actions.
- **Verified against real BullMQ** — a Node seeder drives authentic
  queues/jobs/flows; Rust e2e tests assert the client and TUI reproduce
  bullmq's own view.
- **Works over SSH & tmux** — vim-style keys throughout; `y` copies any detail
  tab to the clipboard via OSC 52.

## Keys

`?` context help · `Enter`/`→` drill in · `Esc` back · `:` command palette ·
`/` filter · `i` Redis stats · `w` workers · `E` events · `S` schedulers ·
`q` quit. Every screen shows its own bindings in the status bar.

## Containers

bulltui ships as a small, distroless container image, so you can run it on
cloud infra without a Node or Rust toolchain.

```sh
docker build -t bulltui .
docker run --rm -it bulltui --url redis://host.docker.internal:6379
```

### Kubernetes

Reach a Redis/Valkey that only lives inside your cluster's VPC by running
bulltui as an ephemeral pod right next to it — no port-forward, no public
exposure:

```sh
kubectl run bulltui --rm -it --restart=Never \
  --image=vinnymac/bulltui:latest \
  --env="BULLTUI_REDIS_URL=redis://redis.default.svc.cluster.local:6379"
```

`-it` gives the TUI a TTY and `--rm` cleans the pod up on exit. Point
`BULLTUI_REDIS_URL` at the in-cluster Service DNS name (or ClusterIP) of your
broker; set `BULLTUI_PREFIX` if you don't use the default `bull`. For a managed
broker outside the cluster, use a `rediss://` URL (see [TLS](#tls)).

## Development

```sh
just            # list tasks
just check      # fmt-check + clippy
just test       # full workspace suite (needs Docker + Node)
just demo       # start a local Valkey + seed it
just run        # run against the demo Valkey
```

The workspace is two crates: [`bullmq`](crates/bullmq) — a reusable,
direct-to-Redis BullMQ client (reads + admin writes) — and
[`bulltui`](crates/bulltui), the ratatui TUI built on top of it.

## License

Apache-2.0
