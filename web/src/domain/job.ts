import type { Job } from "../api/projects";

export type QueueState
  = | { phase: "idle"; }
    | { phase: "queued"; retryOf: string | undefined; }
    | { phase: "waiting"; untilIso: string; reason: string | undefined; }
    | { phase: "running"; }
    | { phase: "done"; at: string; }
    | { phase: "failed"; error: string | undefined; attempts: number; };

// A running job is followed closely, because it ends when it ends. A job that is only
// queued cannot start before the app's next schedule tick, so reading faster than the
// tick shows the same answer again.
const RUNNING_POLL_MS = 4000;
const SCHEDULE_TICK_MS = 15_000;
// A waiting job is read a moment after it comes due, so the read sees the job the worker
// has taken rather than the one that was still waiting.
const WAKE_MARGIN_MS = 1000;

// The newest job is the answer: an older one is history a reader did not ask for. A queued
// job that already carries an error is a retry, which reads differently from a first run.
export const queueState = (jobs: readonly Job[], nowMs = Date.now()): QueueState => {
  const newest = jobs.at(0);

  if (newest === undefined) return { phase: "idle" };

  switch (newest.state) {
    case "running": {
      return { phase: "running" };
    }
    case "queued": {
      return waitMs(newest, nowMs) === undefined
        ? { phase: "queued", retryOf: newest.last_error }
        : { phase: "waiting", untilIso: newest.available_at, reason: newest.last_error };
    }
    case "failed": {
      return { phase: "failed", error: newest.last_error, attempts: newest.attempts };
    }
    case "done": {
      return { phase: "done", at: newest.finished_at ?? newest.created_at };
    }
  }
};

const MOVING_PHASES: ReadonlySet<string> = new Set(["running", "queued", "waiting"]);

// Whether the queue still owes this project work, whether or not it is doing it yet.
export const isMoving = (state: QueueState): boolean => MOVING_PHASES.has(state.phase);

// How long until reading the queue again could say something different, and `undefined`
// when only a reader's own action can change it. A job waiting for a retry or for a
// forge's request budget names the moment it comes due, so that is read once at that
// moment instead of every few seconds until then.
export const nextQueueReadMs = (jobs: readonly Job[], nowMs = Date.now()): number | undefined => {
  let hasRunning = false;
  let hasReadyJob = false;
  let soonestWaitMs: number | undefined;

  for (const job of jobs) {
    if (job.state === "running") {
      hasRunning = true;

      continue;
    }

    if (job.state !== "queued") continue;

    const waitingMs = waitMs(job, nowMs);

    if (waitingMs === undefined) {
      hasReadyJob = true;

      continue;
    }

    if (soonestWaitMs === undefined || waitingMs < soonestWaitMs) soonestWaitMs = waitingMs;
  }

  if (hasRunning) return RUNNING_POLL_MS;

  if (hasReadyJob) return SCHEDULE_TICK_MS;

  return soonestWaitMs === undefined ? undefined : soonestWaitMs + WAKE_MARGIN_MS;
};

// A queued job whose `available_at` has not arrived is waiting: a retry backing off, or a
// job deferred until a forge's request budget resets. The queue will not touch it before.
const waitMs = (job: Job, nowMs: number): number | undefined => {
  const availableAtMs = Date.parse(job.available_at);

  if (Number.isNaN(availableAtMs)) return undefined;

  const delayMs = availableAtMs - nowMs;

  return delayMs > 0 ? delayMs : undefined;
};
