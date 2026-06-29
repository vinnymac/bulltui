// Behavioural helpers for write e2e tests. Runs real bullmq workers so we can
// prove that jobs created/retried/promoted by the Rust client are actually
// processable by bullmq, and seed authentic completed/failed states.
//
// Subcommands:
//   worker  <url> <queue> <count> <timeoutMs>           run a success worker
//   addrun  <url> <queue> <count> <success|fail> <toMs> add jobs + drain them
//   gc      <url> <queue>                                print getGlobalConcurrency

import { Queue, Worker } from 'bullmq';

const [, , cmd, url, queue, ...rest] = process.argv;
if (!cmd || !url) {
  console.error('usage: node e2e.mjs <cmd> <url> [...]');
  process.exit(2);
}

const u = new URL(url);
const connection = { host: u.hostname, port: Number(u.port || 6379), maxRetriesPerRequest: null };
const prefix = 'bull';

function out(obj) {
  process.stdout.write('<<<RESULT>>>' + JSON.stringify(obj) + '<<<END>>>\n');
}

async function runWorker({ q, count, timeoutMs, mode }) {
  let completed = 0;
  let failed = 0;
  const done = new Promise((resolve) => {
    const worker = new Worker(
      q,
      async () => {
        if (mode === 'fail') throw new Error('intentional failure');
        return { ok: true };
      },
      { connection, prefix, concurrency: 4 }
    );
    const finish = () => resolve();
    worker.on('completed', () => {
      completed += 1;
      if (mode === 'success' && completed >= count) finish();
    });
    worker.on('failed', () => {
      failed += 1;
      if (mode === 'fail' && failed >= count) finish();
    });
    setTimeout(finish, timeoutMs);
  });
  await done;
  return { completed, failed };
}

async function main() {
  if (cmd === 'gc') {
    const qh = new Queue(queue, { connection, prefix });
    const gc = await qh.getGlobalConcurrency();
    out({ globalConcurrency: gc });
    process.exit(0);
  }

  if (cmd === 'worker') {
    const count = Number(rest[0]);
    const timeoutMs = Number(rest[1] || 8000);
    const res = await runWorker({ q: queue, count, timeoutMs, mode: 'success' });
    out(res);
    process.exit(0);
  }

  if (cmd === 'addrun') {
    const count = Number(rest[0]);
    const mode = rest[1] === 'fail' ? 'fail' : 'success';
    const timeoutMs = Number(rest[2] || 8000);
    const qh = new Queue(queue, { connection, prefix });
    await qh.waitUntilReady();
    for (let i = 0; i < count; i++) {
      await qh.add('job', { i }, { attempts: 1 });
    }
    const res = await runWorker({ q: queue, count, timeoutMs, mode });
    out(res);
    process.exit(0);
  }

  console.error('unknown cmd', cmd);
  process.exit(2);
}

main().catch((e) => {
  console.error('[e2e] fatal', e);
  process.exit(1);
});
