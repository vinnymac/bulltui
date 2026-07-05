# bulltui

[![npm](https://img.shields.io/npm/v/bulltui.svg)](https://npmx.dev/package/bulltui)
[![Downloads](https://img.shields.io/npm/dm/bulltui.svg)](https://npmx.dev/package/bulltui)
[![GHCR](https://ghcr-badge.egpl.dev/vinnymac/bulltui/latest_tag?trim=patch&label=ghcr)](https://github.com/vinnymac/bulltui/pkgs/container/bulltui)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A fast, keyboard-driven terminal UI for [BullMQ](https://docs.bullmq.io/). Written in Rust with [ratatui](https://ratatui.rs), it connects to Redis/Valkey directly.

<!-- Demo: drop the recorded gif at assets/demo.gif -->
![bulltui](assets/demo.gif)

## Install

```sh
# run via npx
npx bulltui

# or install globally
npm install -g bulltui

# Docker
docker run --rm -it ghcr.io/vinnymac/bulltui --url redis://my-redis:6379
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

`BULLTUI_REDIS_URL` and `BULLTUI_PREFIX` mirror `--url` / `--prefix`. Run `bulltui --help` for the full flag list.

### TLS

Use a `rediss://` URL to connect over TLS, the standard for managed brokers like AWS ElastiCache (in-transit encryption), Upstash, Redis Cloud, and Azure Cache. The binary bundles Mozilla's CA roots via `webpki-roots`.

```sh
bulltui --url rediss://user:pass@my-redis.example.com:6380
```

For a broker with a self-signed or private-CA certificate on a trusted network, `--insecure` skips certificate verification. Use this only when you understand the trade-off. It rejects plaintext `redis://` URLs.

## Features

- **Every bull-board operation**: pause/resume (including all queues), empty, obliterate, clean, retry all, promote all, add jobs, set concurrency, and per-job retry, promote, remove, duplicate, update.
- **Full job detail**: data, options, progress, error and stack trace, logs, timeline, and a navigable parent-to-children Flow tree.
- **Beyond bull-board**: a live `XREAD` events feed, a workers/busy view with lock-TTL health, job schedulers with next-run countdowns, a fuzzy command palette, and multi-select bulk actions.
- **Verified against real BullMQ**: Rust end-to-end tests assert the client and TUI reproduce BullMQ's own view.
- **Works over SSH and tmux**: vim-style keys throughout. `y` copies any detail tab to the clipboard via OSC 52.

## Keys

`?` context help · `Enter`/`→` drill in · `Esc` back · `:` command palette ·
`/` filter · `i` Redis stats · `w` workers · `E` events · `S` schedulers ·
`q` quit. Every screen shows its own bindings in the status bar.

## Containers

bulltui ships as a small, distroless container image.

```sh
docker run --rm -it ghcr.io/vinnymac/bulltui --url redis://host.docker.internal:6379
```

### Kubernetes

Run bulltui as an ephemeral pod to reach a Redis/Valkey instance inside your cluster's VPC:

```sh
kubectl run bulltui --rm -it --restart=Never \
  --image=vinnymac/bulltui:latest \
  --env="BULLTUI_REDIS_URL=redis://redis.default.svc.cluster.local:6379"
```

`-it` gives the TUI a TTY and `--rm` cleans the pod up on exit. Point `BULLTUI_REDIS_URL` at the in-cluster Service DNS name (or ClusterIP) of your broker. Set `BULLTUI_PREFIX` if you use a prefix other than `bull`. For a managed broker outside the cluster, use a `rediss://` URL (see [TLS](#tls)).

## Development

```sh
just            # list tasks
just check      # fmt-check + clippy
just test       # full workspace suite (needs Docker)
just demo       # start a local Valkey + seed it
just run        # run against the demo Valkey
```

The workspace is two crates: [`bullmq`](crates/bullmq), a reusable direct-to-Redis BullMQ client (reads and admin writes), and [`bulltui`](crates/bulltui), the ratatui TUI built on top of it.

## License

Apache-2.0
