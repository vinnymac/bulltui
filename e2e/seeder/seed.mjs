// Seeds authentic BullMQ data into a Redis/Valkey instance and prints a
// manifest describing bullmq's OWN view of that data. The Rust client is then
// asserted to reproduce this manifest exactly — i.e. "bulltui == bullmq" on
// identical data.
//
// Usage: node seed.mjs <redis-url>
// Output: a JSON manifest wrapped in <<<MANIFEST>>> ... <<<END>>> on stdout.

import { Queue, Worker, FlowProducer } from 'bullmq';
import IORedis from 'ioredis';

const url = process.argv[2];
if (!url) {
  console.error('usage: node seed.mjs <redis-url>');
  process.exit(2);
}

const u = new URL(url);
const connection = {
  host: u.hostname,
  port: Number(u.port || 6379),
  maxRetriesPerRequest: null,
};

const log = (...a) => console.error('[seeder]', ...a);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Watchdog: never hang a test run.
const watchdog = setTimeout(() => {
  console.error('[seeder] watchdog timeout — exiting');
  process.exit(3);
}, 90000);
watchdog.unref?.();

async function waitFor(fn, timeoutMs = 15000, intervalMs = 100) {
  const start = Date.now();
  for (;;) {
    if (await fn()) return;
    if (Date.now() - start > timeoutMs) throw new Error('waitFor timed out');
    await sleep(intervalMs);
  }
}

// bullmq getJobCounts -> our normalized shape.
async function countsOf(queue) {
  const c = await queue.getJobCounts(
    'active',
    'waiting',
    'waiting-children',
    'prioritized',
    'completed',
    'failed',
    'delayed',
    'paused'
  );
  return {
    active: c.active || 0,
    waiting: c.waiting || 0,
    waitingChildren: c['waiting-children'] || 0,
    prioritized: c.prioritized || 0,
    completed: c.completed || 0,
    failed: c.failed || 0,
    delayed: c.delayed || 0,
    paused: c.paused || 0,
  };
}

async function dumpQueue(queue, opts = {}) {
  const counts = await countsOf(queue);
  const isPaused = await queue.isPaused();
  const globalConcurrency = await queue.getGlobalConcurrency();

  const states = [
    'active',
    'waiting',
    'waiting-children',
    'prioritized',
    'completed',
    'failed',
    'delayed',
    'paused',
  ];
  const jobsByState = {};
  for (const s of states) {
    const jobs = await queue.getJobs([s], 0, 50);
    jobsByState[s] = jobs.filter(Boolean).map((j) => j.id);
  }

  let sampleJobs = [];
  if (opts.sampleStates) {
    for (const s of opts.sampleStates) {
      const jobs = await queue.getJobs([s], 0, 3);
      for (const j of jobs.filter(Boolean)) {
        const { logs } = await queue.getJobLogs(j.id, 0, -1);
        sampleJobs.push({ state: s, job: j.toJSON(), logs });
      }
    }
  }

  let metrics = null;
  if (opts.metrics) {
    const m = await queue.getMetrics('completed', 0, -1);
    // m.meta maps to the `metrics:completed` hash; m.count is the data-point count.
    metrics = {
      metaCount: m.meta.count,
      prevTS: m.meta.prevTS,
      prevCount: m.meta.prevCount,
      dataLen: m.data.length,
    };
  }

  return {
    name: queue.name,
    counts,
    isPaused,
    globalConcurrency,
    jobsByState,
    sampleJobs,
    metrics,
  };
}

async function main() {
  const prefix = 'bull';
  const manifest = { prefix, queues: [], flow: null };

  // Start from a clean slate. The manifest describes bullmq's whole-DB view, so
  // leftover jobs from a prior run would make it lie — and the absolute-count
  // waitFor() checks below assume counts start at zero. The e2e harness already
  // uses a fresh container per run, so this is a no-op there; it's what keeps
  // the persistent `just demo` Valkey reproducible.
  const flusher = new IORedis(url, { maxRetriesPerRequest: null });
  await flusher.flushall();
  await flusher.quit();

  // -- emails: completed + failed via a real worker --------------------------
  const emails = new Queue('emails', { connection, prefix });
  await emails.waitUntilReady();
  for (let i = 0; i < 5; i++) {
    await emails.add('ok', { to: `user${i}@example.com`, n: i }, { attempts: 1 });
  }
  for (let i = 0; i < 3; i++) {
    await emails.add('fail', { to: `bad${i}@example.com` }, { attempts: 1 });
  }
  const emailWorker = new Worker(
    'emails',
    async (job) => {
      await job.log(`processing ${job.name} ${job.id}`);
      if (job.name === 'fail') {
        throw new Error(`boom for ${job.id}`);
      }
      await job.log(`done ${job.id}`);
      return { ok: true, id: job.id };
    },
    { connection, prefix, concurrency: 4, metrics: { maxDataPoints: 60 } }
  );
  log('waiting for emails to process...');
  await waitFor(async () => {
    const c = await countsOf(emails);
    return c.completed === 5 && c.failed === 3;
  });
  await emailWorker.close();

  // -- notifications: waiting / delayed / prioritized (no worker) ------------
  const notifications = new Queue('notifications', { connection, prefix });
  await notifications.waitUntilReady();
  for (let i = 0; i < 4; i++) {
    await notifications.add('notify', { i }, { attempts: 2 });
  }
  for (let i = 0; i < 2; i++) {
    await notifications.add('later', { i }, { delay: 600000 });
  }
  for (let i = 1; i <= 3; i++) {
    await notifications.add('prio', { i }, { priority: i });
  }
  // A cron job scheduler + an interval one (each produces a delayed job).
  await notifications.upsertJobScheduler(
    'digest-cron',
    { pattern: '0 0 * * *', tz: 'America/New_York' },
    { name: 'digest', data: { kind: 'daily' } },
  );
  await notifications.upsertJobScheduler(
    'poll-every',
    { every: 60000 },
    { name: 'poll', data: {} },
  );
  await notifications.setGlobalConcurrency(5);

  // -- reports: paused -------------------------------------------------------
  const reports = new Queue('reports', { connection, prefix });
  await reports.waitUntilReady();
  await reports.pause();
  for (let i = 0; i < 2; i++) {
    await reports.add('build', { i });
  }

  // -- media: one active job (worker blocks; never closed) -------------------
  const media = new Queue('media', { connection, prefix });
  await media.waitUntilReady();
  await media.add('transcode', { file: 'a.mov' }, { attempts: 1 });
  const mediaWorker = new Worker(
    'media',
    () => new Promise(() => {}), // never resolves -> job stays active
    { connection, prefix, concurrency: 1 }
  );
  await waitFor(async () => {
    const c = await countsOf(media);
    return c.active === 1;
  });
  // intentionally do NOT close mediaWorker

  // -- flow: parent waiting-children with children --------------------------
  const flow = new FlowProducer({ connection, prefix });
  const tree = await flow.add({
    name: 'aggregate',
    queueName: 'orchestrator',
    data: { report: 'monthly' },
    children: [
      { name: 'collect', queueName: 'workers', data: { part: 1 } },
      { name: 'collect', queueName: 'workers', data: { part: 2 } },
      { name: 'collect', queueName: 'workers', data: { part: 3 } },
    ],
  });
  const orchestrator = new Queue('orchestrator', { connection, prefix });
  await orchestrator.waitUntilReady();

  // Give events a moment to settle.
  await sleep(300);

  // -- build manifest --------------------------------------------------------
  manifest.queues.push(await dumpQueue(emails, { sampleStates: ['completed', 'failed'], metrics: true }));
  manifest.queues.push(await dumpQueue(notifications));
  manifest.queues.push(await dumpQueue(reports));
  manifest.queues.push(await dumpQueue(media));
  manifest.queues.push(await dumpQueue(orchestrator));

  manifest.flow = {
    rootQueue: 'orchestrator',
    rootId: tree.job.id,
    childQueue: 'workers',
    childIds: (tree.children || []).map((c) => c.job.id),
  };

  process.stdout.write('<<<MANIFEST>>>' + JSON.stringify(manifest) + '<<<END>>>\n');

  // Hard-exit so the blocking media worker doesn't release its active job.
  process.exit(0);
}

main().catch((err) => {
  console.error('[seeder] fatal', err);
  process.exit(1);
});
